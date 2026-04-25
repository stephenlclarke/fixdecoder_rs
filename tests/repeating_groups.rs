// SPDX-License-Identifier: AGPL-3.0-only
// Smoke tests for the repeating-group sample corpus.

use assert_cmd::cargo::cargo_bin_cmd;
use std::fs;
use std::path::Path;

#[test]
fn generated_repeating_group_corpus_is_present_and_validation_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("examples")
        .join("repeating_groups");

    let manifest = fs::read_to_string(root.join("manifest.json")).expect("read manifest");
    let file_count = manifest.matches("\"file\":").count();

    assert!(
        file_count >= 4,
        "expected the repeating-group manifest to contain at least four files, found {file_count}"
    );
    for sample in [
        "new_order_single_parties.fix",
        "new_order_single_preallocs.fix",
        "allocation_instruction_orders.fix",
        "market_data_snapshot_full_refresh.fix",
    ] {
        assert!(
            manifest.contains(sample),
            "manifest should include {sample}: {manifest}"
        );
    }

    let decode = cargo_bin_cmd!("fixdecoder")
        .args(["--fix=44"])
        .arg(root.join("all.fixlog"))
        .assert()
        .success();
    let decode_stdout = String::from_utf8_lossy(&decode.get_output().stdout);
    assert!(
        decode_stdout.contains("BeginString"),
        "repeating-group corpus should decode cleanly: {decode_stdout}"
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
        "repeating-group corpus should be validation-clean: {validate_stdout}"
    );
}
