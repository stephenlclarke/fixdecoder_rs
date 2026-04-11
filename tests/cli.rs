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
