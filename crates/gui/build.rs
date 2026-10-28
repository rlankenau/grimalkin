//! Embeds every file under `assets/skins/<name>/` into the binary.
//!
//! Generates `$OUT_DIR/builtin_skins.rs` with a table
//! `BUILTIN_SKINS: &[(&str, SkinFiles)]` (skin name -> [(file name, bytes)]),
//! so adding a skin directory is enough: no Rust edit is needed.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    embed_windows_resources();
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let skins = manifest.join("assets").join("skins");
    println!("cargo:rerun-if-changed=assets/skins");

    let mut out = String::from(
        "pub type SkinFiles = &'static [(&'static str, &'static [u8])];\npub static BUILTIN_SKINS: &[(&str, SkinFiles)] = &[\n",
    );
    let mut names: Vec<PathBuf> = match std::fs::read_dir(&skins) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    for dir in names {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        files.sort();
        if !files.iter().any(|f| f.file_name().is_some_and(|n| n == "skin.toml")) {
            continue;
        }
        println!("cargo:rerun-if-changed=assets/skins/{name}");
        let _ = writeln!(out, "    ({name:?}, &[");
        for f in files {
            let fname = f.file_name().unwrap().to_string_lossy().to_string();
            // `{:?}` escapes backslashes, so Windows paths are valid literals.
            let _ = writeln!(out, "        ({fname:?}, include_bytes!({:?})),", f.to_string_lossy());
        }
        out.push_str("    ]),\n");
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("builtin_skins.rs");
    std::fs::write(dest, out).expect("write builtin_skins.rs");
}

/// Windows only: embeds the app icon and version info (FileVersion,
/// ProductName, ...) into `grimalkin.exe` through `winresource`, which drives
/// the resource compiler (`rc.exe` from the Windows SDK on MSVC). The check is
/// on the *target* OS, not the host, so macOS/Linux builds never touch it.
fn embed_windows_resources() {
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
        .set("FileDescription", "Grimalkin terminal emulator")
        .set("InternalName", "grimalkin")
        .set("OriginalFilename", "grimalkin.exe")
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set("LegalCopyright", "Copyright (C) Grimalkin contributors. MIT OR Apache-2.0");
    if let Err(e) = res.compile() {
        // Fail loudly: a silently icon-less release exe is worse than a red build.
        panic!("embedding Windows resources failed: {e}");
    }
}
