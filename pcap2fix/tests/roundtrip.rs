use assert_cmd::Command;

/// Build a minimal FIX message with correct BodyLength/Checksum using the given delimiter.
fn build_fix_message(delim: u8) -> Vec<u8> {
    build_fix_message_with_type(delim, "0")
}

fn build_fix_message_with_type(delim: u8, message_type: &str) -> Vec<u8> {
    let d = delim as char;
    let body = format!("35={message_type}{d}");
    let body_len = body.len();
    let mut msg = format!("8=FIX.4.2{d}9={body_len}{d}{body}").into_bytes();
    let checksum: u8 = msg.iter().fold(0u16, |acc, b| acc + *b as u16) as u8;
    msg.extend_from_slice(format!("10={:03}{}", checksum, d).as_bytes());
    msg
}

/// Construct one Ethernet/IPv4/TCP packet.
fn build_packet(payload: &[u8], seq: u32, flags: u8) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&[0, 1, 2, 3, 4, 5]); // dst
    pkt.extend_from_slice(&[6, 7, 8, 9, 10, 11]); // src
    pkt.extend_from_slice(&[0x08, 0x00]); // ethertype IPv4

    let ip_header_len = 20u16;
    let tcp_header_len = 20u16;
    let total_len = ip_header_len + tcp_header_len + payload.len() as u16;
    pkt.extend_from_slice(&[0x45, 0x00]); // version/IHL, DSCP
    pkt.extend_from_slice(&total_len.to_be_bytes());
    pkt.extend_from_slice(&[0x00, 0x00]); // identification
    pkt.extend_from_slice(&[0x40, 0x00]); // flags/frag offset
    pkt.extend_from_slice(&[64]); // TTL
    pkt.extend_from_slice(&[6]); // protocol TCP
    pkt.extend_from_slice(&[0x00, 0x00]); // checksum (omitted)
    pkt.extend_from_slice(&[10, 0, 0, 1]); // src IP
    pkt.extend_from_slice(&[10, 0, 0, 2]); // dst IP
                                           // TCP header
    let src_port: u16 = 40000;
    let dst_port: u16 = 12083;
    pkt.extend_from_slice(&src_port.to_be_bytes());
    pkt.extend_from_slice(&dst_port.to_be_bytes());
    pkt.extend_from_slice(&seq.to_be_bytes());
    pkt.extend_from_slice(&0u32.to_be_bytes()); // ack
    pkt.extend_from_slice(&[0x50, flags]); // data offset=5
    pkt.extend_from_slice(&0xffffu16.to_be_bytes()); // window
    pkt.extend_from_slice(&[0x00, 0x00]); // checksum (omitted)
    pkt.extend_from_slice(&[0x00, 0x00]); // urgent ptr
    pkt.extend_from_slice(payload);
    pkt
}

/// Construct a classic PCAP from timestamped TCP records.
fn build_pcap(records: &[(&[u8], u32, u32, u8)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes()); // magic
    buf.extend_from_slice(&0x0002u16.to_le_bytes()); // version major
    buf.extend_from_slice(&0x0004u16.to_le_bytes()); // version minor
    buf.extend_from_slice(&0u32.to_le_bytes()); // thiszone
    buf.extend_from_slice(&0u32.to_le_bytes()); // sigfigs
    buf.extend_from_slice(&65535u32.to_le_bytes()); // snaplen
    buf.extend_from_slice(&1u32.to_le_bytes()); // network = Ethernet

    for (payload, seq, timestamp, flags) in records {
        let packet = build_packet(payload, *seq, *flags);
        let packet_len = packet.len() as u32;
        buf.extend_from_slice(&timestamp.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // microseconds
        buf.extend_from_slice(&packet_len.to_le_bytes());
        buf.extend_from_slice(&packet_len.to_le_bytes());
        buf.extend_from_slice(&packet);
    }
    buf
}

#[test]
fn pcap_roundtrip_matches_expected_output() {
    let delim = 0x01;
    let msg = build_fix_message(delim);
    let pcap_bytes = build_pcap(&[(&msg, 1, 0, 0x18)]);
    let expected_output = {
        let mut v = msg.clone();
        v.push(b'\n');
        v
    };

    let bin = assert_cmd::cargo::cargo_bin!("pcap2fix");
    Command::new(bin)
        .args(["--input", "-", "--port", "12083"])
        .write_stdin(pcap_bytes)
        .assert()
        .success()
        .stdout(expected_output);
}

#[test]
fn zero_idle_timeout_keeps_split_message_state() {
    let msg = build_fix_message(0x01);
    let split = 17;
    let pcap_bytes = build_pcap(&[
        (&msg[..split], 1, 1, 0x18),
        (&msg[split..], 1 + split as u32, 2, 0x18),
    ]);
    let mut expected = msg;
    expected.push(b'\n');

    let bin = assert_cmd::cargo::cargo_bin!("pcap2fix");
    Command::new(bin)
        .args(["--input", "-", "--port", "12083", "--idle-timeout", "0"])
        .write_stdin(pcap_bytes)
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn capture_time_gap_expires_reused_flow() {
    let first = build_fix_message(0x01);
    let second = build_fix_message_with_type(0x01, "1");
    let pcap_bytes = build_pcap(&[(&first, 100, 1, 0x18), (&second, 10, 3601, 0x18)]);
    let mut expected = first;
    expected.push(b'\n');
    expected.extend_from_slice(&second);
    expected.push(b'\n');

    let bin = assert_cmd::cargo::cargo_bin!("pcap2fix");
    Command::new(bin)
        .args(["--input", "-", "--port", "12083"])
        .write_stdin(pcap_bytes)
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn strict_mode_fails_on_incomplete_flow() {
    let msg = build_fix_message(0x01);
    let pcap_bytes = build_pcap(&[(&msg[..17], 1, 1, 0x18)]);

    let bin = assert_cmd::cargo::cargo_bin!("pcap2fix");
    Command::new(bin)
        .args(["--input", "-", "--port", "12083", "--strict"])
        .write_stdin(pcap_bytes)
        .assert()
        .failure()
        .stderr(predicates::str::contains("capture was incomplete"));
}

#[test]
fn fin_retires_and_reports_an_incomplete_flow() {
    let msg = build_fix_message(0x01);
    let partial = &msg[..17];
    let pcap_bytes = build_pcap(&[
        (partial, 1, 1, 0x18),
        (&[], 1 + partial.len() as u32, 2, 0x11),
    ]);

    let bin = assert_cmd::cargo::cargo_bin!("pcap2fix");
    Command::new(bin)
        .args(["--input", "-", "--port", "12083", "--strict"])
        .write_stdin(pcap_bytes)
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "FIN or RST closed an incomplete flow",
        ));
}
