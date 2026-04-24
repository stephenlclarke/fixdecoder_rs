// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

fn main() {
    println!("cargo:rerun-if-changed=../resources/icons/marvin.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../resources/icons/marvin.ico");

    if let Err(err) = resource.compile() {
        panic!("failed to compile Windows icon resource from ../resources/icons/marvin.ico: {err}");
    }
}
