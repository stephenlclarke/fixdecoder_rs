// SPDX-License-Identifier: AGPL-3.0-only
// Integration smoke tests for the CLI to ensure end-to-end flows keep working.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::io::Write;
use tempfile::NamedTempFile;

fn fix_message(body: &str) -> String {
    let soh = '\u{0001}';
    format!("8=FIX.4.4{soh}9=005{soh}{body}10=000{soh}\n")
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
        .stdout(contains("Usage: fixdecoder"));
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
    let expected = format!("-- {} ", file.path().display());

    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--style=header"])
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains(expected));
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
fn message_listing_works_in_plain_and_column_modes() {
    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--message"])
        .assert()
        .success()
        .stdout(contains("Heartbeat").and(contains("ExecutionReport")));

    cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--message", "--column"])
        .assert()
        .success()
        .stdout(contains("Heartbeat").and(contains("Logon")));
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
