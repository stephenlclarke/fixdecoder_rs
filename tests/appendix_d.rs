// SPDX-License-Identifier: AGPL-3.0-only
// Smoke tests for the generated Appendix D sample corpus.

use assert_cmd::cargo::cargo_bin_cmd;
use std::fs;
use std::path::Path;

#[test]
fn generated_appendix_d_corpus_is_present_and_decodes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("examples")
        .join("appendix_d");

    let manifest = fs::read_to_string(root.join("manifest.json")).expect("read manifest");
    let file_count = manifest.matches("\"file\":").count();

    assert!(
        manifest.contains("\"file\": \"general/"),
        "manifest should include general Appendix D samples"
    );
    assert!(
        manifest.contains("\"file\": \"exchange/"),
        "manifest should include exchange Appendix D samples"
    );
    assert!(
        file_count >= 100,
        "expected a substantial generated corpus, found only {file_count} files"
    );

    let assert = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44"])
        .arg(root.join("all.fixlog"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("BeginString"),
        "generated Appendix D aggregate log should decode cleanly: {stdout}"
    );

    let validate = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44", "--validate"])
        .arg(root.join("all.fixlog"))
        .assert()
        .success();
    let validate_stdout = String::from_utf8_lossy(&validate.get_output().stdout);

    assert!(
        !validate_stdout.contains("Line ")
            && !validate_stdout.contains("Missing")
            && !validate_stdout.contains("BeginString"),
        "generated Appendix D aggregate log should be validation-clean: {validate_stdout}"
    );
}
