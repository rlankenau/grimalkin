//! Windows only: embeds the app icon and version info into the console
//! launcher (`grimalkin.com`) so it shows the cat in Explorer and carries the
//! same product metadata as `grimalkin.exe`. See crates/gui/build.rs.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/grimalkin.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("..").join("..").join("assets").join("icon").join("grimalkin.ico");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy())
        .set("ProductName", "Grimalkin")
        .set("FileDescription", "Grimalkin console launcher")
        .set("InternalName", "grimalkin-launcher")
        .set("OriginalFilename", "grimalkin.com")
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set("LegalCopyright", "Copyright (C) Grimalkin contributors. MIT OR Apache-2.0");
    if let Err(e) = res.compile() {
        panic!("embedding Windows resources failed: {e}");
    }
}
