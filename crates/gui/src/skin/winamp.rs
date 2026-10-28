//! WinAMP 2.x skin importer (docs/WINAMP_SKINS.md).
//!
//! A classic `.wsz` is a ZIP of bitmaps. Only the playlist editor artwork
//! (`pledit.bmp`, `pledit.txt`) is used: it is the one WinAMP window that
//! resizes, so it makes a good terminal frame. The importer cuts the sprites
//! out of `pledit.bmp`, composes the frame, windowshade bar and button sheet
//! in memory and returns the same [`Skin`] the TOML loader produces, so
//! rendering and hit testing are shared. `pledit.txt` becomes a theme
//! ([`SkinTheme`]) that applies while the skin is active.
//!
//! * `archive`  zip / directory access (case-insensitive, size-guarded)
//! * `compose`  sprite table, frame / shade / button composition
//! * `text`     `pledit.txt`, `viscolor.txt` and the derived theme
//! * `install`  the `--install-skin` / `--list-skins` command line

mod archive;
mod compose;
pub mod install;
mod text;

use std::path::{Path, PathBuf};

use self::archive::Archive;
pub use self::text::SkinTheme;
use super::Skin;

/// Prefix of a skin reference that names a WinAMP skin.
const PREFIX: &str = "winamp:";

/// Build a skin from the raw bytes of a `.wsz`. `font_ok` says whether the
/// font named in `pledit.txt` may become the terminal font hint; `None`
/// drops the hint.
pub fn from_wsz(
    bytes: Vec<u8>,
    name: &str,
    font_ok: Option<&dyn Fn(&str) -> bool>,
) -> Result<Skin, String> {
    let mut a = Archive::from_bytes(bytes)?;
    from_archive(&mut a, name, "wsz data".into(), font_ok)
}

fn from_archive(
    a: &mut Archive,
    name: &str,
    origin: String,
    font_ok: Option<&dyn Fn(&str) -> bool>,
) -> Result<Skin, String> {
    let bmp = a
        .read_named("pledit", &["bmp", "png"])?
        .ok_or("not a WinAMP skin: pledit.bmp not found")?;
    let pl = compose::decode(&bmp, "pledit.bmp")?;
    // The text files are optional and forgiving: a read problem means defaults.
    let pledit = match a.read_named("pledit", &["txt"]) {
        Ok(Some(b)) => text::parse_pledit(&b),
        Ok(None) => text::PlEdit::default(),
        Err(e) => {
            log::warn!("skin {name}: {e}; using default playlist colours");
            text::PlEdit::default()
        }
    };
    let vis = match a.read_named("viscolor", &["txt"]) {
        Ok(Some(b)) => text::parse_viscolor(&b),
        _ => text::default_viscolor(),
    };
    let parts = compose::compose(&pl, pledit.normal_bg)?;
    let def = compose::skin_def(name, pledit.normal_bg);
    def.validate(parts.frame.w, parts.frame.h)?;
    Ok(Skin {
        name: name.to_string(),
        def,
        frame1: parts.frame,
        frame2: None,
        shade_img: Some(parts.shade),
        sheet: Some(parts.sheet),
        origin,
        theme: Some(text::build_theme(name, &pledit, &vis, font_ok)),
    })
}

fn load_path(path: &Path) -> Result<Skin, String> {
    let name = path
        .file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "winamp".into());
    let mut a = Archive::open(path)?;
    let probe = |f: &str| crate::fonts::monospace_family_installed(f);
    let skin = from_archive(&mut a, &name, path.display().to_string(), Some(&probe))?;
    log::info!("loaded WinAMP skin {:?} from {}", skin.name, skin.origin);
    Ok(skin)
}

fn is_wsz(name: &str) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".wsz")
}

/// Installed `.wsz` skins in `dir`: (skin name, file), sorted by name.
pub fn installed(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if is_wsz(&file) && e.file_type().is_ok_and(|t| t.is_file()) {
                v.push((file[..file.len() - 4].to_string(), e.path()));
            }
        }
    }
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// Handle skin references that mean a WinAMP skin; `None` for everything
/// else (TOML skins and built-ins).
///
/// * `winamp:<name>`: `<user_dir>/<name>.wsz`, matched case-insensitively
/// * `winamp:<path>`, or a bare path to a `.wsz` file
/// * a bare path to an unpacked directory that has `pledit.bmp` (and no
///   `skin.toml`)
pub fn resolve(reference: &str, user_dir: Option<&Path>) -> Option<Result<Skin, String>> {
    let r = reference.trim();
    let head = r
        .get(..PREFIX.len())
        .filter(|h| h.eq_ignore_ascii_case(PREFIX));
    let explicit = head.is_some();
    let rest = if explicit {
        r[PREFIX.len()..].trim()
    } else {
        r
    };
    let path_like = rest.contains(['/', '\\']);

    if explicit && !path_like && !is_wsz(rest) {
        if rest.is_empty() || rest.starts_with('.') {
            return Some(Err(format!("invalid WinAMP skin name {rest:?}")));
        }
        let found = user_dir
            .map(installed)
            .unwrap_or_default()
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(rest));
        return Some(match found {
            Some((_, p)) => load_path(&p),
            None => Err(format!(
                "WinAMP skin {rest:?} not found in the user skins directory (install one with --install-skin)"
            )),
        });
    }
    let p = Path::new(rest);
    if explicit {
        return Some(load_path(p));
    }
    if is_wsz(rest) && p.is_file() {
        return Some(load_path(p));
    }
    if path_like
        && p.is_dir()
        && !p.join("skin.toml").exists()
        && Archive::open(p).is_ok_and(|a| a.find("pledit", &["bmp", "png"]).is_some())
    {
        return Some(load_path(p));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::archive::testutil::zip_of;
    use super::*;
    use crate::config::Rgb;
    use crate::skin::hit::{ButtonKind, Hit};
    use crate::skin::{Action, SkinRuntime};

    /// A 280 x 186 pledit.bmp whose sprite regions are flat colours, so the
    /// composition can be checked pixel by pixel.
    fn synthetic_pledit(height: u32) -> image::RgbImage {
        let mut im = image::RgbImage::from_pixel(280, height, image::Rgb([9, 9, 9]));
        let mut paint = |x: u32, y: u32, w: u32, h: u32, c: [u8; 3]| {
            for yy in y..y + h {
                for xx in x..x + w {
                    im.put_pixel(xx, yy, image::Rgb(c));
                }
            }
        };
        paint(0, 0, 25, 20, [200, 0, 0]); // top-left
        paint(127, 0, 25, 20, [0, 200, 0]); // top tile
        paint(153, 0, 25, 20, [0, 0, 200]); // top-right
        paint(26, 0, 100, 20, [250, 250, 0]); // title
        paint(0, 42, 12, 29, [100, 100, 100]); // left tile
        paint(31, 42, 20, 29, [50, 50, 50]); // right tile
        paint(0, 72, 125, 38, [255, 0, 255]); // bottom-left
        paint(179, 0, 25, 38, [0, 255, 255]); // bottom tile
        paint(126, 72, 150, 38, [255, 128, 0]); // bottom-right
        paint(52, 42, 9, 9, [1, 2, 3]); // close pressed
        paint(62, 42, 9, 9, [4, 5, 6]); // collapse pressed
        paint(150, 42, 9, 9, [7, 8, 9]); // expand pressed
        paint(72, 42, 25, 14, [30, 30, 30]); // shade left
        paint(72, 57, 25, 14, [31, 31, 31]); // shade tile
        paint(99, 42, 50, 14, [32, 32, 32]); // shade right
        im
    }

    fn bmp_bytes(im: &image::RgbImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        im.write_to(&mut out, image::ImageFormat::Bmp).unwrap();
        out.into_inner()
    }

    fn synthetic_wsz(txt: Option<&str>) -> Vec<u8> {
        let bmp = bmp_bytes(&synthetic_pledit(186));
        let mut entries: Vec<(&str, &[u8])> = vec![("Skin/PLEDIT.BMP", &bmp)];
        if let Some(t) = txt {
            entries.push(("skin/pledit.txt", t.as_bytes()));
        }
        zip_of(&entries)
    }

    fn px(img: &crate::skin::Image, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * img.w as usize + x as usize) * 4;
        img.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn composes_the_documented_frame() {
        let s = from_wsz(
            synthetic_wsz(Some("[Text]\nNormalBG=#102030\n")),
            "synth",
            None,
        )
        .unwrap();
        let f = &s.frame1;
        assert_eq!((f.w, f.h), (300, 87));
        assert_eq!(px(f, 0, 0), [200, 0, 0, 255], "top-left corner");
        assert_eq!(px(f, 60, 0), [0, 200, 0, 255], "top edge is the tile");
        assert_eq!(px(f, 290, 10), [0, 0, 200, 255], "top-right corner");
        assert_eq!(px(f, 5, 30), [100, 100, 100, 255], "left border");
        assert_eq!(px(f, 290, 30), [50, 50, 50, 255], "right border");
        assert_eq!(
            px(f, 60, 30),
            [0x10, 0x20, 0x30, 255],
            "area under the viewport is the list background"
        );
        assert_eq!(px(f, 10, 60), [255, 0, 255, 255], "bottom-left");
        assert_eq!(px(f, 130, 60), [0, 255, 255, 255], "bottom tile");
        assert_eq!(px(f, 200, 60), [255, 128, 0, 255], "bottom-right");
        assert!(f.rgba.chunks(4).all(|p| p[3] == 255), "the frame is opaque");
        // Shade bar and sheet.
        let sh = s.shade_img.as_ref().unwrap();
        assert_eq!((sh.w, sh.h), (300, 14));
        assert_eq!(px(sh, 3, 3), [30, 30, 30, 255]);
        assert_eq!(px(sh, 100, 3), [31, 31, 31, 255]);
        assert_eq!(px(sh, 295, 3), [32, 32, 32, 255]);
        let sheet = s.sheet.as_ref().unwrap();
        assert_eq!(px(sheet, 0, 18), [1, 2, 3, 255], "close pressed");
        assert_eq!(px(sheet, 9, 18), [4, 5, 6, 255], "shade pressed");
        assert_eq!(px(sheet, 18, 18), [7, 8, 9, 255], "expand pressed");
        assert_eq!(px(sheet, 50, 27 + 5), [250, 250, 0, 255], "title art");
    }

    #[test]
    fn engine_definition_is_consistent_and_clickable() {
        let s = std::sync::Arc::new(from_wsz(synthetic_wsz(None), "synth", None).unwrap());
        assert_eq!(s.name, "synth");
        assert_eq!(s.shade_height(), 14);
        let (w, h) = s.min_window(1, (8, 16));
        assert!(w >= 276 && h >= 59, "{w}x{h}");
        let mut rt = SkinRuntime::new(s.clone(), (700, 400), 1);
        // Viewport overlaps the 9-slice columns: 12 px left, 20 px right, 20 top, 38 bottom.
        assert_eq!(
            rt.layout.viewport,
            crate::skin::hit::PxRect {
                x: 12,
                y: 20,
                w: 668,
                h: 342
            }
        );
        assert_eq!(rt.hit_at(350.0, 200.0), Hit::Viewport);
        assert_eq!(rt.hit_at(100.0, 8.0), Hit::Titlebar);
        let close = rt.layout.button_rect(ButtonKind::Close).unwrap();
        assert_eq!((close.x, close.y, close.w, close.h), (700 - 11, 3, 9, 9));
        let shade = rt.layout.button_rect(ButtonKind::Shade).unwrap();
        assert_eq!((shade.x, shade.y), (700 - 21, 3));
        assert!(rt.layout.button_rect(ButtonKind::Minimize).is_none());
        let t = std::time::Instant::now();
        assert_eq!(rt.press(close.x as f64 + 4.0, 7.0, t), None);
        assert_eq!(rt.release(close.x as f64 + 4.0, 7.0), Some(Action::Close));
        assert_eq!(
            rt.hit_at(690.0, 390.0),
            Hit::ResizeEdge(crate::skin::hit::ResizeDir::SE)
        );
        // The shaded bar keeps both buttons on it.
        rt.set_geometry((700, 14), 1);
        rt.set_shaded(true);
        let b = rt.layout.button_rect(ButtonKind::Shade).unwrap();
        assert!(b.y >= 0 && b.y + b.h <= 14);
    }

    #[test]
    fn theme_comes_from_pledit_txt() {
        let s = from_wsz(
            synthetic_wsz(Some("[Text]\nNormal=#FF0000\nCurrent=#00FF00\nNormalBG=#000010\nSelectedBG=#404040\nFont=Zork\n")),
            "synth",
            Some(&|f: &str| f == "Zork"),
        )
        .unwrap();
        let t = s.theme.unwrap();
        assert_eq!(t.theme.colors.foreground, Some(Rgb(255, 0, 0)));
        assert_eq!(t.theme.colors.background, Some(Rgb(0, 0, 0x10)));
        assert_eq!(t.theme.colors.cursor, Some(Rgb(0, 255, 0)));
        assert_eq!(
            t.theme.colors.selection_background,
            Some(Rgb(0x40, 0x40, 0x40))
        );
        assert!(t.theme.font.family.is_some());
        assert_eq!(s.def.frame.viewport_background, Some(Rgb(0, 0, 0x10)));
    }

    #[test]
    fn not_a_skin_and_too_small_are_clear_errors() {
        let e = from_wsz(zip_of(&[("readme.txt", b"hi")]), "x", None)
            .err()
            .unwrap();
        assert!(e.contains("not a WinAMP skin"), "{e}");
        let small = bmp_bytes(&image::RgbImage::new(100, 50));
        let e = from_wsz(zip_of(&[("pledit.bmp", &small)]), "x", None)
            .err()
            .unwrap();
        assert!(e.contains("too small"), "{e}");
        let e = from_wsz(zip_of(&[("pledit.bmp", b"BMgarbage")]), "x", None)
            .err()
            .unwrap();
        assert!(e.contains("pledit.bmp"), "{e}");
        assert!(from_wsz(b"nope".to_vec(), "x", None).is_err());
    }

    #[test]
    fn png_is_sniffed_by_magic_even_under_a_bmp_name() {
        let im = synthetic_pledit(186);
        let mut png = Cursor::new(Vec::new());
        im.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let s = from_wsz(zip_of(&[("pledit.bmp", png.get_ref())]), "p", None).unwrap();
        assert_eq!(px(&s.frame1, 0, 0), [200, 0, 0, 255]);
        // A 190-row sheet (extra rows ignored) works too.
        let tall = bmp_bytes(&synthetic_pledit(190));
        assert!(from_wsz(zip_of(&[("x/Pledit.bmp", &tall)]), "t", None).is_ok());
    }

    #[test]
    fn thirty_two_bit_bmp_ignores_the_alpha_channel() {
        let mut rgba = image::RgbaImage::new(280, 186);
        for p in rgba.pixels_mut() {
            *p = image::Rgba([10, 20, 30, 0]);
        }
        let mut out = Cursor::new(Vec::new());
        rgba.write_to(&mut out, image::ImageFormat::Bmp).unwrap();
        let s = from_wsz(zip_of(&[("pledit.bmp", out.get_ref())]), "a", None).unwrap();
        assert_eq!(
            px(&s.frame1, 60, 30),
            [0, 0, 0, 255],
            "background fill is opaque"
        );
        assert!(s.frame1.rgba.chunks(4).all(|p| p[3] == 255));
        assert_eq!(px(&s.frame1, 0, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn resolve_finds_user_skins_paths_and_directories() {
        let d = std::env::temp_dir().join(format!("grimalkin-wa-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("raw")).unwrap();
        std::fs::write(d.join("My-Skin.WSZ"), synthetic_wsz(None)).unwrap();
        std::fs::write(
            d.join("raw").join("Pledit.bmp"),
            bmp_bytes(&synthetic_pledit(186)),
        )
        .unwrap();
        // By name, case-insensitively.
        let s = resolve("winamp:my-skin", Some(&d)).unwrap().unwrap();
        assert_eq!(s.name, "My-Skin");
        assert!(resolve("WinAmp:nope", Some(&d)).unwrap().is_err());
        assert!(resolve("winamp:../x", Some(&d)).unwrap().is_err());
        assert!(resolve("winamp:", Some(&d)).unwrap().is_err());
        // By path, with and without the prefix.
        let p = d.join("My-Skin.WSZ");
        assert!(resolve(p.to_str().unwrap(), None).unwrap().is_ok());
        assert!(resolve(&format!("winamp:{}", p.display()), None)
            .unwrap()
            .is_ok());
        // Unpacked directory.
        let dir = d.join("raw");
        assert!(resolve(dir.to_str().unwrap(), None).unwrap().is_ok());
        // Not ours.
        assert!(resolve("llama", Some(&d)).is_none());
        std::fs::create_dir_all(d.join("empty")).unwrap();
        assert!(
            resolve(d.join("empty").to_str().unwrap(), None).is_none(),
            "a directory without pledit is not a WinAMP skin"
        );
        assert_eq!(installed(&d).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn skin_load_dispatches_to_the_importer() {
        let d = std::env::temp_dir().join(format!("grimalkin-wa-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("zed.wsz"), synthetic_wsz(None)).unwrap();
        let s = Skin::load_from("winamp:zed", Some(&d)).unwrap();
        assert!(s.theme.is_some() && s.sheet.is_some());
        assert!(Skin::load_from("winamp:missing", Some(&d)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    // -- real skins (skipped when the samples are not on this machine) ------

    fn sample_dir() -> Option<PathBuf> {
        let d = std::env::var_os("GRIMALKIN_WSZ_SAMPLE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    "/private/tmp/claude-501/-Users-rlankenau-src-grimalkin/7e1104fd-b3d4-4b91-9f6e-a4c1266c8385/scratchpad/wsz",
                )
            });
        d.is_dir().then_some(d)
    }

    fn sample(name: &str) -> Option<Skin> {
        let p = sample_dir()?.join(name);
        if !p.is_file() {
            eprintln!("skipping: {} not present", p.display());
            return None;
        }
        Some(load_path(&p).unwrap_or_else(|e| panic!("{name}: {e}")))
    }

    #[test]
    fn base_skin_matches_the_documented_colours() {
        let Some(s) = sample("base-2.91.wsz") else {
            return;
        };
        assert_eq!((s.frame1.w, s.frame1.h), (300, 87));
        let c = s.theme.as_ref().unwrap().theme.colors.clone();
        assert_eq!(c.foreground, Some(Rgb(0, 0xff, 0)));
        assert_eq!(c.background, Some(Rgb(0, 0, 0)));
        assert_eq!(c.selection_background, Some(Rgb(0, 0, 0xc6)));
        assert_eq!(c.cursor, Some(Rgb(255, 255, 255)));
        // The title art and both baked buttons are not blank.
        let sheet = s.sheet.as_ref().unwrap();
        let lit = |x0: u32, y0: u32, w: u32, h: u32| {
            (y0..y0 + h)
                .any(|y| (x0..x0 + w).any(|x| px(sheet, x, y)[..3] != px(sheet, x0, y0)[..3]))
        };
        assert!(lit(0, 27, 100, 20), "title art has detail");
        assert!(lit(0, 0, 9, 9), "close button has detail");
        assert!(lit(9, 0, 9, 9), "shade button has detail");
    }

    #[test]
    fn subfolder_32_bit_190_row_skin_loads() {
        let Some(s) = sample("XMMS-Turquoise.wsz") else {
            return;
        };
        assert_eq!((s.frame1.w, s.frame1.h), (300, 87));
        assert!(s.frame1.rgba.chunks(4).all(|p| p[3] == 255));
        assert!(s.theme.is_some());
    }

    #[test]
    fn every_sample_skin_loads_and_lays_out() {
        let Some(dir) = sample_dir() else { return };
        let mut n = 0;
        for (name, path) in installed(&dir) {
            let s = load_path(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let rt = SkinRuntime::new(std::sync::Arc::new(s), (900, 600), 1);
            assert!(
                rt.layout.viewport.w > 0 && rt.layout.viewport.h > 0,
                "{name}"
            );
            n += 1;
        }
        eprintln!("{n} sample skins loaded");
    }

    /// Dump the composed images of every sample to `GRIMALKIN_WSZ_DUMP_DIR`
    /// (a debugging aid; does nothing unless the variable is set).
    #[test]
    fn dump_composed_images() {
        let (Some(out), Some(dir)) = (
            std::env::var_os("GRIMALKIN_WSZ_DUMP_DIR").map(PathBuf::from),
            sample_dir(),
        ) else {
            return;
        };
        std::fs::create_dir_all(&out).unwrap();
        for (name, path) in installed(&dir) {
            let s = load_path(&path).unwrap();
            for (tag, img) in [
                ("frame", Some(&s.frame1)),
                ("shade", s.shade_img.as_ref()),
                ("sheet", s.sheet.as_ref()),
            ] {
                let Some(img) = img else { continue };
                image::save_buffer(
                    out.join(format!("{name}-{tag}.png")),
                    &img.rgba,
                    img.w,
                    img.h,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
