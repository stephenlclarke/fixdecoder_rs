// SPDX-License-Identifier: AGPL-3.0-only
// Integration smoke tests for the CLI to ensure end-to-end flows keep working.

use assert_cmd::cargo::cargo_bin_cmd;
use chrono::NaiveDateTime;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::{NamedTempFile, tempdir};

const FILE_HEADER_RULE: &str = "----------------------------------------------";

fn fix_message(body: &str) -> String {
    let soh = '\u{0001}';
    format!("8=FIX.4.4{soh}9=005{soh}{body}10=000{soh}\n")
}

fn valid_fix_message(begin_string: &str, fields: &[(&str, &str)]) -> String {
    let soh = '\u{0001}';
    let body: String = fields
        .iter()
        .map(|(tag, value)| format!("{tag}={value}{soh}"))
        .collect();
    let mut msg = format!("8={begin_string}{soh}9={}{}{body}", body.len(), soh);
    let checksum = msg.bytes().fold(0u16, |acc, b| acc + b as u16) % 256;
    msg.push_str(&format!("10={checksum:03}{soh}\n"));
    msg
}

fn run_fixdecoder(args: &[&str]) -> String {
    let assert = cargo_bin_cmd!("fixdecoder").args(args).assert().success();
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn field_value<'a>(msg: &'a str, tag: &str) -> Option<&'a str> {
    msg.split('\u{0001}').find_map(|fragment| {
        let (lhs, rhs) = fragment.split_once('=')?;
        (lhs == tag).then_some(rhs)
    })
}

fn actual_body_length(msg: &str) -> usize {
    let bytes = msg.as_bytes();
    let len_pos = bytes
        .windows(2)
        .position(|window| window == b"9=")
        .expect("body length tag");
    let body_start = bytes
        .iter()
        .enumerate()
        .skip(len_pos)
        .find_map(|(idx, byte)| (*byte == 0x01).then_some(idx + 1))
        .expect("body start");
    let checksum_start = bytes
        .windows(4)
        .enumerate()
        .rfind(|(_, window)| *window == b"\x0110=")
        .map(|(idx, _)| idx)
        .expect("checksum start");
    checksum_start - body_start + 1
}

fn actual_checksum(msg: &str) -> i32 {
    let marker = "\u{0001}10=";
    let idx = msg.rfind(marker).expect("checksum marker");
    msg[..idx + 1].bytes().map(i32::from).sum::<i32>() % 256
}

fn assert_file_banner(stdout: &str, expected_filename: &str) {
    let mut lines = stdout.lines();
    let top_rule = lines.next().expect("file header top rule");
    assert!(
        top_rule.chars().all(|ch| ch == '-'),
        "top file header rule should contain only dashes: {stdout}"
    );
    assert!(
        top_rule.len() >= FILE_HEADER_RULE.len(),
        "top file header rule should be at least the documented width: {stdout}"
    );
    assert_eq!(
        lines.next().expect("filename line"),
        format!("Filename: {expected_filename}")
    );

    let modified_line = lines.next().expect("last modified line");
    let timestamp = modified_line
        .strip_prefix("Last Modified: ")
        .expect("last modified label");
    assert!(
        timestamp.ends_with('Z'),
        "last modified timestamp should be UTC/Zulu: {stdout}"
    );
    NaiveDateTime::parse_from_str(&timestamp[..timestamp.len() - 1], "%d/%m/%y %H:%M:%S%.3f")
        .expect("last modified timestamp should match dd/mm/yy HH:MM:SS.mmmZ");

    assert_eq!(
        lines.next().expect("bottom rule"),
        top_rule,
        "file header rules should match: {stdout}"
    );
    assert_eq!(
        lines.next().expect("blank spacer line"),
        "",
        "file header should be followed by a blank line: {stdout}"
    );
}

#[test]
fn decodes_single_message_from_stdin() {
    let msg = fix_message("35=0");
    let assert = cargo_bin_cmd!("fixdecoder")
        .arg("--fix=44")
        .write_stdin(msg)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        !stdout.starts_with("fixdecoder "),
        "normal decode output should not start with a version banner: {stdout}"
    );
    assert!(stdout.contains("BeginString"));
    assert!(stdout.contains("MsgType"));
}

#[test]
fn validation_reports_missing_fields() {
    let msg = fix_message(""); // missing MsgType intentionally
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--validate"])
        .write_stdin(msg)
        .assert()
        .success()
        .stdout(contains("Line 1:").and(contains("MsgType").and(contains("Missing"))));
}

#[test]
fn decodes_message_from_file_path() {
    let mut file = NamedTempFile::new().expect("temp file");
    let msg = fix_message("35=0");
    write!(file, "{msg}").expect("write temp");
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("BeginString"));
}

#[test]
fn secret_files_mode_writes_valid_obfuscated_sibling_file() {
    let dir = tempdir().expect("temp dir");
    let input = dir.path().join("orders.log");
    let msg = valid_fix_message(
        "FIX.4.4",
        &[("35", "A"), ("49", "BUY1"), ("56", "SELL1"), ("98", "0")],
    );
    let line = format!("INFO {} tail\n", msg.trim_end());
    fs::write(&input, &line).expect("write mixed log");

    cargo_bin_cmd!("fixdecoder")
        .arg("--secret-files")
        .arg(&input)
        .assert()
        .success()
        .stdout(contains("orders.secret.log"));

    let secret_path = Path::new(&input).with_file_name("orders.secret.log");
    let secret = fs::read_to_string(&secret_path).expect("read secret file");
    let original = fs::read_to_string(&input).expect("read original file");

    assert_eq!(original, line, "input file should remain unchanged");
    assert!(secret.starts_with("INFO 8=FIX.4.4"));
    assert!(secret.contains("49=SenderCompID0001"));
    assert!(secret.contains("56=TargetCompID0001"));
    assert!(secret.ends_with(" tail\n"));

    let start = secret.find("8=FIX.4.4").expect("find FIX start");
    let checksum_start = secret[start..]
        .find("\u{0001}10=")
        .map(|idx| start + idx + 1)
        .expect("find checksum");
    let checksum_end = secret[checksum_start..]
        .find('\u{0001}')
        .map(|idx| checksum_start + idx)
        .expect("find checksum end");
    let fix = &secret[start..=checksum_end];

    let declared_body_length = field_value(fix, "9")
        .and_then(|value| value.parse::<usize>().ok())
        .expect("declared body length");
    let declared_checksum = field_value(fix, "10")
        .and_then(|value| value.parse::<i32>().ok())
        .expect("declared checksum");

    assert_eq!(declared_body_length, actual_body_length(fix));
    assert_eq!(declared_checksum, actual_checksum(fix));
}

#[test]
fn file_decode_prints_separator_before_message_type_summary() {
    let mut file = NamedTempFile::new().expect("temp file");
    write!(file, "{}{}", fix_message("35=0"), fix_message("35=D")).expect("write temp");

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--style=plain"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let header_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Message Type"))
        .expect("message type summary header should be present");
    assert!(
        header_index > 0,
        "summary header should not be first line: {stdout}"
    );
    let separator = lines[header_index - 1].trim_start();
    assert!(
        separator.chars().all(|ch| ch == '-'),
        "message type summary should be preceded by a dashed separator: {stdout}"
    );
}

#[test]
fn nocounts_suppresses_message_type_summary() {
    let mut file = NamedTempFile::new().expect("temp file");
    write!(file, "{}{}", fix_message("35=0"), fix_message("35=D")).expect("write temp");

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--style=plain", "--nocounts"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        !stdout.contains("Message Counts:"),
        "--nocounts should suppress the message count summary: {stdout}"
    );
    assert!(
        stdout.contains("BeginString"),
        "--nocounts should not suppress decoded messages: {stdout}"
    );
}

#[test]
fn summary_nocounts_suppresses_message_type_summary() {
    let mut file = NamedTempFile::new().expect("temp file");
    let msg = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "8"),
            ("37", "O1"),
            ("11", "C1"),
            ("150", "0"),
            ("39", "0"),
        ],
    );
    write!(file, "{msg}").expect("write temp");

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--summary", "--paging=never", "--nocounts"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("Order Summary"),
        "--summary --nocounts should still render order summaries: {stdout}"
    );
    assert!(
        !stdout.contains("Message Counts:"),
        "--summary --nocounts should suppress the message count summary: {stdout}"
    );
}

#[test]
fn file_output_starts_with_the_file_name_even_without_header_style() {
    let mut file = NamedTempFile::new().expect("temp file");
    let msg = fix_message("35=0");
    write!(file, "{msg}").expect("write temp");
    let expected = Path::new(file.path())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap();

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--style=plain"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert_file_banner(&stdout, expected);
}

#[test]
fn summary_mode_outputs_order_summary() {
    let mut file = NamedTempFile::new().expect("temp file");
    let soh = '\u{0001}';
    let msg1 = format!("8=FIX.4.4{soh}9=005{soh}35=8{soh}37=O1{soh}11=C1{soh}10=000{soh}\n");
    let msg2 = format!("8=FIX.4.4{soh}9=005{soh}35=8{soh}37=O1{soh}11=C1{soh}10=000{soh}\n");
    write!(file, "{msg1}{msg2}").expect("write temp");
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--summary"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(
            contains("Order Summary").and(contains("Execution Report").or(contains("EXECUTION"))),
        );
}

#[test]
fn summary_mode_ignores_admin_messages() {
    let mut file = NamedTempFile::new().expect("temp file");
    let heartbeat = valid_fix_message(
        "FIX.4.4",
        &[("35", "0"), ("49", "SENDER"), ("56", "TARGET")],
    );
    let market_data = valid_fix_message(
        "FIX.4.4",
        &[("35", "W"), ("55", "EUR/USD"), ("268", "1"), ("269", "0")],
    );
    let exec_report = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "8"),
            ("37", "O1"),
            ("11", "C1"),
            ("150", "0"),
            ("39", "0"),
        ],
    );
    write!(file, "{heartbeat}{market_data}{exec_report}").expect("write temp");

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--summary"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("Execution Report") || stdout.contains("EXECUTION"),
        "summary should still include application traffic: {stdout}"
    );
    assert!(
        !stdout.contains("Heartbeat"),
        "summary should ignore FIX admin traffic: {stdout}"
    );
    assert!(
        !stdout.contains("UNKNOWN-"),
        "summary should ignore non-order application traffic too: {stdout}"
    );
}

#[test]
fn summary_mode_orders_events_and_collapses_duplicate_order_flow_messages() {
    let mut file = NamedTempFile::new().expect("temp file");
    let partial = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "8"),
            ("52", "20250519-15:07:20.036"),
            ("11", "C1"),
            ("41", "C1"),
            ("150", "F"),
            ("39", "1"),
            ("32", "1000000"),
            ("31", "19.391"),
            ("151", "2000000"),
            ("14", "1000000"),
            ("6", "19.391"),
        ],
    );
    let filled = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "8"),
            ("52", "20250519-15:07:20.106"),
            ("11", "C1"),
            ("41", "C1"),
            ("150", "F"),
            ("39", "2"),
            ("32", "2000000"),
            ("31", "19.391"),
            ("151", "0"),
            ("14", "3000000"),
            ("6", "19.391"),
        ],
    );
    let new_order = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "D"),
            ("52", "20250519-15:04:03.540"),
            ("60", "20250519-11:04:03.540"),
            ("11", "C1"),
            ("55", "USD/MXN"),
            ("54", "2"),
            ("38", "3000000"),
            ("44", "19.391000"),
            ("59", "5"),
        ],
    );
    let accepted = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "8"),
            ("52", "20250519-15:04:03.541"),
            ("11", "C1"),
            ("41", "C1"),
            ("150", "0"),
            ("39", "0"),
            ("151", "3000000"),
            ("14", "0"),
            ("6", "0"),
            ("58", "Accepted order"),
        ],
    );

    write!(
        file,
        "{partial}{filled}{new_order}{accepted}{partial}{filled}"
    )
    .expect("write temp");

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--summary"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("[New -> Partially Filled -> Filled]"),
        "summary header should be chronological and skip the synthetic unknown: {stdout}"
    );
    assert_eq!(
        stdout.matches("NEW_ORDER_SINGLE [C1]").count(),
        1,
        "duplicate order messages should collapse in the timeline: {stdout}"
    );
    assert_eq!(
        stdout.matches("35=D").count(),
        1,
        "duplicate raw FIX messages should collapse too: {stdout}"
    );
}

#[test]
fn summary_mode_highlights_invalid_order_messages_and_surfaces_reason() {
    let mut file = NamedTempFile::new().expect("temp file");
    let invalid_new = valid_fix_message(
        "FIX.4.4",
        &[
            ("49", "SenderCompID0002"),
            ("56", "TargetCompID0001"),
            ("34", "2146"),
            ("52", "20250519-15:07:21.162"),
            ("37", "1747588115027927786:000000004"),
            ("11", "12-193-1747602389"),
            ("41", "12-193-1747602389"),
            ("17", "1747588115027927786:0000000044"),
            ("150", "0"),
            ("39", "0"),
            ("64", "20250521"),
            ("55", "USD/MXN"),
            ("54", "1"),
            ("38", "3000000"),
            ("40", "2"),
            ("44", "19.38"),
            ("151", "3000000"),
            ("14", "0"),
            ("6", "0"),
            ("58", "Accepted order"),
        ],
    );
    let filled = valid_fix_message(
        "FIX.4.4",
        &[
            ("35", "8"),
            ("49", "SenderCompID0002"),
            ("56", "TargetCompID0001"),
            ("34", "2156"),
            ("52", "20250519-15:11:05.312"),
            ("37", "1747588115027927786:000000004"),
            ("11", "12-193-1747602389"),
            ("41", "12-193-1747602389"),
            ("17", "1747588115027927786:0000000047"),
            ("150", "F"),
            ("39", "2"),
            ("64", "20250521"),
            ("55", "USD/MXN"),
            ("54", "1"),
            ("38", "3000000"),
            ("40", "2"),
            ("44", "19.38"),
            ("32", "1000000"),
            ("31", "19.38"),
            ("151", "0"),
            ("14", "3000000"),
            ("6", "19.38"),
        ],
    );
    write!(file, "{invalid_new}{filled}").expect("write temp");

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--summary", "--colour=yes"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("Invalid: Missing required tag 35 (MsgType)"),
        "invalid summary rows should explain why the message failed validation: {stdout}"
    );
    assert!(
        stdout.contains(
            "Accepted order\u{001b}[0m | \u{001b}[31mInvalid: Missing required tag 35 (MsgType)\u{001b}[0m"
        ),
        "only the invalid timeline text should be highlighted in red: {stdout}"
    );
    assert!(
        stdout.contains("\u{001b}[31m-\u{001b}[0m ["),
        "the invalid timeline message cell should keep its original formatting: {stdout}"
    );
    assert!(
        stdout.contains("\u{001b}[31m8=FIX.4.4"),
        "invalid raw FIX messages should be highlighted in red: {stdout}"
    );
}

#[test]
fn override_is_honoured_with_fallback() {
    let soh = '\u{0001}';
    let msg = format!("8=FIXT.1.1{soh}9=005{soh}35=0{soh}1128=8{soh}10=000{soh}\n");
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44"])
        .write_stdin(msg)
        .assert()
        .success()
        .stdout(contains("ApplVerID"));
}

#[test]
fn version_flag_prints_only_version_information() {
    let assert = cargo_bin_cmd!("fixdecoder")
        .arg("--version")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.starts_with("fixdecoder "),
        "version output should begin with the version banner: {stdout}"
    );
    assert!(
        !stdout.contains("BeginString"),
        "version output should not decode messages: {stdout}"
    );
}

#[test]
fn default_args_env_applies_cli_flags() {
    let msg = fix_message("35=0");
    cargo_bin_cmd!("fixdecoder")
        .env("FIXDECODER_DEFAULT_ARGS", "--fix=44 --number")
        .write_stdin(msg)
        .assert()
        .success()
        .stdout(contains("     1 | "));
}

#[test]
fn explicit_cli_args_override_default_args_env() {
    let msg = fix_message("35=0");
    cargo_bin_cmd!("fixdecoder")
        .env("FIXDECODER_DEFAULT_ARGS", "--fix=45")
        .arg("--fix=44")
        .write_stdin(msg)
        .assert()
        .success()
        .stdout(contains("BeginString"));
}

#[test]
fn default_args_env_rejects_version_flag() {
    cargo_bin_cmd!("fixdecoder")
        .env("FIXDECODER_DEFAULT_ARGS", "--version")
        .assert()
        .failure()
        .stderr(contains("FIXDECODER_DEFAULT_ARGS").and(contains("--help or --version")));
}

#[test]
fn explicit_version_ignores_invalid_default_args_env() {
    cargo_bin_cmd!("fixdecoder")
        .env("FIXDECODER_DEFAULT_ARGS", "--definitely-not-a-real-flag")
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("fixdecoder "));
}

#[test]
fn explicit_help_ignores_invalid_default_args_env() {
    cargo_bin_cmd!("fixdecoder")
        .env("FIXDECODER_DEFAULT_ARGS", "--definitely-not-a-real-flag")
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: fixdecoder").and(contains("Command line option examples:")));
}

#[test]
fn plain_overrides_number_from_default_args_env() {
    let msg = fix_message("35=0");
    let assert = cargo_bin_cmd!("fixdecoder")
        .env("FIXDECODER_DEFAULT_ARGS", "--number")
        .arg("--plain")
        .write_stdin(msg)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        !stdout.contains("     1 | "),
        "--plain should suppress line numbers even when defaults enable them: {stdout}"
    );
}

#[test]
fn number_flag_prefixes_input_lines() {
    let msg = fix_message("35=0");
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--number"])
        .write_stdin(msg)
        .assert()
        .success()
        .stdout(contains("     1 | "));
}

#[test]
fn duplicate_fix_flags_are_rejected() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--fix=50", "--info"])
        .assert()
        .failure();
}

#[test]
fn explicit_header_style_renders_source_banner_for_files() {
    let mut file = NamedTempFile::new().expect("temp file");
    let msg = fix_message("35=0");
    write!(file, "{msg}").expect("write temp");
    let expected = Path::new(file.path())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap();

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--style=header"])
        .arg(file.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert_file_banner(&stdout, expected);
}

#[test]
fn info_flag_lists_available_dictionaries_and_highlights_selection() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--info"])
        .assert()
        .success()
        .stdout(
            contains("Available FIX Dictionaries")
                .and(contains("Loaded dictionaries"))
                .and(contains("FIX44")),
        );
}

#[test]
fn info_flag_marks_fix27_and_fix30_as_fix40_aliases() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=27", "--info"])
        .assert()
        .success()
        .stdout(contains("FIX27").and(contains("built-in alias of FIX40")));
}

#[test]
fn query_commands_normalise_fix_key_variants() {
    let fix40 = run_fixdecoder(&["--fix=40", "--message"]);
    let fix40_prefixed = run_fixdecoder(&["--fix=FIX40", "--message"]);
    let fix40_dotted = run_fixdecoder(&["--fix=4.0", "--message"]);
    let fix27_alias = run_fixdecoder(&["--fix=27", "--message"]);

    assert_eq!(fix40_prefixed, fix40);
    assert_eq!(fix40_dotted, fix40);
    assert_eq!(fix27_alias, fix40);
}

#[test]
fn message_listing_works_in_plain_and_column_modes() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--message"])
        .assert()
        .success()
        .stdout(
            contains("Session/Admin:")
                .and(contains("Business:"))
                .and(contains("Order Flow:"))
                .and(contains("Market Data:"))
                .and(contains("Heartbeat"))
                .and(contains("ExecutionReport")),
        );

    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--message", "--column"])
        .assert()
        .success()
        .stdout(
            contains("Session/Admin:")
                .and(contains("Business:"))
                .and(contains("Heartbeat"))
                .and(contains("Logon"))
                .and(contains("ExecutionReport")),
        );
}

#[test]
fn message_detail_accepts_msg_type_lookup() {
    cargo_bin_cmd!("fixdecoder")
        .args([
            "--fix=44",
            "--message",
            "0",
            "--verbose",
            "--header",
            "--trailer",
            "--column",
        ])
        .assert()
        .success()
        .stdout(
            contains("Message: ")
                .and(contains("Heartbeat"))
                .and(contains("Header"))
                .and(contains("Trailer"))
                .and(contains("HEARTBEAT")),
        );
}

#[test]
fn missing_message_is_reported() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--message", "NoSuchMessage"])
        .assert()
        .success()
        .stdout(contains("Message not found: NoSuchMessage"));
}

#[test]
fn tag_listing_works_in_plain_and_column_modes() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--tag"])
        .assert()
        .success()
        .stdout(contains("BeginString").and(contains("CheckSum")));

    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--tag", "--column"])
        .assert()
        .success()
        .stdout(contains("BeginString").and(contains("ClOrdID")));
}

#[test]
fn tag_detail_verbose_columns_show_enum_values() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--tag", "54", "--verbose", "--column"])
        .assert()
        .success()
        .stdout(contains("Side").and(contains("BUY")).and(contains("SELL")));
}

#[test]
fn invalid_and_missing_tags_are_reported() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--tag", "nope"])
        .assert()
        .failure()
        .stderr(contains("Invalid tag: nope"));

    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--tag", "999999"])
        .assert()
        .success()
        .stdout(contains("Tag not found: 999999"));
}

#[test]
fn component_listing_works_in_plain_and_column_modes() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--component"])
        .assert()
        .success()
        .stdout(contains("Header").and(contains("CommissionData")));

    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--component", "--column"])
        .assert()
        .success()
        .stdout(contains("Header").and(contains("Parties")));
}

#[test]
fn component_detail_verbose_columns_show_fields() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--component", "Header", "--verbose", "--column"])
        .assert()
        .success()
        .stdout(
            contains("Component: ")
                .and(contains("Header"))
                .and(contains("BeginString"))
                .and(contains("NEW_ORDER_SINGLE")),
        );
}

#[test]
fn missing_component_is_reported() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--component", "NoSuchComponent"])
        .assert()
        .success()
        .stdout(contains("Component not found: NoSuchComponent"));
}

#[test]
fn fixt_logon_default_appl_ver_applies_to_later_messages() {
    let mut xml = NamedTempFile::new().expect("temp xml");
    write!(
        xml,
        "<fix type='FIX' major='5' minor='0' servicepack='2'>
  <header/>
  <trailer/>
  <messages>
    <message name='Heartbeat' msgtype='0' msgcat='admin'>
      <field name='MsgType' required='Y'/>
      <field name='CustomSp2Tag' required='N'/>
    </message>
    <message name='Logon' msgtype='A' msgcat='admin'>
      <field name='MsgType' required='Y'/>
      <field name='DefaultApplVerID' required='N'/>
    </message>
  </messages>
  <components/>
  <fields>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='0' description='HEARTBEAT'/>
      <value enum='A' description='LOGON'/>
    </field>
    <field number='1137' name='DefaultApplVerID' type='STRING'>
      <value enum='9' description='FIX50SP2'/>
    </field>
    <field number='9001' name='CustomSp2Tag' type='CHAR'/>
  </fields>
</fix>"
    )
    .expect("write xml");

    let logon = valid_fix_message(
        "FIXT.1.1",
        &[
            ("35", "A"),
            ("49", "BUY"),
            ("56", "SELL"),
            ("34", "1"),
            ("52", "20240101-00:00:00"),
            ("98", "0"),
            ("108", "30"),
            ("1137", "9"),
        ],
    );
    let heartbeat = valid_fix_message(
        "FIXT.1.1",
        &[
            ("35", "0"),
            ("49", "BUY"),
            ("56", "SELL"),
            ("52", "20240101-00:00:01"),
            ("9001", "SP2ONLY"),
        ],
    );

    cargo_bin_cmd!("fixdecoder")
        .args(["--xml", &xml.path().display().to_string(), "--validate"])
        .write_stdin(format!("{logon}{heartbeat}"))
        .assert()
        .success()
        .stdout(contains("CustomSp2Tag").and(contains("Unknown tag 9001").not()));
}
