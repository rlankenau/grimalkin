//! Window skins: undecorated, optionally non-rectangular windows drawn from
//! bitmaps (docs/SKINS.md).
//!
//! * `parse`   the `skin.toml` schema
//! * `hit`     pure geometry: 9-slice mapping, layout, hit testing, shape rects
//! * `chrome`  the GPU pass that draws frame, buttons and title
//! * `win`     Windows only: `SetWindowRgn` from the shape mask, work area
//! * `winstate` normal / maximized / shaded as one state, and its geometry
//! * `winamp`   importer for WinAMP 2.x `.wsz` skins (docs/WINAMP_SKINS.md)
//!
//! This module loads skins (user directory first, then the built-ins that
//! `build.rs` embeds from `assets/skins/`) and holds [`SkinRuntime`], the
//! pointer state machine the app drives: hover and pressed button states,
//! titlebar double-click, and which window action a click means.

pub mod chrome;
pub mod drag;
pub mod hit;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod parse;
pub mod win;
pub mod winstate;
pub mod winamp;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::window::CursorIcon;

use self::hit::{ButtonKind, Hit, Layout, ResizeDir, SliceSpec, ALPHA_CUTOFF};
use self::parse::{DoubleClick, Insets, SkinDef, MAX_SHADE_HEIGHT, MAX_SKIN_DIM};

include!(concat!(env!("OUT_DIR"), "/builtin_skins.rs"));

/// The work area (screen minus taskbar / menu bar / Dock) of the window's
/// monitor, physical pixels. Falls back to the whole monitor, then to the
/// window itself.
pub fn work_area(window: &winit::window::Window) -> winstate::Rect {
    #[cfg(windows)]
    let native = win::work_area(window);
    #[cfg(target_os = "macos")]
    let native = mac::work_area(window);
    #[cfg(not(any(windows, target_os = "macos")))]
    let native: Option<(i32, i32, u32, u32)> = None;
    let (x, y, w, h) = native
        .or_else(|| {
            let m = window.current_monitor()?;
            let (p, s) = (m.position(), m.size());
            Some((p.x, p.y, s.width, s.height))
        })
        .unwrap_or_else(|| {
            let (p, s) = (window.outer_position().unwrap_or_default(), window.inner_size());
            (p.x, p.y, s.width, s.height)
        });
    winstate::Rect::new(x, y, w, h)
}

/// Two clicks on the titlebar within this time are a double-click.
/// Largest window dimension (physical pixels) the app ever asks the window
/// system for. Anything derived from a skin is clamped to this.
pub const MAX_WINDOW_PX: u32 = 16384;

/// Largest window coordinate (either sign) handed to the window system.
pub const MAX_WINDOW_POS: i32 = 65536;

/// A window size that is safe to hand to winit: each side in `1..=MAX_WINDOW_PX`.
pub fn clamp_size(w: u32, h: u32) -> winit::dpi::PhysicalSize<u32> {
    winit::dpi::PhysicalSize::new(w.clamp(1, MAX_WINDOW_PX), h.clamp(1, MAX_WINDOW_PX))
}

/// A window position that is safe to hand to winit.
pub fn clamp_pos(x: i32, y: i32) -> winit::dpi::PhysicalPosition<i32> {
    winit::dpi::PhysicalPosition::new(x.clamp(-MAX_WINDOW_POS, MAX_WINDOW_POS), y.clamp(-MAX_WINDOW_POS, MAX_WINDOW_POS))
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Does this `skin` config value ask for a skin at all?
pub fn is_active(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.eq_ignore_ascii_case("none")
}

/// Integer frame scale for a display scale factor: `max(1, round(f))`.
pub fn pixel_scale(factor: f64) -> u32 {
    if factor.is_finite() {
        (factor.round() as i64).clamp(1, 16) as u32
    } else {
        1
    }
}

/// Whether the swapchain must carry per-pixel alpha for the window shape.
/// Windows clips with a window region instead and keeps an opaque swapchain.
pub const fn needs_alpha_surface() -> bool {
    !cfg!(windows)
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// A decoded RGBA8 image.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn alpha(&self, x: u32, y: u32) -> u8 {
        if x >= self.w || y >= self.h {
            return 0;
        }
        self.rgba[(y as usize * self.w as usize + x as usize) * 4 + 3]
    }

    /// Image dimensions from the header alone (nothing is decoded).
    fn probe(bytes: &[u8], what: &str) -> Result<(u32, u32), String> {
        image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| format!("{what}: {e}"))?
            .into_dimensions()
            .map_err(|e| format!("{what}: {e}"))
    }

    /// Decode, but only after the header says the result fits: each side at
    /// most `MAX_SKIN_DIM`, at most `MAX_SKIN_IMAGE_PIXELS` pixels, and within
    /// what is left of the skin's `budget` of pixels (decoded RGBA is 4 bytes
    /// per pixel, and a tiny PNG can claim gigapixels). Decoding runs under
    /// `image::Limits` as well.
    fn decode(bytes: &[u8], what: &str, budget: &mut u64) -> Result<Image, String> {
        let (w, h) = Image::probe(bytes, what)?;
        Image::admit(w, h, what, budget)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_SKIN_DIM);
        limits.max_image_height = Some(MAX_SKIN_DIM);
        limits.max_alloc = Some(MAX_SKIN_IMAGE_PIXELS * 4 * 2);
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| format!("{what}: {e}"))?;
        reader.limits(limits);
        let img = reader.decode().map_err(|e| format!("{what}: {e}"))?.to_rgba8();
        Ok(Image {
            w: img.width(),
            h: img.height(),
            rgba: img.into_raw(),
        })
    }

    /// Check a `w` x `h` image against the per-image and total limits and
    /// charge it to `budget`.
    fn admit(w: u32, h: u32, what: &str, budget: &mut u64) -> Result<(), String> {
        let px = u64::from(w) * u64::from(h);
        if w == 0 || h == 0 || w > MAX_SKIN_DIM || h > MAX_SKIN_DIM {
            return Err(format!("{what}: {w}x{h} pixels; each side must be 1..={MAX_SKIN_DIM}"));
        }
        if px > MAX_SKIN_IMAGE_PIXELS {
            return Err(format!("{what}: {w}x{h} pixels is more than {MAX_SKIN_IMAGE_PIXELS} in one image"));
        }
        if px > *budget {
            return Err(format!("{what}: the skin's images together exceed {MAX_SKIN_TOTAL_PIXELS} pixels"));
        }
        *budget -= px;
        Ok(())
    }
}

/// Most pixels in one skin image (a 7680x4320 frame is 33 M).
pub const MAX_SKIN_IMAGE_PIXELS: u64 = 36 << 20;
/// Most pixels across all images of one skin.
pub const MAX_SKIN_TOTAL_PIXELS: u64 = 100 << 20;

/// Which texture a draw refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexId {
    Frame1,
    Frame2,
    Shade,
    Sheet,
}

enum Source {
    Dir(PathBuf),
    Builtin(SkinFiles),
}

impl Source {
    fn read(&self, file: &str) -> Option<Cow<'static, [u8]>> {
        match self {
            // Asset names come from skin.toml: never leave the skin directory
            // (an absolute path replaces the base in `join`), and never read
            // devices or FIFOs or more than a sane size.
            Source::Dir(d) => {
                if !crate::fsutil::is_contained_relative(file) {
                    log::warn!("skin file name {file:?} is not inside the skin directory; ignored");
                    return None;
                }
                crate::fsutil::read_regular(&d.join(file), crate::fsutil::MAX_SKIN_FILE_BYTES)
                    .ok()
                    .map(Cow::Owned)
            }
            Source::Builtin(files) => files.iter().find(|(n, _)| *n == file).map(|(_, b)| Cow::Borrowed(*b)),
        }
    }

    fn describe(&self, name: &str) -> String {
        match self {
            Source::Dir(d) => d.display().to_string(),
            Source::Builtin(_) => format!("built-in {name}"),
        }
    }
}

/// Names of the skins embedded in the binary.
pub fn builtin_names() -> Vec<&'static str> {
    BUILTIN_SKINS.iter().map(|(n, _)| *n).collect()
}

/// Every skin the user can pick, as `(config value, menu label)`, in groups
/// (no skin; built-ins; skins in the user directory; installed WinAMP
/// `.wsz`). Empty groups are left out. The internal `debug` skin is hidden.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))] // only the native menus list skins
pub fn choices(user_dir: Option<&Path>) -> Vec<Vec<(String, String)>> {
    let builtins = builtin_names()
        .into_iter()
        .filter(|n| *n != "debug")
        .map(|n| (n.to_string(), n.to_string()))
        .collect();
    let mut user: Vec<(String, String)> = Vec::new();
    let mut winamp: Vec<(String, String)> = Vec::new();
    if let Some(dir) = user_dir {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().join("skin.toml").is_file()
                    && !name.starts_with('.')
                    && !crate::textsafe::has_unsafe_chars(&name)
                {
                    user.push((name.clone(), name));
                }
            }
        }
        user.sort_by_key(|(n, _)| n.to_lowercase());
        winamp = winamp::installed(dir)
            .into_iter()
            .map(|(n, _)| (format!("winamp:{n}"), format!("WinAMP: {n}")))
            .collect();
    }
    let mut groups = vec![vec![("none".to_string(), "None (system window)".to_string())], builtins, user, winamp];
    groups.retain(|g| !g.is_empty());
    groups
}

/// Is the `skin` config value `current` the menu choice `value`?
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub fn same_choice(current: &str, value: &str) -> bool {
    let (c, v) = (current.trim(), value.trim());
    if !is_active(c) || !is_active(v) {
        return !is_active(c) && !is_active(v);
    }
    c.eq_ignore_ascii_case(v)
}

/// `<config_dir>/grimalkin/skins`.
pub fn user_skin_dir() -> Option<PathBuf> {
    crate::config::Config::default_path().and_then(|p| p.parent().map(|d| d.join("skins")))
}

pub struct Skin {
    pub name: String,
    pub def: SkinDef,
    pub frame1: Image,
    pub frame2: Option<Image>,
    pub shade_img: Option<Image>,
    pub sheet: Option<Image>,
    /// Where it was loaded from (for logs).
    pub origin: String,
    /// Colours (and font hint) the skin brings along; used while the skin is
    /// active unless the user chose a theme. Only imported skins have one.
    pub theme: Option<winamp::SkinTheme>,
}

impl Skin {
    /// Load the skin `name`: a path (contains a separator), else the user
    /// skin directory, else a built-in.
    pub fn load(name: &str) -> Result<Skin, String> {
        Skin::load_from(name, user_skin_dir().as_deref())
    }

    pub fn load_from(name: &str, user_dir: Option<&Path>) -> Result<Skin, String> {
        let name = name.trim();
        if let Some(r) = winamp::resolve(name, user_dir) {
            return r;
        }
        let source = if name.contains('/') || name.contains('\\') {
            Source::Dir(PathBuf::from(name))
        } else if name.is_empty() || name.starts_with('.') {
            return Err(format!("invalid skin name {name:?}"));
        } else if let Some(dir) = user_dir.map(|d| d.join(name)).filter(|d| d.join("skin.toml").is_file()) {
            Source::Dir(dir)
        } else if let Some((_, files)) = BUILTIN_SKINS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Source::Builtin(files)
        } else {
            return Err(format!("skin {name:?} not found (looked in the user skins directory and the built-ins)"));
        };
        Skin::from_source(name, &source)
    }

    fn from_source(name: &str, src: &Source) -> Result<Skin, String> {
        let toml = src.read("skin.toml").ok_or("skin.toml missing")?;
        let text = std::str::from_utf8(&toml).map_err(|_| "skin.toml is not UTF-8")?;
        let def = SkinDef::from_toml(text)?;
        let budget = std::cell::Cell::new(MAX_SKIN_TOTAL_PIXELS);
        let img = |file: &str| -> Result<Image, String> {
            let bytes = src.read(file).ok_or_else(|| format!("{file} missing"))?;
            let mut left = budget.get();
            let r = Image::decode(&bytes, file, &mut left);
            budget.set(left);
            r
        };
        let frame1 = img(&def.frame.image)?;
        def.validate(frame1.w, frame1.h)?;
        let frame2 = {
            let f2 = at_2x(&def.frame.image);
            match src.read(&f2) {
                // Only decode the @2x frame when its header says it is
                // exactly twice the size.
                Some(bytes) => match Image::probe(&bytes, &f2) {
                    Ok((w, h)) if w == frame1.w * 2 && h == frame1.h * 2 => {
                        let mut left = budget.get();
                        let r = Image::decode(&bytes, &f2, &mut left);
                        budget.set(left);
                        match r {
                            Ok(i) => Some(i),
                            Err(e) => {
                                log::warn!("skin {name}: {e}");
                                None
                            }
                        }
                    }
                    Ok(_) => {
                        log::warn!("skin {name}: {f2} is not exactly twice the size of {}; ignored", def.frame.image);
                        None
                    }
                    Err(e) => {
                        log::warn!("skin {name}: {e}");
                        None
                    }
                },
                None => None,
            }
        };
        let optional = |file: &Option<String>| -> Option<Image> {
            let file = file.as_ref()?;
            match img(file) {
                Ok(i) => Some(i),
                Err(e) => {
                    log::warn!("skin {name}: {e}");
                    None
                }
            }
        };
        let sheet = optional(&def.buttons.sheet);
        let shade_img = optional(&def.shade.as_ref().and_then(|s| s.image.clone()));
        let skin = Skin {
            name: if def.name.is_empty() { name.to_string() } else { def.name.clone() },
            origin: src.describe(name),
            def,
            frame1,
            frame2,
            shade_img,
            sheet,
            theme: None,
        };
        log::info!("loaded skin {:?} from {}", skin.name, skin.origin);
        Ok(skin)
    }

    pub fn image(&self, t: TexId) -> Option<&Image> {
        match t {
            TexId::Frame1 => Some(&self.frame1),
            TexId::Frame2 => self.frame2.as_ref(),
            TexId::Shade => self.shade_img.as_ref(),
            TexId::Sheet => self.sheet.as_ref(),
        }
    }

    /// Height in skin pixels of the shaded (windowshade) window.
    pub fn shade_height(&self) -> u32 {
        let d = &self.def;
        let h = d
            .shade
            .as_ref()
            .map(|s| s.height)
            .filter(|h| *h > 0)
            .or_else(|| d.titlebar.as_ref().map(|t| t.rect.h.max(0) as u32).filter(|h| *h > 0))
            .unwrap_or(d.frame.slices.top.max(8));
        h.min(MAX_SHADE_HEIGHT)
    }

    /// Shade bar height in physical pixels at `scale`, at least 1 and never
    /// more than [`MAX_WINDOW_PX`].
    pub fn shade_bar_px(&self, scale: u32) -> u32 {
        self.shade_height().saturating_mul(scale.max(1)).clamp(1, MAX_WINDOW_PX)
    }

    /// Is a window `height` physical pixels tall the windowshade bar? The
    /// chrome (frame spec, layout, grid freeze) is derived from the window
    /// size with this, never from a separate flag, so it cannot disagree with
    /// the window: a normal window is always taller than the bar (its minimum
    /// height is larger), and a window as thin as the bar can only show the bar.
    pub fn is_shade_height(&self, height: u32, scale: u32) -> bool {
        height <= self.shade_bar_px(scale)
    }

    /// How to stretch the frame (or the shade bar) over a window at `scale`:
    /// the slice spec, the texture and destination pixels per texel.
    pub fn frame_spec(&self, shaded: bool, scale: u32) -> (SliceSpec, TexId, i32) {
        let scale = scale.max(1);
        let c = self.def.frame.slices;
        let mode = self.def.frame.edge_mode;
        if shaded {
            let bar = Insets { left: c.left, right: c.right, top: 0, bottom: 0 };
            if let Some(s) = &self.shade_img {
                let spec = SliceSpec { region: [0, 0, s.w, s.h], caps: bar, mode };
                return (spec, TexId::Shade, scale as i32);
            }
            // No shade image: the top band of the frame, as high as the bar.
            let h = self.shade_height().min(self.frame1.h);
            let spec = SliceSpec { region: [0, 0, self.frame1.w, h], caps: bar, mode };
            return (spec, TexId::Frame1, scale as i32);
        }
        if scale == 2 {
            if let Some(f2) = &self.frame2 {
                let caps = Insets {
                    left: c.left.saturating_mul(2),
                    right: c.right.saturating_mul(2),
                    top: c.top.saturating_mul(2),
                    bottom: c.bottom.saturating_mul(2),
                };
                return (SliceSpec { region: [0, 0, f2.w, f2.h], caps, mode }, TexId::Frame2, 1);
            }
        }
        (
            SliceSpec { region: [0, 0, self.frame1.w, self.frame1.h], caps: c, mode },
            TexId::Frame1,
            scale as i32,
        )
    }

    /// (left, top) and (right, bottom) viewport insets in physical pixels.
    pub fn viewport_insets(&self, scale: u32) -> ((u32, u32), (u32, u32)) {
        let v = self.def.frame.viewport_inset;
        let s = scale.max(1);
        let m = |a: u32| a.saturating_mul(s).min(MAX_WINDOW_PX);
        ((m(v.left), m(v.top)), (m(v.right), m(v.bottom)))
    }

    /// Smallest window (physical pixels) that keeps the fixed frame parts
    /// intact around a 1x1 cell grid of the given size.
    pub fn min_window(&self, scale: u32, cell: (u32, u32)) -> (u32, u32) {
        let ((l, t), (r, b)) = self.viewport_insets(scale);
        let c = self.def.frame.slices;
        let s = scale.max(1);
        let cap = |a: u32, b: u32| a.saturating_add(b).saturating_mul(s).saturating_add(1);
        let w = l.saturating_add(r).saturating_add(cell.0.saturating_mul(4)).max(cap(c.left, c.right));
        let h = t.saturating_add(b).saturating_add(cell.1.saturating_mul(2)).max(cap(c.top, c.bottom));
        (w.min(MAX_WINDOW_PX), h.min(MAX_WINDOW_PX))
    }

    /// Is window pixel (x, y) part of the window shape?
    pub fn inside(&self, spec: &SliceSpec, tex: TexId, k: i32, win: (i32, i32), x: i32, y: i32) -> bool {
        let (tx, ty) = spec.texel_at(win.0, win.1, k, x, y);
        self.image(tex).is_some_and(|i| i.alpha(tx, ty) >= ALPHA_CUTOFF)
    }

    /// Rectangles covering the window shape (for Windows regions).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn shape_rects(&self, win: (u32, u32), scale: u32, shaded: bool) -> Vec<[i32; 4]> {
        let (spec, tex, k) = self.frame_spec(shaded, scale);
        let layout = Layout::new(&self.def, win.0, win.1, scale, shaded, false);
        let Some(img) = self.image(tex) else { return Vec::new() };
        hit::shape_rects(&spec, win.0 as i32, win.1 as i32, k, layout.viewport, |x, y| img.alpha(x, y))
    }
}

fn at_2x(file: &str) -> String {
    match file.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}@2x.{ext}"),
        None => format!("{file}@2x"),
    }
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// What a pointer press or release asks the window to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Close,
    Minimize,
    ToggleMaximize,
    ToggleShade,
    DragMove,
    DragResize(ResizeDir),
}

pub struct SkinRuntime {
    pub skin: Arc<Skin>,
    pub layout: Layout,
    win: (u32, u32),
    scale: u32,
    resizable: bool,
    spec: (SliceSpec, TexId, i32),
    hover: Hit,
    pressed: Option<ButtonKind>,
    last_click: Option<(Instant, f64, f64)>,
}

impl SkinRuntime {
    pub fn new(skin: Arc<Skin>, win: (u32, u32), scale: u32) -> SkinRuntime {
        let mut r = SkinRuntime {
            layout: Layout::new(&skin.def, win.0, win.1, scale, false, true),
            spec: skin.frame_spec(false, scale),
            skin,
            win,
            scale,
            resizable: true,
            hover: Hit::Outside,
            pressed: None,
            last_click: None,
        };
        r.relayout();
        r
    }

    fn relayout(&mut self) {
        let shaded = self.shaded();
        self.layout = Layout::new(&self.skin.def, self.win.0, self.win.1, self.scale, shaded, self.resizable);
        self.spec = self.skin.frame_spec(shaded, self.scale);
    }

    /// The window (or the display scale) changed.
    pub fn set_geometry(&mut self, win: (u32, u32), scale: u32) {
        self.win = win;
        self.scale = scale.max(1);
        self.relayout();
    }

    /// The window is the windowshade bar. Derived from the window size (see
    /// `Skin::is_shade_height`), so the chrome always matches the window.
    pub fn shaded(&self) -> bool {
        self.skin.is_shade_height(self.win.1, self.scale)
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Resizing is off while the window is maximized.
    pub fn set_resizable(&mut self, resizable: bool) {
        if self.resizable != resizable {
            self.resizable = resizable;
            self.relayout();
        }
    }

    pub fn hit_at(&self, x: f64, y: f64) -> Hit {
        let (spec, tex, k) = &self.spec;
        let win = (self.win.0 as i32, self.win.1 as i32);
        self.layout.hit(x, y, |px, py| self.skin.inside(spec, *tex, *k, win, px, py))
    }

    pub fn hover(&self) -> Hit {
        self.hover
    }

    /// Button currently highlighted by the pointer.
    pub fn hover_button(&self) -> Option<ButtonKind> {
        match self.hover {
            Hit::Button(k) => Some(k),
            _ => None,
        }
    }

    pub fn pressed_button(&self) -> Option<ButtonKind> {
        self.pressed
    }

    /// The pointer moved; returns true if the chrome needs a redraw.
    pub fn pointer_moved(&mut self, x: f64, y: f64) -> bool {
        let h = self.hit_at(x, y);
        let changed = h != self.hover && (matches!(h, Hit::Button(_)) || matches!(self.hover, Hit::Button(_)));
        self.hover = h;
        changed
    }

    /// Update the hover state for a pointer at (x, y) and return the icon the
    /// pointer should show (`None`: the viewport decides). Call whenever no
    /// chrome drag is in progress, so the icon always follows the hit test.
    pub fn hover_cursor(&mut self, x: f64, y: f64) -> (bool, Option<CursorIcon>) {
        let redraw = self.pointer_moved(x, y);
        (redraw, self.cursor_for(self.hover))
    }

    pub fn pointer_left(&mut self) -> bool {
        let was = matches!(self.hover, Hit::Button(_));
        self.hover = Hit::Outside;
        was
    }

    /// Left button pressed at (x, y) on a non-viewport spot.
    pub fn press(&mut self, x: f64, y: f64, now: Instant) -> Option<Action> {
        let h = self.hit_at(x, y);
        self.hover = h;
        match h {
            Hit::Button(k) => {
                self.pressed = Some(k);
                None
            }
            Hit::ResizeEdge(d) => Some(Action::DragResize(d)),
            Hit::Titlebar => {
                let dbl = self
                    .last_click
                    .is_some_and(|(t, lx, ly)| now.duration_since(t) <= DOUBLE_CLICK && (lx - x).abs() < 6.0 && (ly - y).abs() < 6.0);
                if dbl {
                    self.last_click = None;
                    let act = self.skin.def.titlebar.as_ref().map(|t| t.double_click).unwrap_or_default();
                    return match act {
                        DoubleClick::Shade => Some(Action::ToggleShade),
                        DoubleClick::Maximize => Some(Action::ToggleMaximize),
                        DoubleClick::None => Some(Action::DragMove),
                    };
                }
                self.last_click = Some((now, x, y));
                Some(Action::DragMove)
            }
            Hit::Chrome => Some(Action::DragMove),
            Hit::Viewport | Hit::Outside => None,
        }
    }

    /// Left button released; fires the pressed button if still over it.
    pub fn release(&mut self, x: f64, y: f64) -> Option<Action> {
        let k = self.pressed.take()?;
        if self.hit_at(x, y) != Hit::Button(k) {
            return None;
        }
        Some(match k {
            ButtonKind::Close => Action::Close,
            ButtonKind::Minimize => Action::Minimize,
            ButtonKind::Maximize => Action::ToggleMaximize,
            ButtonKind::Shade => Action::ToggleShade,
        })
    }

    /// Pointer shape for what is under it.
    pub fn cursor_for(&self, hit: Hit) -> Option<CursorIcon> {
        Some(match hit {
            Hit::Button(_) => CursorIcon::Pointer,
            Hit::ResizeEdge(d) => match d {
                ResizeDir::N => CursorIcon::NResize,
                ResizeDir::S => CursorIcon::SResize,
                ResizeDir::E => CursorIcon::EResize,
                ResizeDir::W => CursorIcon::WResize,
                ResizeDir::NE => CursorIcon::NeResize,
                ResizeDir::NW => CursorIcon::NwResize,
                ResizeDir::SE => CursorIcon::SeResize,
                ResizeDir::SW => CursorIcon::SwResize,
            },
            Hit::Titlebar | Hit::Chrome | Hit::Outside => CursorIcon::Default,
            Hit::Viewport => return None,
        })
    }

    pub fn view<'a>(&self, title: &'a str) -> chrome::ChromeView<'a> {
        chrome::ChromeView {
            title,
            hover: self.hover_button(),
            pressed: self.pressed,
        }
    }

    /// Push the window shape to the OS (Windows regions; a no-op elsewhere).
    pub fn apply_shape(&self, window: &winit::window::Window) {
        #[cfg(windows)]
        {
            let rects = self.skin.shape_rects(self.win, self.scale, self.shaded());
            win::set_window_region(window, &rects, self.win.0 as i32, self.win.1 as i32);
        }
        #[cfg(not(windows))]
        let _ = window;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `skin.toml` plus a 64x64 frame.png into a fresh temp dir and load it.
    fn load_toml(tag: &str, toml: &str) -> Result<Skin, String> {
        let dir = std::env::temp_dir().join(format!("grimalkin-hostile-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 20, 30, 255])).save(dir.join("frame.png")).unwrap();
        std::fs::write(dir.join("skin.toml"), toml).unwrap();
        let r = Skin::load_from(dir.to_str().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    /// A real, tiny PNG whose header claims `w` x `h` gray pixels (the data is
    /// all zero, so it compresses to a few hundred KB at most).
    fn png_claiming(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            use image::ImageEncoder;
            let enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::NoFilter,
            );
            let row = vec![0u8; w as usize];
            let all: Vec<u8> = (0..h).flat_map(|_| row.iter().copied()).collect();
            enc.write_image(&all, w, h, image::ExtendedColorType::L8).unwrap();
        }
        out
    }

    #[test]
    fn oversized_images_are_rejected_from_the_header_before_decoding() {
        // F28: a 261 KB frame.png cost 1.3 GB of RSS before validation.
        let mut budget = MAX_SKIN_TOTAL_PIXELS;
        // Too wide.
        let e = Image::decode(&png_claiming(MAX_SKIN_DIM + 1, 1), "wide.png", &mut budget).err().expect("rejected");
        assert!(e.contains("wide.png"), "{e}");
        // Too many pixels in one image, though each side is legal.
        let e = Image::decode(&png_claiming(MAX_SKIN_DIM, 2400), "big.png", &mut budget).err().expect("rejected");
        assert!(e.contains("big.png") && e.contains("pixels"), "{e}");
        assert_eq!(budget, MAX_SKIN_TOTAL_PIXELS, "a refused image costs nothing");
        // A normal one is charged to the skin's total.
        let ok = Image::decode(&png_claiming(100, 50), "ok.png", &mut budget).expect("decodes");
        assert_eq!((ok.w, ok.h), (100, 50));
        assert_eq!(budget, MAX_SKIN_TOTAL_PIXELS - 5000);
        // The total runs out.
        let mut small = 1000;
        assert!(Image::decode(&png_claiming(100, 50), "again.png", &mut small).is_err());
    }

    #[test]
    fn skin_asset_paths_cannot_leave_the_skin_directory() {
        // F27: image = "/dev/zero" (join replaces the base) exhausted memory.
        let toml = "[frame]\nimage = \"/dev/zero\"\nslices = { left = 8, right = 8, top = 8, bottom = 8 }\n";
        let e = load_toml("abs", toml).err().expect("must not load");
        assert!(e.contains("missing"), "{e}");
        let toml = "[frame]\nimage = \"../../etc/passwd\"\n";
        assert!(load_toml("dotdot", toml).is_err());
    }

    #[test]
    fn well_formed_skin_loads_from_a_directory() {
        let toml = "[frame]\nslices = { left = 8, right = 8, top = 8, bottom = 8 }\n\
                    viewport_inset = { left = 8, right = 8, top = 8, bottom = 8 }\n\
                    [titlebar]\nrect = { x = 0, y = 0, w = -1, h = 12 }\n\
                    [shade]\nheight = 12\n";
        let s = load_toml("ok", toml).expect("valid skin");
        assert_eq!(s.shade_height(), 12);
    }

    // The five hostile skin.toml variants from docs/qa/crash-triage-2026-10-28.md.
    // Each used to panic (debug) or reach AppKit with a wrapped size (release);
    // each must now be a clean load error that names the offending field.

    #[test]
    fn hostile_a_slices_u32_max_is_rejected() {
        let e = load_toml("a", "[frame]\nslices = { left = 4294967295 }\n").err().expect("must be rejected");
        assert!(e.contains("frame.slices"), "{e}");
        let e = load_toml("a2", "[frame]\nslices = { left = 4294967295, right = 4294967295, top = 4294967295, bottom = 4294967295 }\n")
            .err()
            .expect("must be rejected");
        assert!(e.contains("frame.slices"), "{e}");
    }

    #[test]
    fn hostile_b_viewport_inset_extremes_are_rejected() {
        for v in ["4294967295", "2147483647"] {
            for field in ["left", "right", "top", "bottom"] {
                let toml = format!("[frame]\nviewport_inset = {{ {field} = {v} }}\n");
                let e = load_toml("b", &toml).err().expect("must be rejected");
                assert!(e.contains("frame.viewport_inset"), "{field}={v}: {e}");
            }
        }
    }

    #[test]
    fn hostile_c_button_and_titlebar_rect_extremes_are_rejected() {
        let toml = "[buttons]\nclose = { rect = { x = 2147483647, y = 2147483647, w = 2147483647, h = 2147483647 }, sprite = [0, 0] }\n";
        let e = load_toml("c1", toml).err().expect("must be rejected");
        assert!(e.contains("buttons.close.rect"), "{e}");
        let toml = "[titlebar]\nrect = { x = 2000000000, y = 0, w = 2000000000, h = 10 }\n";
        let e = load_toml("c2", toml).err().expect("must be rejected");
        assert!(e.contains("titlebar.rect"), "{e}");
        let toml = "[buttons]\nclose = { rect = { x = -2147483648, y = 0, w = 10, h = 10 }, sprite = [0, 0] }\n";
        assert!(load_toml("c3", toml).is_err());
    }

    #[test]
    fn hostile_d_titlebar_text_extremes_are_rejected() {
        let toml = "[titlebar]\nrect = { x = 0, y = 0, w = -1, h = 10 }\n\
                    text = { x = 2147483647, y = 2147483647, w = -2147483648 }\n";
        let e = load_toml("d", toml).err().expect("must be rejected");
        assert!(e.contains("titlebar.text"), "{e}");
        let toml = "[titlebar]\ntext = { w = -2147483648 }\n";
        let e = load_toml("d2", toml).err().expect("must be rejected");
        assert!(e.contains("titlebar.text.w"), "{e}");
    }

    #[test]
    fn hostile_e_shade_height_extremes_are_rejected() {
        for v in ["4294967295", "2147483647", "100000", "4097", "65"] {
            let toml = format!("[shade]\nheight = {v}\n");
            let r = load_toml("e", &toml);
            // 65 is larger than the 64 pixel frame; the rest exceed the hard cap.
            let e = r.err().unwrap_or_else(|| panic!("shade.height {v} must be rejected"));
            assert!(e.contains("shade.height"), "{v}: {e}");
        }
        for v in ["-1", "-2147483648"] {
            assert!(load_toml("e2", &format!("[shade]\nheight = {v}\n")).is_err(), "{v}");
        }
    }

    #[test]
    fn nonsensical_but_in_range_values_are_rejected_too() {
        // Slices that leave no centre; sprite coordinates beyond any texture.
        assert!(load_toml("n1", "[frame]\nslices = { left = 32, right = 32 }\n").is_err());
        let toml = "[buttons]\nclose = { rect = { x = 0, y = 0, w = 4, h = 4 }, sprite = [4294967295, 0] }\n";
        assert!(load_toml("n2", toml).is_err());
        assert!(load_toml("n3", "[frame]\nviewport_inset = { left = 40, right = 40 }\n").is_err());
    }

    #[test]
    fn hostile_definitions_that_skip_validation_still_cannot_overflow() {
        // Defence in depth: the layout / bar arithmetic must not panic even
        // for a definition that never went through `validate`.
        let nasty = "[frame]\nslices = { left = 4294967295, right = 4294967295, top = 4294967295, bottom = 4294967295 }\n\
            viewport_inset = { left = 4294967295, right = 4294967295, top = 2147483647, bottom = 2147483647 }\n\
            [titlebar]\nrect = { x = 2147483647, y = 2147483647, w = 2147483647, h = 2147483647 }\n\
            text = { x = 2147483647, y = 2147483647, w = -2147483648, font_px = 1e30 }\n\
            [buttons]\nclose = { rect = { x = -2147483648, y = 2147483647, w = -2147483648, h = 2147483647 }, sprite = [4294967295, 4294967295] }\n\
            [resize]\ngrip = { x = 2147483647, y = -2147483648, w = 2147483647, h = -2147483648 }\nborder = 4294967295\n\
            [shade]\nheight = 4294967295\n";
        let def = SkinDef::from_toml(nasty).unwrap();
        for shaded in [false, true] {
            for scale in [1u32, 2, 16] {
                let l = Layout::new(&def, 800, 600, scale, shaded, true);
                let _ = l.hit(10.0, 10.0, |_, _| true);
            }
        }
        let _ = Layout::new(&def, u32::MAX, u32::MAX, u32::MAX, false, true);
        let mut skin = debug();
        skin.def = def;
        for scale in [1u32, 2, 16, u32::MAX] {
            assert!(skin.shade_bar_px(scale) <= MAX_WINDOW_PX);
            assert!(skin.shade_bar_px(scale) >= 1);
            let (w, h) = skin.min_window(scale, (u32::MAX, u32::MAX));
            assert!(w <= MAX_WINDOW_PX && h <= MAX_WINDOW_PX);
            let _ = skin.viewport_insets(scale);
            let _ = skin.is_shade_height(u32::MAX, scale);
            let _ = skin.frame_spec(false, scale);
            let _ = skin.frame_spec(true, scale);
        }
    }

    #[test]
    fn window_size_and_position_clamps() {
        let s = clamp_size(0, u32::MAX);
        assert_eq!((s.width, s.height), (1, MAX_WINDOW_PX));
        let s = clamp_size(800, 600);
        assert_eq!((s.width, s.height), (800, 600));
        let p = clamp_pos(i32::MIN, i32::MAX);
        assert_eq!((p.x, p.y), (-MAX_WINDOW_POS, MAX_WINDOW_POS));
        let p = clamp_pos(-30, 40);
        assert_eq!((p.x, p.y), (-30, 40));
    }

    fn debug() -> Skin {
        Skin::load_from("debug", None).expect("built-in debug skin")
    }

    #[test]
    fn debug_skin_is_embedded_and_valid() {
        assert!(builtin_names().contains(&"debug"));
        let s = debug();
        assert_eq!(s.name, "Debug");
        assert_eq!((s.frame1.w, s.frame1.h), (64, 64));
        assert!(s.sheet.is_some() && s.shade_img.is_some());
        assert_eq!(s.shade_height(), 12);
        assert_eq!(s.def.buttons.close.unwrap().rect.x, -14);
    }

    #[test]
    fn all_builtin_skins_load_and_are_consistent() {
        for want in ["debug", "llama", "grimalkin", "mint"] {
            assert!(builtin_names().contains(&want), "{want} is not embedded");
        }
        for name in builtin_names() {
            let s = Skin::load_from(name, None).unwrap_or_else(|e| panic!("skin {name}: {e}"));
            let f = &s.def.frame;
            assert!(s.frame1.w > 0 && s.frame1.h > 0, "{name}: empty frame");
            // The terminal viewport must be a positive area inside the frame.
            let (iw, ih) = (f.viewport_inset.left + f.viewport_inset.right, f.viewport_inset.top + f.viewport_inset.bottom);
            assert!(iw < s.frame1.w && ih < s.frame1.h, "{name}: viewport inset exceeds frame");
            assert!(s.sheet.is_some(), "{name}: buttons.png missing");
            assert!(s.shade_img.is_some(), "{name}: shade.png missing");
            // A runtime must build and lay out at a typical window size.
            let rt = SkinRuntime::new(Arc::new(s), (1000, 640), 1);
            assert!(rt.layout.viewport.w > 0 && rt.layout.viewport.h > 0, "{name}: empty viewport");
        }
    }

    #[test]
    fn unknown_and_bad_names_fail() {
        assert!(Skin::load_from("no-such-skin", None).is_err());
        assert!(Skin::load_from("", None).is_err());
        assert!(Skin::load_from("../etc", None).is_err());
        assert!(!is_active("none") && !is_active(" NONE ") && !is_active(""));
        assert!(is_active("llama"));
    }

    #[test]
    fn pixel_scale_rounds_and_clamps() {
        assert_eq!(pixel_scale(1.0), 1);
        assert_eq!(pixel_scale(1.25), 1);
        assert_eq!(pixel_scale(1.5), 2);
        assert_eq!(pixel_scale(2.0), 2);
        assert_eq!(pixel_scale(0.5), 1);
        assert_eq!(pixel_scale(f64::NAN), 1);
    }

    #[test]
    fn notched_corners_are_outside() {
        let s = Arc::new(debug());
        let rt = SkinRuntime::new(s, (300, 200), 1);
        assert_eq!(rt.hit_at(0.0, 0.0), Hit::Outside, "top-left notch");
        assert_eq!(rt.hit_at(299.0, 0.0), Hit::Outside, "top-right notch");
        assert_eq!(rt.hit_at(299.0, 199.0), Hit::Outside, "bottom-right notch");
        assert_eq!(rt.hit_at(150.0, 100.0), Hit::Viewport);
        assert!(matches!(rt.hit_at(150.0, 8.0), Hit::Titlebar | Hit::ResizeEdge(_)));
        assert_eq!(rt.hit_at(150.0, 8.0), Hit::Titlebar);
        // Same notch at 2x.
        let rt2 = SkinRuntime::new(Arc::new(debug()), (600, 400), 2);
        assert_eq!(rt2.hit_at(1.0, 1.0), Hit::Outside);
        assert_eq!(rt2.hit_at(598.0, 1.0), Hit::Outside);
    }

    #[test]
    fn shape_rects_exclude_the_notches() {
        let s = debug();
        let rects = s.shape_rects((300, 200), 1, false);
        let covered = |x: i32, y: i32| rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
        assert!(!covered(0, 0) && !covered(299, 0) && !covered(299, 199));
        assert!(covered(150, 100) && covered(150, 0) && covered(0, 100));
        // Shaded bar is its own shape.
        let bar = s.shape_rects((300, 12), 1, true);
        assert!(!bar.is_empty());
        assert!(bar.iter().all(|r| r[3] <= 12));
    }

    #[test]
    fn button_press_release_fires_only_over_the_button() {
        let mut rt = SkinRuntime::new(Arc::new(debug()), (300, 200), 1);
        let now = Instant::now();
        let (cx, cy) = (300.0 - 9.0, 8.0);
        assert_eq!(rt.press(cx, cy, now), None);
        assert_eq!(rt.pressed_button(), Some(ButtonKind::Close));
        assert_eq!(rt.release(cx, cy), Some(Action::Close));
        // Press, drag off, release: nothing.
        rt.press(cx, cy, now);
        assert_eq!(rt.release(100.0, 100.0), None);
        assert_eq!(rt.pressed_button(), None);
        // Hover changes request redraws only for buttons.
        rt.pointer_moved(100.0, 100.0);
        assert!(rt.pointer_moved(cx, cy));
        assert!(!rt.pointer_moved(cx + 1.0, cy));
        assert!(rt.pointer_moved(100.0, 100.0));
    }

    #[test]
    fn cursor_follows_the_hit_test_after_an_edge_drag() {
        let mut rt = SkinRuntime::new(Arc::new(debug()), (300, 200), 1);
        let t0 = Instant::now();
        // Hover, press and drag the west edge: the resize icon shows.
        assert_eq!(rt.hover_cursor(1.0, 100.0).1, Some(CursorIcon::WResize));
        assert_eq!(rt.press(1.0, 100.0, t0), Some(Action::DragResize(ResizeDir::W)));
        // The drag ends on the title bar: release, and the very next hover
        // there must show the plain icon, not the stale resize one.
        assert_eq!(rt.release(100.0, 8.0), None);
        assert_eq!(rt.hover_cursor(100.0, 8.0).1, Some(CursorIcon::Default));
        // And back over an edge, then the corner grip/zone, then the viewport.
        assert_eq!(rt.hover_cursor(1.0, 100.0).1, Some(CursorIcon::WResize));
        assert_eq!(rt.hover_cursor(150.0, 100.0).1, None);
    }

    #[test]
    fn titlebar_drag_and_double_click() {
        let mut rt = SkinRuntime::new(Arc::new(debug()), (300, 200), 1);
        let t0 = Instant::now();
        assert_eq!(rt.press(100.0, 8.0, t0), Some(Action::DragMove));
        assert_eq!(rt.press(101.0, 8.0, t0 + Duration::from_millis(200)), Some(Action::ToggleShade));
        // A third click starts over.
        assert_eq!(rt.press(101.0, 8.0, t0 + Duration::from_millis(300)), Some(Action::DragMove));
        // Too slow: drag again.
        assert_eq!(rt.press(101.0, 8.0, t0 + Duration::from_millis(900)), Some(Action::DragMove));
        // Edges resize.
        assert_eq!(rt.press(1.0, 100.0, t0), Some(Action::DragResize(ResizeDir::W)));
    }

    #[test]
    fn shaded_window_has_only_the_bar() {
        let mut rt = SkinRuntime::new(Arc::new(debug()), (300, 12), 1);
        assert!(rt.shaded());
        assert_eq!(rt.hit_at(150.0, 6.0), Hit::Titlebar);
        assert_eq!(rt.hit_at(0.0, 0.0), Hit::Outside, "bar corner notch");
        rt.set_geometry((300, 200), 1);
        assert!(!rt.shaded());
        assert_eq!(rt.hit_at(150.0, 100.0), Hit::Viewport);
    }

    #[test]
    fn builtin_skins_shade_and_unshade_from_the_bar() {
        for name in builtin_names() {
            for scale in [1u32, 2] {
                let skin = Arc::new(Skin::load_from(name, None).unwrap());
                let bar = skin.shade_height() * scale;
                let mut rt = SkinRuntime::new(skin.clone(), (900, 600), scale);
                // Click the shade button in the full window: the app shades.
                let b = rt.layout.button_rect(ButtonKind::Shade).unwrap_or_else(|| panic!("{name}: no shade button"));
                let (x, y) = (b.x as f64 + b.w as f64 / 2.0, b.y as f64 + b.h as f64 / 2.0);
                let t = Instant::now();
                assert_eq!(rt.press(x, y, t), None);
                assert_eq!(rt.release(x, y), Some(Action::ToggleShade), "{name}");
                // Shaded: the window is the bar, the shade button is still on it.
                rt.set_geometry((900, bar), scale);
                assert!(rt.shaded(), "{name}");
                let b = rt.layout.button_rect(ButtonKind::Shade).unwrap();
                assert!(b.y >= 0 && b.y + b.h <= bar as i32, "{name} @{scale}x: shade button off the bar: {b:?} bar {bar}");
                let (x, y) = (b.x as f64 + b.w as f64 / 2.0, b.y as f64 + b.h as f64 / 2.0);
                assert_eq!(rt.press(x, y, t), None);
                assert_eq!(rt.release(x, y), Some(Action::ToggleShade), "{name}: unshade from the bar");
                // Double-click on an empty stretch of the bar acts per the skin.
                let tb = rt.layout.titlebar.expect("titlebar");
                let mid = (tb.x as f64 + tb.w as f64 / 3.0, tb.y as f64 + tb.h as f64 / 2.0);
                assert_eq!(rt.hit_at(mid.0, mid.1), Hit::Titlebar, "{name}");
                let want = match skin.def.titlebar.as_ref().unwrap().double_click {
                    DoubleClick::Shade => Action::ToggleShade,
                    DoubleClick::Maximize => Action::ToggleMaximize,
                    DoubleClick::None => Action::DragMove,
                };
                assert_eq!(rt.press(mid.0, mid.1, t), Some(Action::DragMove));
                assert_eq!(rt.press(mid.0, mid.1, t + Duration::from_millis(100)), Some(want), "{name}");
                // No resize zones while shaded.
                assert!(!matches!(rt.hit_at(1.0, bar as f64 / 2.0), Hit::ResizeEdge(_)), "{name}");
            }
        }
    }

    #[test]
    fn min_window_keeps_corners() {
        let s = debug();
        let (w, h) = s.min_window(2, (8, 16));
        assert!(w >= 2 * (8 + 8) + 32 && h >= 2 * (16 + 8) + 32);
    }

    /// Regression: after maximize -> shade -> unshade the window was tall but
    /// the chrome still drew the shade bar, tiled down the whole window. The
    /// chrome is derived from the window height, whatever happened before.
    #[test]
    fn chrome_follows_the_window_height_not_the_history() {
        for name in builtin_names() {
            for scale in [1u32, 2] {
                let skin = Arc::new(Skin::load_from(name, None).unwrap());
                let bar = skin.shade_height() * scale;
                let mut rt = SkinRuntime::new(skin.clone(), (1016, 597), scale);
                // Every size a window goes through, in any order.
                let sizes = [(1016, 597), (2560, 1410), (2560, bar), (1016, bar), (1016, 597), (700, 400), (700, bar), (2560, 1410)];
                for (w, h) in sizes {
                    rt.set_geometry((w, h), scale);
                    let shaded = h <= bar;
                    assert_eq!(rt.shaded(), shaded, "{name} @{scale}x {w}x{h}");
                    assert_eq!(rt.layout.shaded, shaded, "{name} @{scale}x {w}x{h}");
                    assert_eq!(skin.is_shade_height(h, scale), shaded);
                    let tex = rt.spec.1;
                    assert_eq!(tex == TexId::Shade || (shaded && tex == TexId::Frame1), shaded, "{name}: frame spec at {w}x{h}");
                    // A tall window always has a viewport and no bar-only layout.
                    assert_eq!(rt.layout.viewport.w > 0, !shaded, "{name} {w}x{h}");
                }
            }
        }
    }

    fn bbox(rects: &[[i32; 4]]) -> [i32; 4] {
        rects.iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, r| {
            [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])]
        })
    }

    /// The Windows region covers the whole window where the art is opaque to
    /// the edge (llama), and is exactly one pixel short on the right and
    /// bottom where the art itself leaves a transparent last column and row
    /// (mint, whose baked shadow ends one pixel inside the frame image).
    #[test]
    fn shape_region_bbox_matches_the_art() {
        let llama = Skin::load_from("llama", None).unwrap();
        assert_eq!(bbox(&llama.shape_rects((1016, 597), 1, false)), [0, 0, 1016, 597]);
        let mint = Skin::load_from("mint", None).unwrap();
        let img = mint.image(TexId::Frame1).unwrap();
        let (w, h) = (img.w, img.h);
        assert!((0..h).all(|y| img.alpha(w - 1, y) < ALPHA_CUTOFF), "mint's last column is transparent art");
        assert!((0..w).all(|x| img.alpha(x, h - 1) < ALPHA_CUTOFF), "mint's last row is transparent art");
        assert_eq!(bbox(&mint.shape_rects((1008, 585), 1, false)), [0, 0, 1007, 584]);
    }
}
