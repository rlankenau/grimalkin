//! `grimalkin --install-skin <path-or-https-url>` and `--list-skins`.
//!
//! Installing validates the archive by running the importer on it, so a file
//! that would not load as a skin is never copied into the skins directory.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{from_wsz, installed};

/// Largest download we accept.
pub const MAX_DOWNLOAD: u64 = 20 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Where the Skin Museum keeps the raw archives.
const MUSEUM_RAW: &str = "https://r2.webampskins.org/skins";

/// Skin names are file stems: lower case `[a-z0-9._-]`, no leading dot.
pub fn sanitize_name(stem: &str) -> String {
    let s: String = stem
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches(|c| c == '.' || c == '-').to_string();
    if s.is_empty() {
        "skin".into()
    } else {
        s
    }
}

/// `https://skins.webamp.org/skin/<md5>/...` is an HTML page; the archive is
/// `https://r2.webampskins.org/skins/<md5>.wsz`. Other URLs are unchanged.
pub fn map_museum_url(url: &str) -> String {
    let rest = url
        .strip_prefix("https://skins.webamp.org/skin/")
        .or_else(|| url.strip_prefix("https://www.skins.webamp.org/skin/"));
    if let Some(rest) = rest {
        let md5 = rest.split(['/', '?', '#']).next().unwrap_or("");
        if md5.len() == 32 && md5.bytes().all(|b| b.is_ascii_hexdigit()) {
            return format!("{MUSEUM_RAW}/{}.wsz", md5.to_ascii_lowercase());
        }
    }
    url.to_string()
}

fn is_url(s: &str) -> bool {
    let l = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// The file stem a URL suggests (`.../Name.wsz?x=1` -> `Name`).
fn stem_of_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let tail = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("skin");
    let decoded = tail.replace("%20", " ");
    match decoded.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("wsz") || ext.eq_ignore_ascii_case("zip") => {
            stem.to_string()
        }
        _ => decoded,
    }
}

/// `https://skins.webamp.org/skin/<md5>/Some_Name.wsz/` -> `Some_Name`.
fn museum_name(page: &str) -> Option<String> {
    let rest = page.trim().strip_prefix("https://skins.webamp.org/skin/")?;
    let mut parts = rest
        .split(['?', '#'])
        .next()?
        .split('/')
        .filter(|p| !p.is_empty());
    parts.next()?; // the md5
    Some(stem_of_url(parts.next()?))
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    if !url.to_ascii_lowercase().starts_with("https://") {
        return Err("only https:// URLs are supported".into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .https_only(true)
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("download failed: {e}"))?;
    if bytes.len() as u64 > MAX_DOWNLOAD {
        return Err(format!("download is larger than {} MB", MAX_DOWNLOAD >> 20));
    }
    Ok(bytes)
}

/// Validate `bytes` as a skin and write it to `<dir>/<name>.wsz`.
/// Returns the skin name. Existing files are kept unless `force`.
pub fn install_bytes(
    bytes: Vec<u8>,
    stem: &str,
    dir: &Path,
    force: bool,
) -> Result<String, String> {
    if !bytes.starts_with(b"PK\x03\x04") {
        let html = bytes.iter().take(256).copied().collect::<Vec<u8>>();
        let hint = if String::from_utf8_lossy(&html)
            .to_ascii_lowercase()
            .contains("<html")
        {
            " (it is an HTML page; for the Skin Museum use the skin's page URL or its .wsz download link)"
        } else {
            ""
        };
        return Err(format!(
            "not a WinAMP skin: the data is not a zip archive{hint}"
        ));
    }
    let name = sanitize_name(stem);
    // Run the importer: pledit.bmp must be present and decode.
    from_wsz(bytes.clone(), &name, None)?;
    let target = dir.join(format!("{name}.wsz"));
    if target.exists() && !force {
        return Err(format!(
            "{} already exists (use --force to overwrite)",
            target.display()
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.wsz.part"));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot install {}: {e}", target.display())
    })?;
    Ok(name)
}

/// Install from a local file or an https URL into `dir`.
pub fn install_from(src: &str, dir: &Path, force: bool) -> Result<String, String> {
    if is_url(src) {
        let url = map_museum_url(src.trim());
        let bytes = download(&url)?;
        // A museum page URL may carry a readable name after the hash.
        let stem = museum_name(src).unwrap_or_else(|| stem_of_url(&url));
        return install_bytes(bytes, &stem, dir, force);
    }
    let path = Path::new(src);
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!(
            "{}: not a file (point --install-skin at a .wsz archive)",
            path.display()
        ));
    }
    if meta.len() > MAX_DOWNLOAD {
        return Err(format!(
            "{}: larger than {} MB",
            path.display(),
            MAX_DOWNLOAD >> 20
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    install_bytes(bytes, &stem, dir, force)
}

/// Run `--install-skin`; returns the process exit code.
pub fn run(src: &str, force: bool) -> i32 {
    let Some(dir) = crate::skin::user_skin_dir() else {
        eprintln!("grimalkin: cannot determine the config directory");
        return 1;
    };
    match install_from(src, &dir, force) {
        Ok(name) => {
            println!("Installed. Add to grimalkin.toml:  skin = \"winamp:{name}\"");
            0
        }
        Err(e) => {
            eprintln!("grimalkin: {e}");
            1
        }
    }
}

/// Text for `--list-skins`: the built-ins, then the user's skins.
pub fn list_text(user_dir: Option<&Path>) -> String {
    let mut s = String::new();
    for n in crate::skin::builtin_names() {
        s.push_str(&format!("{n}\n"));
    }
    if let Some(dir) = user_dir {
        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.path().join("skin.toml").is_file() {
                    dirs.push((e.file_name().to_string_lossy().into_owned(), e.path()));
                }
            }
        }
        dirs.sort();
        for (n, p) in dirs {
            s.push_str(&format!("{n}\t(user: {})\n", p.display()));
        }
        for (n, p) in installed(dir) {
            s.push_str(&format!("winamp:{n}\t(user: {})\n", p.display()));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::super::archive::testutil::zip_of;
    use super::*;

    fn good_wsz() -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbImage::new(280, 186)
            .write_to(&mut out, image::ImageFormat::Bmp)
            .unwrap();
        zip_of(&[("PLEDIT.BMP", out.get_ref())])
    }

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("grimalkin-wa-inst-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn names_are_sanitised() {
        assert_eq!(sanitize_name("Winamp3 Classified!"), "winamp3-classified");
        assert_eq!(sanitize_name("../../Evil Skin"), "evil-skin");
        assert_eq!(sanitize_name("base-2.91"), "base-2.91");
        assert_eq!(sanitize_name("..."), "skin");
        assert_eq!(sanitize_name("é"), "skin");
    }

    #[test]
    fn museum_pages_map_to_the_raw_archive() {
        let md5 = "aec27704d023c2e21990cfc8f2dfb517";
        let want = format!("https://r2.webampskins.org/skins/{md5}.wsz");
        assert_eq!(
            map_museum_url(&format!("https://skins.webamp.org/skin/{md5}/")),
            want
        );
        assert_eq!(
            map_museum_url(&format!(
                "https://skins.webamp.org/skin/{md5}/Winamp3_Classified.wsz/"
            )),
            want
        );
        assert_eq!(
            map_museum_url("https://example.com/a.wsz"),
            "https://example.com/a.wsz"
        );
        assert_eq!(
            map_museum_url("https://skins.webamp.org/skin/short/"),
            "https://skins.webamp.org/skin/short/"
        );
        assert_eq!(
            museum_name(&format!(
                "https://skins.webamp.org/skin/{md5}/Winamp3_Classified.wsz/"
            ))
            .as_deref(),
            Some("Winamp3_Classified")
        );
        assert_eq!(
            museum_name(&format!("https://skins.webamp.org/skin/{md5}/")),
            None
        );
        assert_eq!(stem_of_url("https://x.org/dl/My%20Skin.wsz?x=1"), "My Skin");
        assert_eq!(
            stem_of_url("https://r2.webampskins.org/skins/abc.wsz"),
            "abc"
        );
    }

    #[test]
    fn install_validates_copies_and_refuses_overwrite() {
        let d = tmp("ok");
        let src = d.join("in").join("Cool Skin.wsz");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, good_wsz()).unwrap();
        let skins = d.join("skins");
        let name = install_from(src.to_str().unwrap(), &skins, false).unwrap();
        assert_eq!(name, "cool-skin");
        assert!(skins.join("cool-skin.wsz").is_file());
        let again = install_from(src.to_str().unwrap(), &skins, false).unwrap_err();
        assert!(again.contains("--force"), "{again}");
        assert!(install_from(src.to_str().unwrap(), &skins, true).is_ok());
        // The listing shows it, and the skin reference resolves.
        assert!(list_text(Some(&skins)).contains("winamp:cool-skin"));
        assert!(crate::skin::Skin::load_from("winamp:cool-skin", Some(&skins)).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn install_rejects_html_non_skins_and_directories() {
        let d = tmp("bad");
        let skins = d.join("skins");
        std::fs::create_dir_all(&d).unwrap();
        let e = install_bytes(b"<!DOCTYPE html><html>".to_vec(), "x", &skins, false).unwrap_err();
        assert!(e.contains("HTML"), "{e}");
        let e = install_bytes(zip_of(&[("readme.txt", b"x")]), "x", &skins, false).unwrap_err();
        assert!(e.contains("pledit"), "{e}");
        assert!(
            !skins.exists() || installed(&skins).is_empty(),
            "nothing installed on failure"
        );
        assert!(install_from(d.to_str().unwrap(), &skins, false)
            .unwrap_err()
            .contains("not a file"));
        assert!(install_from("https://host.invalid/x.wsz", &skins, false).is_err());
        assert!(download("http://example.com/x.wsz")
            .unwrap_err()
            .contains("https"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn list_includes_builtins() {
        let t = list_text(None);
        assert!(t.lines().any(|l| l == "llama"));
    }
}
