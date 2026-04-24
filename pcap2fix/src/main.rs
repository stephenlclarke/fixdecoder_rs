// SPDX-License-Identifier: AGPL-3.0-only
// Minimal PCAP-to-FIX filter: reads PCAP (file or stdin), reassembles TCP
// streams, and emits FIX messages separated by the chosen delimiter.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use etherparse::{NetSlice, SlicedPacket, TransportSlice};
use pcap_parser::data::{get_packetdata, PacketData, ETHERTYPE_IPV4, ETHERTYPE_IPV6};
use pcap_parser::pcapng::Block;
use pcap_parser::traits::{PcapNGPacketBlock, PcapReaderIterator};
use pcap_parser::{create_reader, Linktype, PcapBlockOwned};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::net::IpAddr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// PCAP file path or "-" for stdin
    #[arg(short, long, default_value = "-")]
    input: String,
    /// TCP port filter (optional). If omitted, all ports are considered.
    #[arg(short = 'p', long)]
    port: Option<u16>,
    /// Message delimiter. Accepts "SOH", literal char, or hex like \x01.
    #[arg(short = 'd', long, default_value = "SOH")]
    delimiter: String,
    /// Max bytes to buffer per flow before eviction
    #[arg(long, default_value = "1048576")]
    max_flow_bytes: usize,
    /// Idle timeout for flows (seconds)
    #[arg(long, default_value = "60")]
    idle_timeout: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct FlowKey {
    src: IpAddr,
    dst: IpAddr,
    sport: u16,
    dport: u16,
    // direction handled by seq tracking in FlowState
}

#[derive(Debug)]
struct FlowState {
    next_seq: Option<u32>,
    buffer: Vec<u8>,
    pending: BTreeMap<u32, Vec<u8>>,
    last_seen: Instant,
}

impl Default for FlowState {
    fn default() -> Self {
        FlowState {
            next_seq: None,
            buffer: Vec::new(),
            pending: BTreeMap::new(),
            last_seen: Instant::now(),
        }
    }
}

#[derive(Error, Debug)]
enum ReassemblyError {
    #[error("flow exceeded max buffer")]
    Overflow,
}

#[derive(Clone, Copy)]
struct CaptureOptions {
    port_filter: Option<u16>,
    delimiter: u8,
    max_flow_bytes: usize,
    idle_timeout: Duration,
}

#[derive(Default)]
struct CaptureState {
    legacy_linktype: Option<Linktype>,
    idb_linktypes: HashMap<u32, Linktype>,
    next_if_id: u32,
    next_packet_index: u64,
    last_idle_sweep: Option<Instant>,
}

#[derive(Debug)]
struct PacketWork {
    index: u64,
    seen_at: Instant,
    key: FlowKey,
    seq: u32,
    payload: Vec<u8>,
}

#[derive(Debug)]
struct PacketResult {
    index: u64,
    output: Vec<u8>,
    warning: Option<String>,
}

#[derive(Debug)]
struct BufferedFlowOutput {
    key: FlowKey,
    output: Vec<u8>,
}

enum ShardCommand {
    Packet(PacketWork),
    EvictIdle(Instant),
}

struct FlowShard {
    sender: Sender<ShardCommand>,
    handle: thread::JoinHandle<Vec<BufferedFlowOutput>>,
}

struct FlowShardPool {
    shards: Vec<FlowShard>,
    results: Receiver<PacketResult>,
    pending: BTreeMap<u64, PacketResult>,
    next_output_index: u64,
}

const FIX_BEGIN: &[u8] = b"8=FIX";
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(1);

enum MessageEnd {
    Complete(usize),
    Incomplete,
    Invalid,
}

impl FlowShardPool {
    fn new(options: CaptureOptions) -> Self {
        let shard_count = thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .max(1);
        let (result_tx, result_rx) = mpsc::channel();
        let mut shards = Vec::with_capacity(shard_count);

        for index in 0..shard_count {
            let (work_tx, work_rx) = mpsc::channel();
            let shard_results = result_tx.clone();
            let shard_options = options;
            let handle = thread::Builder::new()
                .name(format!("pcap2fix-flow-{index}"))
                .spawn(move || run_flow_shard(work_rx, shard_results, shard_options))
                .expect("flow shard thread should start");
            shards.push(FlowShard {
                sender: work_tx,
                handle,
            });
        }
        drop(result_tx);

        Self {
            shards,
            results: result_rx,
            pending: BTreeMap::new(),
            next_output_index: 0,
        }
    }

    fn submit<W: Write>(&mut self, work: PacketWork, out: &mut W) -> Result<()> {
        let shard_index = self.shard_index(work.key);
        self.shards[shard_index]
            .sender
            .send(ShardCommand::Packet(work))
            .map_err(|_| anyhow!("flow shard worker stopped unexpectedly"))?;
        self.drain_ready_results(out)
    }

    fn evict_idle<W: Write>(&mut self, now: Instant, out: &mut W) -> Result<()> {
        for shard in &self.shards {
            shard
                .sender
                .send(ShardCommand::EvictIdle(now))
                .map_err(|_| anyhow!("flow shard worker stopped unexpectedly"))?;
        }
        self.drain_ready_results(out)
    }

    fn finish<W: Write>(mut self, out: &mut W) -> Result<()> {
        self.drain_ready_results(out)?;

        let mut buffered = Vec::new();
        for shard in self.shards.drain(..) {
            drop(shard.sender);
            buffered.extend(
                shard
                    .handle
                    .join()
                    .map_err(|_| anyhow!("flow shard worker panicked"))?,
            );
        }

        while let Ok(result) = self.results.recv() {
            self.record_result(result, out)?;
        }

        if !self.pending.is_empty() {
            return Err(anyhow!("flow shard results were not emitted in full"));
        }

        buffered.sort_by_key(|item| item.key);
        for item in buffered {
            out.write_all(&item.output)?;
        }

        Ok(())
    }

    fn shard_index(&self, key: FlowKey) -> usize {
        if self.shards.len() == 1 {
            return 0;
        }
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % self.shards.len()
    }

    fn drain_ready_results<W: Write>(&mut self, out: &mut W) -> Result<()> {
        while let Ok(result) = self.results.try_recv() {
            self.record_result(result, out)?;
        }
        Ok(())
    }

    fn record_result<W: Write>(&mut self, result: PacketResult, out: &mut W) -> Result<()> {
        self.pending.insert(result.index, result);

        while let Some(result) = self.pending.remove(&self.next_output_index) {
            if let Some(warning) = result.warning {
                eprintln!("{warning}");
            }
            out.write_all(&result.output)?;
            self.next_output_index += 1;
        }

        Ok(())
    }
}

fn run_flow_shard(
    work_rx: Receiver<ShardCommand>,
    result_tx: Sender<PacketResult>,
    options: CaptureOptions,
) -> Vec<BufferedFlowOutput> {
    let mut flows = HashMap::new();
    let mut scratch = Vec::new();

    while let Ok(command) = work_rx.recv() {
        match command {
            ShardCommand::Packet(work) => {
                let mut output = Vec::new();
                let warning =
                    handle_packet_work(&work, &options, &mut flows, &mut scratch, &mut output)
                        .err()
                        .map(|err| format!("warn: skipping packet: {err}"));

                if result_tx
                    .send(PacketResult {
                        index: work.index,
                        output,
                        warning,
                    })
                    .is_err()
                {
                    break;
                }

                evict_idle_at(&mut flows, options.idle_timeout, work.seen_at);
            }
            ShardCommand::EvictIdle(now) => {
                evict_idle_at(&mut flows, options.idle_timeout, now);
            }
        }
    }

    flush_remaining_flow_outputs(&mut flows, options.delimiter, &mut scratch)
}

fn main() -> Result<()> {
    run(Args::parse())
}

fn run(args: Args) -> Result<()> {
    let options = CaptureOptions {
        port_filter: args.port,
        delimiter: parse_delimiter(&args.delimiter)?,
        max_flow_bytes: args.max_flow_bytes,
        idle_timeout: Duration::from_secs(args.idle_timeout),
    };
    let mut reader = open_reader(&args.input)?;
    let mut state = CaptureState::default();
    let mut shards = FlowShardPool::new(options);
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    process_capture(&mut *reader, &options, &mut state, &mut shards, &mut stdout)?;
    shards.finish(&mut stdout)?;
    stdout.flush()?;
    Ok(())
}

fn process_capture<R: PcapReaderIterator + ?Sized, W: Write>(
    reader: &mut R,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) -> Result<()> {
    loop {
        match reader.next() {
            Ok((offset, block)) => {
                handle_block(block, options, state, shards, out);
                reader.consume(offset);
                sweep_idle_flows(state, shards, out)?;
            }
            Err(pcap_parser::PcapError::Eof) => return Ok(()),
            Err(pcap_parser::PcapError::Incomplete) => {
                reader
                    .refill()
                    .map_err(|err| anyhow!("failed to refill reader: {err}"))?;
            }
            Err(err) => return Err(anyhow!("pcap parse error: {err}")),
        }
    }
}

fn sweep_idle_flows<W: Write>(
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) -> Result<()> {
    let now = Instant::now();
    let should_sweep = state
        .last_idle_sweep
        .map(|last| now.duration_since(last) >= IDLE_SWEEP_INTERVAL)
        .unwrap_or(true);
    if should_sweep {
        shards.evict_idle(now, out)?;
        state.last_idle_sweep = Some(now);
    }
    Ok(())
}

fn handle_block<W: Write>(
    block: PcapBlockOwned<'_>,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) {
    match block {
        PcapBlockOwned::LegacyHeader(header) => {
            state.legacy_linktype = Some(header.network);
        }
        PcapBlockOwned::Legacy(packet) => {
            let linktype = state.legacy_linktype.unwrap_or(Linktype::ETHERNET);
            handle_packet_block(
                packet.data,
                linktype,
                packet.caplen as usize,
                options,
                state,
                shards,
                out,
            );
        }
        PcapBlockOwned::NG(block) => handle_ng_block(block, options, state, shards, out),
    }
}

fn handle_ng_block<W: Write>(
    block: Block<'_>,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) {
    match block {
        Block::SectionHeader(_) => {
            state.idb_linktypes.clear();
            state.next_if_id = 0;
        }
        Block::InterfaceDescription(description) => {
            state
                .idb_linktypes
                .insert(state.next_if_id, description.linktype);
            state.next_if_id += 1;
        }
        Block::EnhancedPacket(packet) => {
            if let Some(linktype) = state.idb_linktypes.get(&packet.if_id).copied() {
                handle_packet_block(
                    packet.packet_data(),
                    linktype,
                    packet.caplen as usize,
                    options,
                    state,
                    shards,
                    out,
                );
            }
        }
        Block::SimplePacket(packet) => {
            if let Some(linktype) = state.idb_linktypes.get(&0).copied() {
                handle_packet_block(
                    packet.packet_data(),
                    linktype,
                    packet.origlen as usize,
                    options,
                    state,
                    shards,
                    out,
                );
            }
        }
        _ => {}
    }
}

fn handle_packet_block<W: Write>(
    data: &[u8],
    linktype: Linktype,
    captured_len: usize,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) {
    let Some(packet) = get_packetdata(data, linktype, captured_len) else {
        return;
    };
    let index = state.next_packet_index;
    match packet_to_work(packet, options.port_filter, index, Instant::now()) {
        Ok(Some(work)) => {
            state.next_packet_index += 1;
            if let Err(err) = shards.submit(work, out) {
                eprintln!("warn: skipping packet: {err}");
            }
        }
        Ok(None) => {}
        Err(err) => eprintln!("warn: skipping packet: {err}"),
    }
}

fn open_reader(path: &str) -> Result<Box<dyn PcapReaderIterator>> {
    if path == "-" {
        let stdin = io::stdin();
        create_reader(65536, stdin).map_err(|e| anyhow!("failed to create reader: {e}"))
    } else {
        let file = File::open(path).with_context(|| format!("open pcap {path}"))?;
        create_reader(65536, file).map_err(|e| anyhow!("failed to create reader: {e}"))
    }
}

fn parse_delimiter(raw: &str) -> Result<u8> {
    if raw.eq_ignore_ascii_case("SOH") {
        return Ok(0x01);
    }
    if let Some(hex) = raw.strip_prefix("\\x").or_else(|| raw.strip_prefix("0x")) {
        let val =
            u8::from_str_radix(hex, 16).map_err(|_| anyhow!("invalid hex delimiter: {raw}"))?;
        return Ok(val);
    }
    if raw.len() == 1 {
        return Ok(raw.as_bytes()[0]);
    }
    Err(anyhow!(
        "delimiter must be SOH, hex (\\x01), or single byte"
    ))
}

fn packet_to_work(
    packet: PacketData<'_>,
    port_filter: Option<u16>,
    index: u64,
    seen_at: Instant,
) -> Result<Option<PacketWork>> {
    match packet {
        PacketData::L2(data) => {
            let sliced = SlicedPacket::from_ethernet(data).map_err(|e| anyhow!("parse: {e:?}"))?;
            sliced_packet_to_work(sliced, port_filter, index, seen_at)
        }
        PacketData::L3(ethertype, data)
            if ethertype == ETHERTYPE_IPV4 || ethertype == ETHERTYPE_IPV6 =>
        {
            let sliced = SlicedPacket::from_ip(data).map_err(|e| anyhow!("parse: {e:?}"))?;
            sliced_packet_to_work(sliced, port_filter, index, seen_at)
        }
        _ => Ok(None),
    }
}

fn sliced_packet_to_work(
    sliced: SlicedPacket<'_>,
    port_filter: Option<u16>,
    index: u64,
    seen_at: Instant,
) -> Result<Option<PacketWork>> {
    let (src, dst, tcp) = match (sliced.net, sliced.transport) {
        (Some(NetSlice::Ipv4(ip)), Some(TransportSlice::Tcp(tcp))) => (
            IpAddr::V4(ip.header().source_addr()),
            IpAddr::V4(ip.header().destination_addr()),
            tcp,
        ),
        (Some(NetSlice::Ipv6(ip)), Some(TransportSlice::Tcp(tcp))) => (
            IpAddr::V6(ip.header().source_addr()),
            IpAddr::V6(ip.header().destination_addr()),
            tcp,
        ),
        _ => return Ok(None),
    };
    if let Some(p) = port_filter {
        if tcp.source_port() != p && tcp.destination_port() != p {
            return Ok(None);
        }
    }

    let payload = tcp.payload();
    if payload.is_empty() {
        return Ok(None);
    }

    Ok(Some(PacketWork {
        index,
        seen_at,
        key: FlowKey {
            src,
            dst,
            sport: tcp.source_port(),
            dport: tcp.destination_port(),
        },
        seq: tcp.sequence_number(),
        payload: payload.to_vec(),
    }))
}

fn handle_packet_work<W: Write>(
    work: &PacketWork,
    options: &CaptureOptions,
    flows: &mut HashMap<FlowKey, FlowState>,
    scratch: &mut Vec<u8>,
    out: &mut W,
) -> Result<()> {
    let flow = flows.entry(work.key).or_default();
    flow.last_seen = work.seen_at;
    reassemble_and_emit_with_scratch(
        flow,
        work.seq,
        &work.payload,
        options.delimiter,
        options.max_flow_bytes,
        scratch,
        out,
    )
}

#[cfg(test)]
fn handle_sliced_packet<W: Write>(
    sliced: SlicedPacket<'_>,
    port_filter: Option<u16>,
    delimiter: u8,
    max_flow_bytes: usize,
    flows: &mut HashMap<FlowKey, FlowState>,
    out: &mut W,
) -> Result<()> {
    let options = CaptureOptions {
        port_filter,
        delimiter,
        max_flow_bytes,
        idle_timeout: Duration::from_secs(60),
    };
    let Some(work) = sliced_packet_to_work(sliced, port_filter, 0, Instant::now())? else {
        return Ok(());
    };
    let mut scratch = Vec::new();
    handle_packet_work(&work, &options, flows, &mut scratch, out)
}

#[cfg(test)]
fn reassemble_and_emit<W: Write>(
    flow: &mut FlowState,
    seq: u32,
    payload: &[u8],
    delimiter: u8,
    max_flow_bytes: usize,
    out: &mut W,
) -> Result<()> {
    let mut scratch = Vec::new();
    reassemble_and_emit_with_scratch(
        flow,
        seq,
        payload,
        delimiter,
        max_flow_bytes,
        &mut scratch,
        out,
    )
}

fn reassemble_and_emit_with_scratch<W: Write>(
    flow: &mut FlowState,
    seq: u32,
    payload: &[u8],
    delimiter: u8,
    max_flow_bytes: usize,
    scratch: &mut Vec<u8>,
    out: &mut W,
) -> Result<()> {
    let expected = flow.next_seq.unwrap_or(seq);

    if seq == expected {
        append_segment(flow, seq, payload);
    } else if seq > expected {
        store_future_segment(flow, seq, payload);
    } else {
        append_segment(flow, seq, payload);
    }

    drain_pending_segments(flow);

    if buffered_bytes(flow) > max_flow_bytes {
        flow.buffer.clear();
        flow.pending.clear();
        flow.next_seq = None;
        return Err(ReassemblyError::Overflow.into());
    }

    flush_complete_messages(&mut flow.buffer, delimiter, scratch, out)?;
    Ok(())
}

fn append_segment(flow: &mut FlowState, seq: u32, payload: &[u8]) {
    let expected = flow.next_seq.unwrap_or(seq);

    if seq == expected {
        flow.buffer.extend_from_slice(payload);
        flow.next_seq = Some(seq.wrapping_add(payload.len() as u32));
        return;
    }

    let end = seq.wrapping_add(payload.len() as u32);
    if end <= expected {
        return;
    }

    let overlap = (expected - seq) as usize;
    flow.buffer.extend_from_slice(&payload[overlap..]);
    flow.next_seq = Some(expected.wrapping_add((payload.len() - overlap) as u32));
}

fn store_future_segment(flow: &mut FlowState, seq: u32, payload: &[u8]) {
    match flow.pending.entry(seq) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(payload.to_vec());
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            if payload.len() > entry.get().len() {
                entry.insert(payload.to_vec());
            }
        }
    }
}

fn drain_pending_segments(flow: &mut FlowState) {
    while let Some((&seq, _)) = flow.pending.first_key_value() {
        let expected = flow.next_seq.unwrap_or(seq);
        if seq > expected {
            break;
        }
        let segment = flow
            .pending
            .remove(&seq)
            .expect("segment present in pending map");
        append_segment(flow, seq, &segment);
    }
}

fn buffered_bytes(flow: &FlowState) -> usize {
    flow.buffer.len() + flow.pending.values().map(std::vec::Vec::len).sum::<usize>()
}

fn flush_complete_messages<W: Write>(
    buffer: &mut Vec<u8>,
    delimiter: u8,
    scratch: &mut Vec<u8>,
    out: &mut W,
) -> Result<()> {
    let mut cursor = 0;
    while cursor < buffer.len() {
        let Some(rel_start) = find_message_start(&buffer[cursor..]) else {
            if cursor > 0 {
                buffer.drain(0..cursor);
            }
            retain_partial_begin_string(buffer);
            return Ok(());
        };
        cursor += rel_start;

        match find_message_end(&buffer[cursor..], delimiter) {
            MessageEnd::Complete(rel_end) => {
                let end = cursor + rel_end;
                scratch.clear();
                scratch.extend_from_slice(&buffer[cursor..=end]);
                scratch.push(b'\n'); // newline so each FIX message prints on its own line
                out.write_all(scratch)?;
                cursor = end + 1;
            }
            MessageEnd::Incomplete => {
                if cursor > 0 {
                    buffer.drain(0..cursor);
                }
                return Ok(());
            }
            MessageEnd::Invalid => {
                cursor += 1;
            }
        }
    }
    if cursor > 0 {
        buffer.drain(0..cursor);
    }
    Ok(())
}

fn find_message_start(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(FIX_BEGIN.len())
        .position(|window| window == FIX_BEGIN)
}

fn retain_partial_begin_string(buffer: &mut Vec<u8>) {
    let keep = (1..FIX_BEGIN.len())
        .rev()
        .find(|prefix_len| buffer.ends_with(&FIX_BEGIN[..*prefix_len]))
        .unwrap_or(0);

    if keep == 0 {
        buffer.clear();
        return;
    }

    let drain_until = buffer.len().saturating_sub(keep);
    if drain_until > 0 {
        buffer.drain(0..drain_until);
    }
}

fn find_message_end(buffer: &[u8], delimiter: u8) -> MessageEnd {
    // Need at least "8=..|9=..|" plus checksum ("10=000|")
    if buffer.len() < 16 {
        return MessageEnd::Incomplete;
    }
    let Some(begin_end) = buffer.iter().position(|b| *b == delimiter) else {
        return MessageEnd::Incomplete;
    };
    let body_len_field_start = begin_end + 1;
    let Some(body_len_rel_end) = buffer[body_len_field_start..]
        .iter()
        .position(|b| *b == delimiter)
    else {
        return MessageEnd::Incomplete;
    };
    let body_len_end = body_len_field_start + body_len_rel_end;
    if body_len_end <= body_len_field_start + 1 {
        return MessageEnd::Invalid;
    }
    if !buffer[body_len_field_start..].starts_with(b"9=") {
        return MessageEnd::Invalid;
    }
    let body_len_bytes = &buffer[body_len_field_start + 2..body_len_end];
    let Some(body_len) = parse_decimal(body_len_bytes) else {
        return MessageEnd::Invalid;
    };
    let body_start = body_len_end + 1;
    let Some(body_end) = body_start.checked_add(body_len) else {
        return MessageEnd::Invalid;
    };
    // checksum starts immediately after body
    if body_end + 7 > buffer.len() {
        return MessageEnd::Incomplete;
    }
    let Some(checksum_field) = buffer.get(body_end..) else {
        return MessageEnd::Incomplete;
    };
    if !checksum_field.starts_with(b"10=") {
        return MessageEnd::Invalid;
    }
    let Some(checksum_val) = buffer.get(body_end + 3..body_end + 6) else {
        return MessageEnd::Incomplete;
    };
    if checksum_val.iter().any(|b| !b.is_ascii_digit()) {
        return MessageEnd::Invalid;
    }
    let end_delim_idx = body_end + 6;
    let Some(end_delimiter) = buffer.get(end_delim_idx) else {
        return MessageEnd::Incomplete;
    };
    if *end_delimiter != delimiter {
        return MessageEnd::Invalid;
    }
    MessageEnd::Complete(end_delim_idx)
}

fn parse_decimal(bytes: &[u8]) -> Option<usize> {
    let mut val: usize = 0;
    for b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?;
        val = val.checked_add((b - b'0') as usize)?;
    }
    Some(val)
}

fn flush_remaining_flow_outputs(
    flows: &mut HashMap<FlowKey, FlowState>,
    delimiter: u8,
    scratch: &mut Vec<u8>,
) -> Vec<BufferedFlowOutput> {
    let mut ordered_flows: Vec<_> = flows.drain().collect();
    ordered_flows.sort_by_key(|(key, _)| *key);

    let mut outputs = Vec::new();
    for (key, mut flow) in ordered_flows {
        let mut output = Vec::new();
        flush_complete_messages(&mut flow.buffer, delimiter, scratch, &mut output)
            .expect("writing flow output into a Vec cannot fail");
        if !output.is_empty() {
            outputs.push(BufferedFlowOutput { key, output });
        }
    }
    outputs
}

#[cfg(test)]
fn evict_idle(flows: &mut HashMap<FlowKey, FlowState>, idle: Duration) {
    evict_idle_at(flows, idle, Instant::now());
}

fn evict_idle_at(flows: &mut HashMap<FlowKey, FlowState>, idle: Duration, now: Instant) {
    flows.retain(|_, state| now.duration_since(state.last_seen) < idle);
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::PacketBuilder;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn build_fix_message(body: &str, delim: u8) -> Vec<u8> {
        let mut msg = Vec::new();
        let d = delim as char;
        let body_len = body.len();
        msg.extend_from_slice(format!("8=FIX.4.4{d}9={body_len}{d}").as_bytes());
        msg.extend_from_slice(body.as_bytes());
        let checksum: u8 = msg.iter().fold(0u16, |acc, b| acc + *b as u16) as u8;
        msg.extend_from_slice(format!("10={:03}{}", checksum, d).as_bytes());
        msg
    }

    #[test]
    fn parse_delimiter_variants() {
        assert_eq!(parse_delimiter("SOH").unwrap(), 0x01);
        assert_eq!(parse_delimiter("\\x02").unwrap(), 0x02);
        assert_eq!(parse_delimiter("0x03").unwrap(), 0x03);
        assert_eq!(parse_delimiter("|").unwrap(), b'|');
    }

    #[test]
    fn reassembly_appends_in_order() {
        let mut flow = FlowState::default();
        let mut out = Vec::new();
        let message = build_fix_message("35=0\u{0001}", 0x01);
        let (part1, rest) = message.split_at(10);
        let (part2, part3) = rest.split_at(8);

        reassemble_and_emit(&mut flow, 10, part1, 0x01, 1024, &mut out).unwrap();
        reassemble_and_emit(
            &mut flow,
            10 + part1.len() as u32,
            part2,
            0x01,
            1024,
            &mut out,
        )
        .unwrap();
        assert!(out.is_empty(), "no complete message yet");
        reassemble_and_emit(
            &mut flow,
            10 + (part1.len() + part2.len()) as u32,
            part3,
            0x01,
            1024,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("8=FIX.4.4"));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn flushes_full_messages_and_discards_non_fix_tail() {
        let mut buf = build_fix_message("35=0\u{0001}", 0x01);
        buf.extend_from_slice(b"extra");
        let mut out = Vec::new();
        let mut scratch = Vec::new();
        flush_complete_messages(&mut buf, 0x01, &mut scratch, &mut out).unwrap();
        let mut expected = build_fix_message("35=0\u{0001}", 0x01);
        expected.push(b'\n');
        assert_eq!(out, expected);
        assert!(
            buf.is_empty(),
            "non-FIX tail should be discarded during resync"
        );
    }

    #[test]
    fn retransmit_is_ignored() {
        let mut flow = FlowState::default();
        let mut out = Vec::new();
        let message = build_fix_message("35=0|49=AAA|", b'|');
        let part = &message[..10];

        reassemble_and_emit(&mut flow, 1, part, b'|', 1024, &mut out).unwrap();
        reassemble_and_emit(&mut flow, 1, part, b'|', 1024, &mut out).unwrap();

        assert_eq!(flow.buffer, part);
        assert!(
            out.is_empty(),
            "retransmits should not emit duplicate output"
        );
    }

    #[test]
    fn out_of_order_future_segment_is_buffered_until_gap_arrives() {
        let mut flow = FlowState::default();
        let mut out = Vec::new();
        let message = build_fix_message("35=0|49=AAA|56=BBB|", b'|');
        let partial = &message[..message.len() - 4];
        let split1 = partial.len() / 3;
        let split2 = (partial.len() * 2) / 3;
        let part1 = &partial[..split1];
        let part2 = &partial[split1..split2];
        let part3 = &partial[split2..];

        reassemble_and_emit(&mut flow, 5, part1, b'|', 1024, &mut out).unwrap();
        reassemble_and_emit(&mut flow, 5 + split2 as u32, part3, b'|', 1024, &mut out).unwrap();
        assert_eq!(flow.buffer, part1);
        assert_eq!(
            flow.pending.get(&(5 + split2 as u32)).map(Vec::as_slice),
            Some(part3)
        );

        reassemble_and_emit(&mut flow, 5 + split1 as u32, part2, b'|', 1024, &mut out).unwrap();
        assert_eq!(flow.buffer, partial);
        assert!(
            flow.pending.is_empty(),
            "future segment should drain once the gap is filled"
        );
        assert!(out.is_empty(), "incomplete messages should stay buffered");
    }

    #[test]
    fn flush_complete_messages_emits_messages_and_discards_non_fix_tail() {
        let mut buf = Vec::new();
        let msg1 = build_fix_message("35=0|", b'|');
        let msg2 = build_fix_message("35=1|", b'|');
        buf.extend_from_slice(&msg1);
        buf.extend_from_slice(&msg2);
        buf.extend_from_slice(b"partial");
        let mut scratch = Vec::new();
        let mut out = Vec::new();
        flush_complete_messages(&mut buf, b'|', &mut scratch, &mut out).unwrap();
        let expected_out = {
            let mut v = msg1.clone();
            v.push(b'\n');
            v.extend_from_slice(&msg2);
            v.push(b'\n');
            v
        };
        assert_eq!(out, expected_out);
        assert!(buf.is_empty(), "non-FIX trailing bytes should be discarded");
    }

    #[test]
    fn ipv6_tcp_payload_is_reassembled() {
        let src = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let dst = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        let payload = build_fix_message("35=0\u{0001}", 0x01);
        let builder = PacketBuilder::ipv6(src, dst, 32).tcp(7001, 9876, 42, 4096);
        let mut packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut packet, &payload).unwrap();

        let sliced = SlicedPacket::from_ip(&packet).unwrap();
        let mut flows = HashMap::new();
        let mut out = Vec::new();
        handle_sliced_packet(sliced, Some(9876), 0x01, 1024, &mut flows, &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("8=FIX.4.4"));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn flush_complete_messages_resynchronizes_after_leading_garbage() {
        let msg = build_fix_message("35=0|", b'|');
        let mut buf = b"garbage".to_vec();
        buf.extend_from_slice(&msg);
        let mut scratch = Vec::new();
        let mut out = Vec::new();

        flush_complete_messages(&mut buf, b'|', &mut scratch, &mut out).unwrap();

        let mut expected = msg.clone();
        expected.push(b'\n');
        assert_eq!(out, expected);
        assert!(
            buf.is_empty(),
            "buffer should advance past garbage and message"
        );
    }

    #[test]
    fn flush_complete_messages_skips_midstream_fragment_before_next_message() {
        let msg = build_fix_message("35=0|", b'|');
        let mut buf = b"35=0|49=AAA|".to_vec();
        buf.extend_from_slice(&msg);
        let mut scratch = Vec::new();
        let mut out = Vec::new();

        flush_complete_messages(&mut buf, b'|', &mut scratch, &mut out).unwrap();

        let mut expected = msg.clone();
        expected.push(b'\n');
        assert_eq!(out, expected);
        assert!(
            buf.is_empty(),
            "midstream fragments should not block later messages"
        );
    }

    #[test]
    fn flush_complete_messages_retains_partial_begin_string() {
        let mut buf = b"noise8=FI".to_vec();
        let mut scratch = Vec::new();
        let mut out = Vec::new();

        flush_complete_messages(&mut buf, b'|', &mut scratch, &mut out).unwrap();

        assert!(out.is_empty());
        assert_eq!(buf, b"8=FI");
    }

    #[test]
    fn parse_delimiter_rejects_invalid_values() {
        assert!(parse_delimiter("long").is_err());
        assert!(parse_delimiter("\\xGG").is_err());
    }

    #[test]
    fn find_message_end_rejects_invalid_body_length_and_checksum_fields() {
        assert!(matches!(
            find_message_end(b"8=FIX.4.4|35=0|10=000|", b'|'),
            MessageEnd::Invalid
        ));
        assert!(matches!(
            find_message_end(b"8=FIX.4.4|9=abc|35=0|10=000|", b'|'),
            MessageEnd::Invalid
        ));
        assert!(matches!(
            find_message_end(b"8=FIX.4.4|9=4|35=0|10=xyz|", b'|'),
            MessageEnd::Invalid
        ));
        assert!(matches!(
            find_message_end(b"8=FIX.4.4|9=4|35=0|10=000!", b'|'),
            MessageEnd::Invalid
        ));
    }

    #[test]
    fn retain_partial_begin_string_clears_non_matching_tail() {
        let mut buf = b"trailing-noise".to_vec();
        retain_partial_begin_string(&mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn append_segment_trims_overlap_and_store_future_segment_prefers_longest() {
        let mut flow = FlowState {
            next_seq: Some(14),
            buffer: b"8=FIX.4.4|9=".to_vec(),
            pending: BTreeMap::new(),
            last_seen: Instant::now(),
        };

        append_segment(&mut flow, 12, b"9=5|35=0|");
        assert_eq!(flow.buffer, b"8=FIX.4.4|9=5|35=0|");

        store_future_segment(&mut flow, 30, b"short");
        store_future_segment(&mut flow, 30, b"longer-segment");
        assert_eq!(
            flow.pending.get(&30).map(Vec::as_slice),
            Some(&b"longer-segment"[..])
        );
    }

    #[test]
    fn reassembly_overflow_clears_flow_state() {
        let mut flow = FlowState::default();
        let mut out = Vec::new();
        let err = reassemble_and_emit(&mut flow, 1, b"0123456789", b'|', 4, &mut out).unwrap_err();

        assert!(err.to_string().contains("flow exceeded max buffer"));
        assert!(flow.buffer.is_empty());
        assert!(flow.pending.is_empty());
        assert!(flow.next_seq.is_none());
    }

    #[test]
    fn evict_idle_drops_stale_flows() {
        let mut flows = HashMap::new();
        flows.insert(
            FlowKey {
                src: "10.0.0.1".parse().unwrap(),
                dst: "10.0.0.2".parse().unwrap(),
                sport: 5000,
                dport: 5001,
            },
            FlowState {
                last_seen: Instant::now() - Duration::from_secs(120),
                ..FlowState::default()
            },
        );
        flows.insert(
            FlowKey {
                src: "10.0.0.3".parse().unwrap(),
                dst: "10.0.0.4".parse().unwrap(),
                sport: 5002,
                dport: 5003,
            },
            FlowState::default(),
        );

        evict_idle(&mut flows, Duration::from_secs(60));

        assert_eq!(flows.len(), 1);
        assert!(flows.keys().any(|flow| flow.sport == 5002));
    }

    #[test]
    fn flow_shard_evicts_stale_partial_flows_on_idle_command() {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let options = CaptureOptions {
            port_filter: None,
            delimiter: b'|',
            max_flow_bytes: 1024,
            idle_timeout: Duration::from_secs(60),
        };
        let handle = thread::spawn(move || run_flow_shard(command_rx, result_tx, options));
        let message = build_fix_message("35=0|49=AAA|", b'|');
        let partial = message[..10].to_vec();
        let stale_seen_at = Instant::now() - Duration::from_secs(120);

        command_tx
            .send(ShardCommand::Packet(PacketWork {
                index: 0,
                seen_at: stale_seen_at,
                key: FlowKey {
                    src: "10.0.0.1".parse().unwrap(),
                    dst: "10.0.0.2".parse().unwrap(),
                    sport: 5000,
                    dport: 5001,
                },
                seq: 1,
                payload: partial,
            }))
            .unwrap();
        let packet_result = result_rx.recv().unwrap();
        assert!(packet_result.output.is_empty());

        command_tx
            .send(ShardCommand::EvictIdle(Instant::now()))
            .unwrap();
        drop(command_tx);

        let buffered = handle.join().unwrap();
        assert!(
            buffered.is_empty(),
            "explicit idle sweeps should drop stale partial flows before final flush"
        );
    }

    #[test]
    fn sweep_idle_flows_dispatches_at_most_once_per_interval() {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        drop(result_tx);
        let evictions = Arc::new(AtomicUsize::new(0));
        let thread_evictions = Arc::clone(&evictions);
        let handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                if matches!(command, ShardCommand::EvictIdle(_)) {
                    thread_evictions.fetch_add(1, Ordering::SeqCst);
                }
            }
            Vec::new()
        });
        let mut pool = FlowShardPool {
            shards: vec![FlowShard {
                sender: command_tx,
                handle,
            }],
            results: result_rx,
            pending: BTreeMap::new(),
            next_output_index: 0,
        };
        let mut state = CaptureState::default();
        let mut out = Vec::new();

        sweep_idle_flows(&mut state, &mut pool, &mut out).unwrap();
        let first_sweep = state
            .last_idle_sweep
            .expect("first idle sweep should record a timestamp");
        sweep_idle_flows(&mut state, &mut pool, &mut out).unwrap();
        assert_eq!(
            state.last_idle_sweep,
            Some(first_sweep),
            "repeated idle sweeps inside the throttle interval should be skipped"
        );

        pool.finish(&mut out).unwrap();
        assert_eq!(evictions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn flow_shard_exits_when_result_channel_is_closed() {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        drop(result_rx);
        let options = CaptureOptions {
            port_filter: None,
            delimiter: b'|',
            max_flow_bytes: 1024,
            idle_timeout: Duration::from_secs(60),
        };
        let handle = thread::spawn(move || run_flow_shard(command_rx, result_tx, options));
        let message = build_fix_message("35=0|49=AAA|", b'|');

        command_tx
            .send(ShardCommand::Packet(PacketWork {
                index: 0,
                seen_at: Instant::now(),
                key: FlowKey {
                    src: "10.0.0.1".parse().unwrap(),
                    dst: "10.0.0.2".parse().unwrap(),
                    sport: 5000,
                    dport: 5001,
                },
                seq: 1,
                payload: message[..10].to_vec(),
            }))
            .unwrap();
        drop(command_tx);

        let buffered = handle.join().unwrap();
        assert!(
            buffered.is_empty(),
            "worker shutdown on a closed result channel should not flush stale partial output"
        );
    }

    #[test]
    fn flow_shard_pool_orders_results_by_packet_index() {
        let (_tx, rx) = mpsc::channel();
        let mut pool = FlowShardPool {
            shards: Vec::new(),
            results: rx,
            pending: BTreeMap::new(),
            next_output_index: 0,
        };
        let mut out = Vec::new();

        pool.record_result(
            PacketResult {
                index: 1,
                output: b"second\n".to_vec(),
                warning: None,
            },
            &mut out,
        )
        .unwrap();
        assert!(
            out.is_empty(),
            "later packets should wait for earlier output"
        );

        pool.record_result(
            PacketResult {
                index: 0,
                output: b"first\n".to_vec(),
                warning: None,
            },
            &mut out,
        )
        .unwrap();

        assert_eq!(out, b"first\nsecond\n");
        assert!(pool.pending.is_empty());
        assert_eq!(pool.next_output_index, 2);
    }

    #[test]
    fn flush_remaining_flow_outputs_are_sorted_by_flow_key() {
        let msg_a = build_fix_message("35=0|49=AAA|", b'|');
        let msg_b = build_fix_message("35=1|49=BBB|", b'|');
        let key_a = FlowKey {
            src: "10.0.0.1".parse().unwrap(),
            dst: "10.0.0.2".parse().unwrap(),
            sport: 5000,
            dport: 5001,
        };
        let key_b = FlowKey {
            src: "10.0.0.3".parse().unwrap(),
            dst: "10.0.0.4".parse().unwrap(),
            sport: 5002,
            dport: 5003,
        };
        let mut flows = HashMap::new();
        flows.insert(
            key_b,
            FlowState {
                buffer: msg_b.clone(),
                ..FlowState::default()
            },
        );
        flows.insert(
            key_a,
            FlowState {
                buffer: msg_a.clone(),
                ..FlowState::default()
            },
        );

        let mut scratch = Vec::new();
        let outputs = flush_remaining_flow_outputs(&mut flows, b'|', &mut scratch);

        assert!(flows.is_empty(), "all flow buffers should be drained");
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].key, key_a);
        assert_eq!(outputs[1].key, key_b);

        let expected_a = {
            let mut value = msg_a.clone();
            value.push(b'\n');
            value
        };
        let expected_b = {
            let mut value = msg_b.clone();
            value.push(b'\n');
            value
        };
        assert_eq!(outputs[0].output, expected_a);
        assert_eq!(outputs[1].output, expected_b);
    }

    #[test]
    fn open_reader_errors_for_missing_file() {
        let err = open_reader("/definitely/missing/file.pcap")
            .err()
            .expect("missing file should error");
        assert!(err.to_string().contains("open pcap"));
    }
}
