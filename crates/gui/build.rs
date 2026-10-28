//! Embeds every file under `assets/skins/<name>/` into the binary.
//!
//! Generates `$OUT_DIR/builtin_skins.rs` with a table
//! `BUILTIN_SKINS: &[(&str, SkinFiles)]` (skin name -> [(file name, bytes)]),
//! so adding a skin directory is enough: no Rust edit is needed.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
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
