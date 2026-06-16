// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use std::process::Command;

// Capture build metadata (rustc version, git commit) at build time so the binary
// can report it in --version even outside CI.
fn main() {
    println!("cargo:rerun-if-changed=resources/icons/marvin.ico");
    println!("cargo:rerun-if-env-changed=FIXDECODER_BRANCH");
    println!("cargo:rerun-if-env-changed=FIXDECODER_COMMIT");
    println!("cargo:rerun-if-env-changed=FIXDECODER_VERSION");
    emit_git_rerun_directives();

    let rustc = rustc_version::version()
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=RUSTC_VERSION={rustc}");

    // Version/tag: prefer FIXDECODER_VERSION env, else git describe, else Cargo pkg version.
    let cargo_ver = env!("CARGO_PKG_VERSION").to_string();
    let mut version = std::env::var("FIXDECODER_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .filter(|v| {
            let stripped = v.trim_start_matches('v');
            stripped.strip_suffix("-dirty").unwrap_or(stripped) == cargo_ver
        })
        .map(|v| ensure_version_prefix(&v))
        .unwrap_or_else(|| format!("v{cargo_ver}"));
    if git_dirty() && !version.ends_with("-dirty") {
        version.push_str("-dirty");
    }
    println!("cargo:rustc-env=FIXDECODER_VERSION={version}");

    let commit = std::env::var("FIXDECODER_COMMIT")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| git_output(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "0000000".to_string());
    println!("cargo:rustc-env=FIXDECODER_COMMIT={commit}");

    let branch = std::env::var("FIXDECODER_BRANCH")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| git_output(&["rev-parse", "--abbrev-ref", "HEAD"]))
        .unwrap_or_else(|| "main".to_string());
    println!("cargo:rustc-env=FIXDECODER_BRANCH={branch}");

    // Surface the version being built so `cargo build` output includes our metadata.
    println!(
        "cargo:warning=Building fixdecoder {version} (branch:{branch}, commit:{commit}) [rust:{rustc}]"
    );

    embed_windows_icon("resources/icons/marvin.ico");
}

fn emit_git_rerun_directives() {
    if let Some(head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }

    if let Some(ref_name) = git_output(&["symbolic-ref", "-q", "HEAD"])
        && let Some(ref_path) = git_output(&["rev-parse", "--git-path", &ref_name])
    {
        println!("cargo:rerun-if-changed={ref_path}");
    }

    if let Some(packed_refs_path) = git_output(&["rev-parse", "--git-path", "packed-refs"]) {
        println!("cargo:rerun-if-changed={packed_refs_path}");
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            } else {
                None
            }
        })
}

fn git_dirty() -> bool {
    git_output(&["status", "--porcelain"]).is_some()
}

fn ensure_version_prefix(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn embed_windows_icon(icon_path: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon_path);

    if let Err(err) = resource.compile() {
        panic!("failed to compile Windows icon resource from {icon_path}: {err}");
    }
}
