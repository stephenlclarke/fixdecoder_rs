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
use std::collections::hash_map::Entry as HashMapEntry;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
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
    /// Max TCP flows retained at once
    #[arg(long, default_value = "4096")]
    max_flows: usize,
    /// Max bytes buffered across all flows
    #[arg(long, default_value = "67108864")]
    max_total_bytes: usize,
    /// Idle timeout for flows in capture time seconds; 0 disables idle eviction
    #[arg(long, default_value = "60")]
    idle_timeout: u64,
    /// Exit unsuccessfully if packets or incomplete flows are dropped
    #[arg(long)]
    strict: bool,
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
    last_seen: Duration,
}

impl FlowState {
    fn new(last_seen: Duration) -> Self {
        Self {
            next_seq: None,
            buffer: Vec::new(),
            pending: BTreeMap::new(),
            last_seen,
        }
    }
}

#[derive(Error, Debug)]
enum ReassemblyError {
    #[error("flow exceeded max buffer")]
    Overflow,
    #[error("capture exceeded max total buffer")]
    TotalOverflow,
    #[error("capture exceeded max flow count")]
    FlowLimit,
}

struct ResourceBudget {
    flows: AtomicUsize,
    buffered_bytes: AtomicUsize,
    max_flows: usize,
    max_total_bytes: usize,
}

impl ResourceBudget {
    fn new(max_flows: usize, max_total_bytes: usize) -> Self {
        Self {
            flows: AtomicUsize::new(0),
            buffered_bytes: AtomicUsize::new(0),
            max_flows,
            max_total_bytes,
        }
    }

    fn try_acquire_flow(&self) -> bool {
        self.flows
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.max_flows).then_some(current + 1)
            })
            .is_ok()
    }

    fn release_flow(&self, buffered_bytes: usize) {
        self.flows.fetch_sub(1, Ordering::AcqRel);
        if buffered_bytes > 0 {
            self.buffered_bytes
                .fetch_sub(buffered_bytes, Ordering::AcqRel);
        }
    }

    fn update_buffered_bytes(&self, before: usize, after: usize) -> bool {
        if after > before {
            let total = self
                .buffered_bytes
                .fetch_add(after - before, Ordering::AcqRel)
                + (after - before);
            total <= self.max_total_bytes
        } else {
            self.buffered_bytes
                .fetch_sub(before - after, Ordering::AcqRel);
            true
        }
    }
}

#[derive(Clone)]
struct CaptureOptions {
    port_filter: Option<u16>,
    delimiter: u8,
    max_flow_bytes: usize,
    idle_timeout: Duration,
    budget: Arc<ResourceBudget>,
}

#[derive(Clone, Copy)]
struct InterfaceInfo {
    linktype: Linktype,
    timestamp_resolution: u64,
    timestamp_offset: u64,
}

struct CapturedPacket<'a> {
    data: &'a [u8],
    linktype: Linktype,
    captured_len: usize,
    captured_at: Duration,
}

#[derive(Default)]
struct CaptureState {
    legacy_linktype: Option<Linktype>,
    legacy_nanosecond_precision: bool,
    interfaces: HashMap<u32, InterfaceInfo>,
    next_if_id: u32,
    next_packet_index: u64,
    last_capture_time: Option<Duration>,
    last_idle_sweep: Option<Duration>,
    losses: usize,
}

#[derive(Debug)]
struct PacketWork {
    index: u64,
    captured_at: Duration,
    key: FlowKey,
    seq: u32,
    payload: Vec<u8>,
    starts_flow: bool,
    ends_flow: bool,
}

#[derive(Debug)]
struct PacketResult {
    index: u64,
    output: Vec<u8>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct BufferedFlowOutput {
    key: FlowKey,
    output: Vec<u8>,
}

#[derive(Default)]
struct ShardReport {
    buffered: Vec<BufferedFlowOutput>,
    losses: usize,
}

enum ShardCommand {
    Packet(PacketWork),
    EvictIdle(Duration),
}

struct FlowShard {
    sender: SyncSender<ShardCommand>,
    handle: thread::JoinHandle<ShardReport>,
}

struct FlowShardPool {
    shards: Vec<FlowShard>,
    results: Receiver<PacketResult>,
    pending: BTreeMap<u64, PacketResult>,
    next_output_index: u64,
    losses: usize,
}

const FIX_BEGIN: &[u8] = b"8=FIX";
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(1);
const WORK_QUEUE_CAPACITY: usize = 256;
const LINKTYPE_LINUX_SLL2: Linktype = Linktype(276);
const LINUX_SLL2_HEADER_LEN: usize = 20;

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
        let queue_capacity = (WORK_QUEUE_CAPACITY / shard_count).max(1);

        for index in 0..shard_count {
            let (work_tx, work_rx) = mpsc::sync_channel(queue_capacity);
            let shard_results = result_tx.clone();
            let shard_options = options.clone();
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
            losses: 0,
        }
    }

    fn submit<W: Write>(&mut self, work: PacketWork, out: &mut W) -> Result<()> {
        self.drain_ready_results(out)?;
        let shard_index = self.shard_index(work.key);
        self.shards[shard_index]
            .sender
            .send(ShardCommand::Packet(work))
            .map_err(|_| anyhow!("flow shard worker stopped unexpectedly"))?;
        self.drain_ready_results(out)
    }

    fn evict_idle<W: Write>(&mut self, now: Duration, out: &mut W) -> Result<()> {
        self.drain_ready_results(out)?;
        for shard in &self.shards {
            shard
                .sender
                .send(ShardCommand::EvictIdle(now))
                .map_err(|_| anyhow!("flow shard worker stopped unexpectedly"))?;
        }
        self.drain_ready_results(out)
    }

    fn finish<W: Write>(mut self, out: &mut W) -> Result<usize> {
        self.drain_ready_results(out)?;

        let mut buffered = Vec::new();
        for shard in self.shards.drain(..) {
            drop(shard.sender);
            let report = shard
                .handle
                .join()
                .map_err(|_| anyhow!("flow shard worker panicked"))?;
            self.losses += report.losses;
            buffered.extend(report.buffered);
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

        Ok(self.losses)
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
            for warning in result.warnings {
                eprintln!("{warning}");
                self.losses += 1;
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
) -> ShardReport {
    let mut flows = HashMap::new();
    let mut scratch = Vec::new();
    let mut losses = 0;

    while let Ok(command) = work_rx.recv() {
        match command {
            ShardCommand::Packet(work) => {
                let mut output = Vec::new();
                let warnings = match handle_packet_work(
                    &work,
                    &options,
                    &mut flows,
                    &mut scratch,
                    &mut output,
                ) {
                    Ok(warnings) => warnings,
                    Err(err) => vec![format!("warn: skipping packet: {err}")],
                };

                if result_tx
                    .send(PacketResult {
                        index: work.index,
                        output,
                        warnings,
                    })
                    .is_err()
                {
                    break;
                }

                losses += evict_idle_at(
                    &mut flows,
                    options.idle_timeout,
                    work.captured_at,
                    &options.budget,
                );
            }
            ShardCommand::EvictIdle(now) => {
                losses += evict_idle_at(&mut flows, options.idle_timeout, now, &options.budget);
            }
        }
    }

    let (buffered, trailing_losses) =
        flush_remaining_flow_outputs(&mut flows, options.delimiter, &mut scratch, &options.budget);
    ShardReport {
        buffered,
        losses: losses + trailing_losses,
    }
}

fn main() -> Result<()> {
    run(Args::parse())
}

fn run(args: Args) -> Result<()> {
    if args.max_flow_bytes == 0 {
        return Err(anyhow!("--max-flow-bytes must be greater than zero"));
    }
    if args.max_flows == 0 {
        return Err(anyhow!("--max-flows must be greater than zero"));
    }
    if args.max_total_bytes == 0 {
        return Err(anyhow!("--max-total-bytes must be greater than zero"));
    }

    let options = CaptureOptions {
        port_filter: args.port,
        delimiter: parse_delimiter(&args.delimiter)?,
        max_flow_bytes: args.max_flow_bytes,
        idle_timeout: Duration::from_secs(args.idle_timeout),
        budget: Arc::new(ResourceBudget::new(args.max_flows, args.max_total_bytes)),
    };
    let mut reader = open_reader(&args.input)?;
    let mut state = CaptureState::default();
    let mut shards = FlowShardPool::new(options.clone());
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    process_capture(&mut *reader, &options, &mut state, &mut shards, &mut stdout)?;
    let losses = state.losses + shards.finish(&mut stdout)?;
    stdout.flush()?;
    if losses > 0 {
        eprintln!("warn: capture completed with {losses} dropped packet(s) or incomplete flow(s)");
        if args.strict {
            return Err(anyhow!("capture was incomplete ({losses} loss event(s))"));
        }
    }
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
                handle_block(block, options, state, shards, out)?;
                reader.consume(offset);
            }
            Err(pcap_parser::PcapError::Eof) => return Ok(()),
            Err(pcap_parser::PcapError::Incomplete(_)) => {
                reader
                    .refill()
                    .map_err(|err| anyhow!("failed to refill reader: {err}"))?;
            }
            Err(err) => return Err(anyhow!("pcap parse error: {err}")),
        }
    }
}

fn sweep_idle_flows<W: Write>(
    captured_at: Duration,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) -> Result<()> {
    state.last_capture_time = Some(captured_at);
    if options.idle_timeout.is_zero() {
        return Ok(());
    }

    let should_sweep = state
        .last_idle_sweep
        .map(|last| captured_at.saturating_sub(last) >= IDLE_SWEEP_INTERVAL)
        .unwrap_or(true);
    if should_sweep {
        shards.evict_idle(captured_at, out)?;
        state.last_idle_sweep = Some(captured_at);
    }
    Ok(())
}

fn handle_block<W: Write>(
    block: PcapBlockOwned<'_>,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) -> Result<()> {
    match block {
        PcapBlockOwned::LegacyHeader(header) => {
            state.legacy_linktype = Some(header.network);
            state.legacy_nanosecond_precision = header.is_nanosecond_precision();
        }
        PcapBlockOwned::Legacy(packet) => {
            let linktype = state.legacy_linktype.unwrap_or(Linktype::ETHERNET);
            let captured_at = capture_timestamp(
                packet.ts_sec,
                packet.ts_usec,
                if state.legacy_nanosecond_precision {
                    1_000_000_000
                } else {
                    1_000_000
                },
            )?;
            handle_packet_block(
                CapturedPacket {
                    data: packet.data,
                    linktype,
                    captured_len: packet.caplen as usize,
                    captured_at,
                },
                options,
                state,
                shards,
                out,
            )?;
        }
        PcapBlockOwned::NG(block) => handle_ng_block(block, options, state, shards, out)?,
    }
    Ok(())
}

fn handle_ng_block<W: Write>(
    block: Block<'_>,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) -> Result<()> {
    match block {
        Block::SectionHeader(_) => {
            state.interfaces.clear();
            state.next_if_id = 0;
        }
        Block::InterfaceDescription(description) => {
            let Some(timestamp_resolution) = description.ts_resolution() else {
                record_capture_loss(state, "invalid PCAPNG interface timestamp resolution");
                state.next_if_id += 1;
                return Ok(());
            };
            state.interfaces.insert(
                state.next_if_id,
                InterfaceInfo {
                    linktype: description.linktype,
                    timestamp_resolution,
                    timestamp_offset: description.ts_offset() as u64,
                },
            );
            state.next_if_id += 1;
        }
        Block::EnhancedPacket(packet) => {
            if let Some(interface) = state.interfaces.get(&packet.if_id).copied() {
                let (seconds, fractional) =
                    packet.decode_ts(interface.timestamp_offset, interface.timestamp_resolution);
                let captured_at =
                    capture_timestamp(seconds, fractional, interface.timestamp_resolution)?;
                handle_packet_block(
                    CapturedPacket {
                        data: packet.packet_data(),
                        linktype: interface.linktype,
                        captured_len: packet.caplen as usize,
                        captured_at,
                    },
                    options,
                    state,
                    shards,
                    out,
                )?;
            } else {
                record_capture_loss(
                    state,
                    &format!(
                        "PCAPNG packet references unknown interface {}",
                        packet.if_id
                    ),
                );
            }
        }
        Block::SimplePacket(packet) => {
            if let Some(interface) = state.interfaces.get(&0).copied() {
                handle_packet_block(
                    CapturedPacket {
                        data: packet.packet_data(),
                        linktype: interface.linktype,
                        captured_len: packet.origlen as usize,
                        captured_at: state.last_capture_time.unwrap_or_default(),
                    },
                    options,
                    state,
                    shards,
                    out,
                )?;
            } else {
                record_capture_loss(state, "PCAPNG simple packet has no interface description");
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_packet_block<W: Write>(
    captured: CapturedPacket<'_>,
    options: &CaptureOptions,
    state: &mut CaptureState,
    shards: &mut FlowShardPool,
    out: &mut W,
) -> Result<()> {
    sweep_idle_flows(captured.captured_at, options, state, shards, out)?;
    let Some(packet) = decode_packet_data(captured.data, captured.linktype, captured.captured_len)
    else {
        record_capture_loss(state, "unsupported or truncated packet data");
        return Ok(());
    };
    if matches!(packet, PacketData::Unsupported(_)) {
        record_capture_loss(
            state,
            &format!("unsupported capture link type {}", captured.linktype.0),
        );
        return Ok(());
    }
    let index = state.next_packet_index;
    match packet_to_work(packet, options.port_filter, index, captured.captured_at) {
        Ok(Some(work)) => {
            state.next_packet_index += 1;
            if let Err(err) = shards.submit(work, out) {
                record_capture_loss(state, &format!("flow worker rejected packet: {err}"));
            }
        }
        Ok(None) => {}
        Err(err) => record_capture_loss(state, &format!("skipping packet: {err}")),
    }
    Ok(())
}

fn decode_packet_data(
    data: &[u8],
    linktype: Linktype,
    captured_len: usize,
) -> Option<PacketData<'_>> {
    if linktype != LINKTYPE_LINUX_SLL2 {
        return get_packetdata(data, linktype, captured_len);
    }
    if captured_len < LINUX_SLL2_HEADER_LEN || data.len() < captured_len {
        return None;
    }

    let protocol = u16::from_be_bytes([data[0], data[1]]);
    let hardware_type = u16::from_be_bytes([data[8], data[9]]);
    let payload = &data[LINUX_SLL2_HEADER_LEN..captured_len];
    match hardware_type {
        778 => Some(PacketData::L4(47, payload)),
        803 | 824 => Some(PacketData::Unsupported(payload)),
        _ => Some(PacketData::L3(protocol, payload)),
    }
}

fn capture_timestamp(seconds: u32, fractional: u32, resolution: u64) -> Result<Duration> {
    if resolution == 0 || u64::from(fractional) >= resolution {
        return Err(anyhow!(
            "invalid capture timestamp {seconds}+{fractional}/{resolution}"
        ));
    }
    let nanos = (u128::from(fractional) * 1_000_000_000u128 / u128::from(resolution)) as u32;
    Ok(Duration::new(u64::from(seconds), nanos))
}

fn record_capture_loss(state: &mut CaptureState, warning: &str) {
    state.losses += 1;
    eprintln!("warn: {warning}");
}

fn open_reader(path: &str) -> Result<Box<dyn PcapReaderIterator + Send>> {
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
    captured_at: Duration,
) -> Result<Option<PacketWork>> {
    match packet {
        PacketData::L2(data) => {
            let sliced = SlicedPacket::from_ethernet(data).map_err(|e| anyhow!("parse: {e:?}"))?;
            sliced_packet_to_work(sliced, port_filter, index, captured_at)
        }
        PacketData::L3(ethertype, data)
            if ethertype == ETHERTYPE_IPV4 || ethertype == ETHERTYPE_IPV6 =>
        {
            let sliced = SlicedPacket::from_ip(data).map_err(|e| anyhow!("parse: {e:?}"))?;
            sliced_packet_to_work(sliced, port_filter, index, captured_at)
        }
        _ => Ok(None),
    }
}

fn sliced_packet_to_work(
    sliced: SlicedPacket<'_>,
    port_filter: Option<u16>,
    index: u64,
    captured_at: Duration,
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
    let starts_flow = tcp.syn();
    let ends_flow = tcp.fin() || tcp.rst();
    if payload.is_empty() && !starts_flow && !ends_flow {
        return Ok(None);
    }

    Ok(Some(PacketWork {
        index,
        captured_at,
        key: FlowKey {
            src,
            dst,
            sport: tcp.source_port(),
            dport: tcp.destination_port(),
        },
        seq: tcp.sequence_number().wrapping_add(u32::from(starts_flow)),
        payload: payload.to_vec(),
        starts_flow,
        ends_flow,
    }))
}

fn handle_packet_work<W: Write>(
    work: &PacketWork,
    options: &CaptureOptions,
    flows: &mut HashMap<FlowKey, FlowState>,
    scratch: &mut Vec<u8>,
    out: &mut W,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();

    if work.starts_flow {
        if let Some(flow) = flows.remove(&work.key) {
            let buffered = buffered_bytes(&flow);
            options.budget.release_flow(buffered);
            if buffered > 0 {
                warnings.push("warn: SYN reset an incomplete existing flow".to_string());
            }
        }
    }

    if let HashMapEntry::Vacant(entry) = flows.entry(work.key) {
        if !options.budget.try_acquire_flow() {
            return Ok(vec![format!(
                "warn: skipping packet: {}",
                ReassemblyError::FlowLimit
            )]);
        }
        entry.insert(FlowState::new(work.captured_at));
    }

    let flow = flows
        .get_mut(&work.key)
        .expect("flow inserted before packet processing");
    flow.last_seen = work.captured_at;
    let before = buffered_bytes(flow);

    if !work.payload.is_empty() {
        if let Err(err) = reassemble_and_emit_with_scratch(
            flow,
            work.seq,
            &work.payload,
            options.delimiter,
            options.max_flow_bytes,
            scratch,
            out,
        ) {
            warnings.push(format!("warn: skipping packet: {err}"));
        }
    }

    let after = buffered_bytes(flow);
    if !options.budget.update_buffered_bytes(before, after) {
        flow.buffer.clear();
        flow.pending.clear();
        flow.next_seq = None;
        options.budget.update_buffered_bytes(after, 0);
        warnings.push(format!(
            "warn: skipping packet: {}",
            ReassemblyError::TotalOverflow
        ));
    }

    if work.ends_flow {
        let flow = flows
            .remove(&work.key)
            .expect("flow exists until FIN or RST handling");
        let buffered = buffered_bytes(&flow);
        options.budget.release_flow(buffered);
        if buffered > 0 {
            warnings.push("warn: FIN or RST closed an incomplete flow".to_string());
        }
    }

    Ok(warnings)
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
    let budget = Arc::new(ResourceBudget::new(1024, 16 * 1024 * 1024));
    let options = CaptureOptions {
        port_filter,
        delimiter,
        max_flow_bytes,
        idle_timeout: Duration::from_secs(60),
        budget,
    };
    let Some(work) = sliced_packet_to_work(sliced, port_filter, 0, Duration::ZERO)? else {
        return Ok(());
    };
    let mut scratch = Vec::new();
    let warnings = handle_packet_work(&work, &options, flows, &mut scratch, out)?;
    if warnings.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(warnings.join("; ")))
    }
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
    } else if sequence_after(seq, expected) {
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
    if !sequence_after(end, expected) {
        return;
    }

    let overlap = expected.wrapping_sub(seq) as usize;
    if overlap >= payload.len() {
        return;
    }
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
    while let Some(expected) = flow.next_seq {
        let Some(seq) = flow
            .pending
            .keys()
            .copied()
            .filter(|candidate| !sequence_after(*candidate, expected))
            .min_by_key(|candidate| expected.wrapping_sub(*candidate))
        else {
            break;
        };
        let segment = flow
            .pending
            .remove(&seq)
            .expect("segment present in pending map");
        append_segment(flow, seq, &segment);
    }
}

fn sequence_after(sequence: u32, reference: u32) -> bool {
    (sequence.wrapping_sub(reference) as i32) > 0
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
    budget: &ResourceBudget,
) -> (Vec<BufferedFlowOutput>, usize) {
    let mut ordered_flows: Vec<_> = flows.drain().collect();
    ordered_flows.sort_by_key(|(key, _)| *key);

    let mut outputs = Vec::new();
    let mut losses = 0;
    for (key, mut flow) in ordered_flows {
        let buffered_before_flush = buffered_bytes(&flow);
        let mut output = Vec::new();
        flush_complete_messages(&mut flow.buffer, delimiter, scratch, &mut output)
            .expect("writing flow output into a Vec cannot fail");
        if buffered_bytes(&flow) > 0 {
            losses += 1;
        }
        budget.release_flow(buffered_before_flush);
        if !output.is_empty() {
            outputs.push(BufferedFlowOutput { key, output });
        }
    }
    (outputs, losses)
}

#[cfg(test)]
fn evict_idle(flows: &mut HashMap<FlowKey, FlowState>, idle: Duration) {
    let budget = ResourceBudget::new(1024, 16 * 1024 * 1024);
    budget.flows.store(flows.len(), Ordering::Release);
    budget
        .buffered_bytes
        .store(flows.values().map(buffered_bytes).sum(), Ordering::Release);
    evict_idle_at(flows, idle, Duration::from_secs(120), &budget);
}

fn evict_idle_at(
    flows: &mut HashMap<FlowKey, FlowState>,
    idle: Duration,
    now: Duration,
    budget: &ResourceBudget,
) -> usize {
    if idle.is_zero() {
        return 0;
    }

    let mut losses = 0;
    flows.retain(|_, state| {
        if now.saturating_sub(state.last_seen) < idle {
            return true;
        }

        let buffered = buffered_bytes(state);
        if buffered > 0 {
            losses += 1;
        }
        budget.release_flow(buffered);
        false
    });
    losses
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

    fn test_options(
        delimiter: u8,
        max_flow_bytes: usize,
        idle_timeout: Duration,
    ) -> CaptureOptions {
        CaptureOptions {
            port_filter: None,
            delimiter,
            max_flow_bytes,
            idle_timeout,
            budget: Arc::new(ResourceBudget::new(1024, 16 * 1024 * 1024)),
        }
    }

    fn test_flow_key(sport: u16) -> FlowKey {
        FlowKey {
            src: "10.0.0.1".parse().unwrap(),
            dst: "10.0.0.2".parse().unwrap(),
            sport,
            dport: 9876,
        }
    }

    fn test_packet_work(index: u64, key: FlowKey, seq: u32, payload: Vec<u8>) -> PacketWork {
        PacketWork {
            index,
            captured_at: Duration::from_secs(index),
            key,
            seq,
            payload,
            starts_flow: false,
            ends_flow: false,
        }
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
        let mut flow = FlowState::new(Duration::ZERO);
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
        let mut flow = FlowState::new(Duration::ZERO);
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
        let mut flow = FlowState::new(Duration::ZERO);
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
    fn out_of_order_segments_reassemble_across_sequence_wrap() {
        let mut flow = FlowState::new(Duration::ZERO);
        let mut out = Vec::new();
        let message = build_fix_message("35=0|49=AAA|56=BBB|", b'|');
        let split1 = 8;
        let split2 = 24;
        let start = u32::MAX - 10;

        reassemble_and_emit(&mut flow, start, &message[..split1], b'|', 1024, &mut out).unwrap();
        reassemble_and_emit(
            &mut flow,
            start.wrapping_add(split2 as u32),
            &message[split2..],
            b'|',
            1024,
            &mut out,
        )
        .unwrap();
        reassemble_and_emit(
            &mut flow,
            start.wrapping_add(split1 as u32),
            &message[split1..split2],
            b'|',
            1024,
            &mut out,
        )
        .unwrap();

        let mut expected = message;
        expected.push(b'\n');
        assert_eq!(out, expected);
        assert!(flow.pending.is_empty());
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
    fn linux_cooked_v2_tcp_payload_is_decoded() {
        let payload = build_fix_message("35=0\u{0001}", 0x01);
        let builder =
            PacketBuilder::ipv4([10, 0, 0, 1], [10, 0, 0, 2], 32).tcp(7001, 9876, 42, 4096);
        let mut ip_packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut ip_packet, &payload).unwrap();

        let mut cooked_packet = vec![0; LINUX_SLL2_HEADER_LEN];
        cooked_packet[0..2].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        cooked_packet[8..10].copy_from_slice(&1u16.to_be_bytes());
        cooked_packet.extend_from_slice(&ip_packet);

        let packet =
            decode_packet_data(&cooked_packet, LINKTYPE_LINUX_SLL2, cooked_packet.len()).unwrap();
        let work = packet_to_work(packet, Some(9876), 0, Duration::ZERO)
            .unwrap()
            .expect("Linux cooked v2 packet should contain matching TCP payload");

        assert_eq!(work.key.sport, 7001);
        assert_eq!(work.key.dport, 9876);
        assert_eq!(work.payload, payload);
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
            last_seen: Duration::ZERO,
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
        let mut flow = FlowState::new(Duration::ZERO);
        let mut out = Vec::new();
        let err = reassemble_and_emit(&mut flow, 1, b"0123456789", b'|', 4, &mut out).unwrap_err();

        assert!(err.to_string().contains("flow exceeded max buffer"));
        assert!(flow.buffer.is_empty());
        assert!(flow.pending.is_empty());
        assert!(flow.next_seq.is_none());
    }

    #[test]
    fn capture_timestamp_supports_microsecond_and_nanosecond_precision() {
        assert_eq!(
            capture_timestamp(10, 500_000, 1_000_000).unwrap(),
            Duration::new(10, 500_000_000)
        );
        assert_eq!(
            capture_timestamp(10, 123_456_789, 1_000_000_000).unwrap(),
            Duration::new(10, 123_456_789)
        );
        assert!(capture_timestamp(10, 1_000_000, 1_000_000).is_err());
    }

    #[test]
    fn zero_idle_timeout_disables_eviction() {
        let key = test_flow_key(5000);
        let mut flows = HashMap::from([(key, FlowState::new(Duration::ZERO))]);
        let budget = ResourceBudget::new(1, 1024);
        budget.flows.store(1, Ordering::Release);

        let losses = evict_idle_at(
            &mut flows,
            Duration::ZERO,
            Duration::from_secs(3600),
            &budget,
        );

        assert_eq!(losses, 0);
        assert!(flows.contains_key(&key));
    }

    #[test]
    fn syn_resets_reused_flow_without_losing_new_message() {
        let options = test_options(b'|', 1024, Duration::from_secs(60));
        let key = test_flow_key(5000);
        let first = build_fix_message("35=0|49=FIRST|", b'|');
        let second = build_fix_message("35=1|49=SECOND|", b'|');
        let mut flows = HashMap::new();
        let mut scratch = Vec::new();
        let mut out = Vec::new();

        let warnings = handle_packet_work(
            &test_packet_work(0, key, 100, first),
            &options,
            &mut flows,
            &mut scratch,
            &mut out,
        )
        .unwrap();
        assert!(warnings.is_empty());

        let mut reused = test_packet_work(1, key, 10, second);
        reused.starts_flow = true;
        let warnings =
            handle_packet_work(&reused, &options, &mut flows, &mut scratch, &mut out).unwrap();

        assert!(warnings.is_empty());
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("49=FIRST"));
        assert!(text.contains("49=SECOND"));
    }

    #[test]
    fn resource_limits_report_loss_and_clear_excess_buffers() {
        let budget = Arc::new(ResourceBudget::new(1, 3));
        let options = CaptureOptions {
            budget: Arc::clone(&budget),
            ..test_options(b'|', 1024, Duration::from_secs(60))
        };
        let mut flows = HashMap::new();
        let mut scratch = Vec::new();
        let mut out = Vec::new();

        let warnings = handle_packet_work(
            &test_packet_work(0, test_flow_key(5000), 1, b"8=FI".to_vec()),
            &options,
            &mut flows,
            &mut scratch,
            &mut out,
        )
        .unwrap();
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("total buffer")));
        assert_eq!(budget.buffered_bytes.load(Ordering::Acquire), 0);

        let warnings = handle_packet_work(
            &test_packet_work(1, test_flow_key(5001), 1, b"8=FI".to_vec()),
            &options,
            &mut flows,
            &mut scratch,
            &mut out,
        )
        .unwrap();
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("flow count")));
        assert_eq!(budget.flows.load(Ordering::Acquire), 1);
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
                last_seen: Duration::ZERO,
                ..FlowState::new(Duration::ZERO)
            },
        );
        flows.insert(
            FlowKey {
                src: "10.0.0.3".parse().unwrap(),
                dst: "10.0.0.4".parse().unwrap(),
                sport: 5002,
                dport: 5003,
            },
            FlowState::new(Duration::from_secs(119)),
        );

        evict_idle(&mut flows, Duration::from_secs(60));

        assert_eq!(flows.len(), 1);
        assert!(flows.keys().any(|flow| flow.sport == 5002));
    }

    #[test]
    fn flow_shard_evicts_stale_partial_flows_on_idle_command() {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let options = test_options(b'|', 1024, Duration::from_secs(60));
        let handle = thread::spawn(move || run_flow_shard(command_rx, result_tx, options));
        let message = build_fix_message("35=0|49=AAA|", b'|');
        let partial = message[..10].to_vec();

        command_tx
            .send(ShardCommand::Packet(PacketWork {
                index: 0,
                captured_at: Duration::ZERO,
                key: FlowKey {
                    src: "10.0.0.1".parse().unwrap(),
                    dst: "10.0.0.2".parse().unwrap(),
                    sport: 5000,
                    dport: 5001,
                },
                seq: 1,
                payload: partial,
                starts_flow: false,
                ends_flow: false,
            }))
            .unwrap();
        let packet_result = result_rx.recv().unwrap();
        assert!(packet_result.output.is_empty());

        command_tx
            .send(ShardCommand::EvictIdle(Duration::from_secs(120)))
            .unwrap();
        drop(command_tx);

        let report = handle.join().unwrap();
        assert!(
            report.buffered.is_empty(),
            "explicit idle sweeps should drop stale partial flows before final flush"
        );
        assert_eq!(report.losses, 1);
    }

    #[test]
    fn sweep_idle_flows_dispatches_at_most_once_per_interval() {
        let (command_tx, command_rx) = mpsc::sync_channel(4);
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
            ShardReport::default()
        });
        let mut pool = FlowShardPool {
            shards: vec![FlowShard {
                sender: command_tx,
                handle,
            }],
            results: result_rx,
            pending: BTreeMap::new(),
            next_output_index: 0,
            losses: 0,
        };
        let mut state = CaptureState::default();
        let mut out = Vec::new();
        let options = test_options(b'|', 1024, Duration::from_secs(60));

        sweep_idle_flows(
            Duration::from_secs(1),
            &options,
            &mut state,
            &mut pool,
            &mut out,
        )
        .unwrap();
        let first_sweep = state
            .last_idle_sweep
            .expect("first idle sweep should record a timestamp");
        sweep_idle_flows(
            Duration::from_millis(1500),
            &options,
            &mut state,
            &mut pool,
            &mut out,
        )
        .unwrap();
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
        let options = test_options(b'|', 1024, Duration::from_secs(60));
        let handle = thread::spawn(move || run_flow_shard(command_rx, result_tx, options));
        let message = build_fix_message("35=0|49=AAA|", b'|');

        command_tx
            .send(ShardCommand::Packet(PacketWork {
                index: 0,
                captured_at: Duration::ZERO,
                key: FlowKey {
                    src: "10.0.0.1".parse().unwrap(),
                    dst: "10.0.0.2".parse().unwrap(),
                    sport: 5000,
                    dport: 5001,
                },
                seq: 1,
                payload: message[..10].to_vec(),
                starts_flow: false,
                ends_flow: false,
            }))
            .unwrap();
        drop(command_tx);

        let report = handle.join().unwrap();
        assert!(
            report.buffered.is_empty(),
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
            losses: 0,
        };
        let mut out = Vec::new();

        pool.record_result(
            PacketResult {
                index: 1,
                output: b"second\n".to_vec(),
                warnings: Vec::new(),
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
                warnings: Vec::new(),
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
                ..FlowState::new(Duration::ZERO)
            },
        );
        flows.insert(
            key_a,
            FlowState {
                buffer: msg_a.clone(),
                ..FlowState::new(Duration::ZERO)
            },
        );

        let mut scratch = Vec::new();
        let budget = ResourceBudget::new(2, 16 * 1024 * 1024);
        budget.flows.store(2, Ordering::Release);
        budget
            .buffered_bytes
            .store(msg_a.len() + msg_b.len(), Ordering::Release);
        let (outputs, losses) =
            flush_remaining_flow_outputs(&mut flows, b'|', &mut scratch, &budget);

        assert!(flows.is_empty(), "all flow buffers should be drained");
        assert_eq!(losses, 0);
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
