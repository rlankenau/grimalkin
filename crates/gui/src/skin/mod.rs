//! Window skins: undecorated, optionally non-rectangular windows drawn from
//! bitmaps (docs/SKINS.md).
//!
//! * `parse`   the `skin.toml` schema
//! * `hit`     pure geometry: 9-slice mapping, layout, hit testing, shape rects
//! * `chrome`  the GPU pass that draws frame, buttons and title
//! * `win`     Windows only: `SetWindowRgn` from the shape mask
//!
//! This module loads skins (user directory first, then the built-ins that
//! `build.rs` embeds from `assets/skins/`) and holds [`SkinRuntime`], the
//! pointer state machine the app drives: hover and pressed button states,
//! titlebar double-click, and which window action a click means.

pub mod chrome;
pub mod drag;
pub mod hit;
pub mod parse;
pub mod win;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::window::CursorIcon;

use self::hit::{ButtonKind, Hit, Layout, ResizeDir, SliceSpec, ALPHA_CUTOFF};
use self::parse::{DoubleClick, Insets, SkinDef};

include!(concat!(env!("OUT_DIR"), "/builtin_skins.rs"));

/// Two clicks on the titlebar within this time are a double-click.
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

    fn decode(bytes: &[u8], what: &str) -> Result<Image, String> {
        let img = image::load_from_memory(bytes).map_err(|e| format!("{what}: {e}"))?.to_rgba8();
        Ok(Image {
            w: img.width(),
            h: img.height(),
            rgba: img.into_raw(),
        })
    }
}

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
            Source::Dir(d) => std::fs::read(d.join(file)).ok().map(Cow::Owned),
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
#[allow(dead_code)] // for tooling / docs
pub fn builtin_names() -> Vec<&'static str> {
    BUILTIN_SKINS.iter().map(|(n, _)| *n).collect()
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
}

impl Skin {
    /// Load the skin `name`: a path (contains a separator), else the user
    /// skin directory, else a built-in.
    pub fn load(name: &str) -> Result<Skin, String> {
        Skin::load_from(name, user_skin_dir().as_deref())
    }

    pub fn load_from(name: &str, user_dir: Option<&Path>) -> Result<Skin, String> {
        let name = name.trim();
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
        let img = |file: &str| -> Result<Image, String> {
            let bytes = src.read(file).ok_or_else(|| format!("{file} missing"))?;
            Image::decode(&bytes, file)
        };
        let frame1 = img(&def.frame.image)?;
        def.validate(frame1.w, frame1.h)?;
        let frame2 = {
            let f2 = at_2x(&def.frame.image);
            match src.read(&f2).map(|b| Image::decode(&b, &f2)) {
                Some(Ok(i)) if i.w == frame1.w * 2 && i.h == frame1.h * 2 => Some(i),
                Some(Ok(_)) => {
                    log::warn!("skin {name}: {f2} is not exactly twice the size of {}; ignored", def.frame.image);
                    None
                }
                Some(Err(e)) => {
                    log::warn!("skin {name}: {e}");
                    None
                }
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
        d.shade
            .as_ref()
            .map(|s| s.height)
            .filter(|h| *h > 0)
            .or_else(|| d.titlebar.as_ref().map(|t| t.rect.h.max(0) as u32).filter(|h| *h > 0))
            .unwrap_or(d.frame.slices.top.max(8))
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
                let caps = Insets { left: c.left * 2, right: c.right * 2, top: c.top * 2, bottom: c.bottom * 2 };
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
        ((v.left * s, v.top * s), (v.right * s, v.bottom * s))
    }

    /// Smallest window (physical pixels) that keeps the fixed frame parts
    /// intact around a 1x1 cell grid of the given size.
    pub fn min_window(&self, scale: u32, cell: (u32, u32)) -> (u32, u32) {
        let ((l, t), (r, b)) = self.viewport_insets(scale);
        let c = self.def.frame.slices;
        let s = scale.max(1);
        let w = (l + r + cell.0 * 4).max((c.left + c.right) * s + 1);
        let h = (t + b + cell.1 * 2).max((c.top + c.bottom) * s + 1);
        (w, h)
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
    shaded: bool,
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
            shaded: false,
            resizable: true,
            hover: Hit::Outside,
            pressed: None,
            last_click: None,
        };
        r.relayout();
        r
    }

    fn relayout(&mut self) {
        self.layout = Layout::new(&self.skin.def, self.win.0, self.win.1, self.scale, self.shaded, self.resizable);
        self.spec = self.skin.frame_spec(self.shaded, self.scale);
    }

    /// The window (or the display scale) changed.
    pub fn set_geometry(&mut self, win: (u32, u32), scale: u32) {
        self.win = win;
        self.scale = scale.max(1);
        self.relayout();
    }

    pub fn set_shaded(&mut self, shaded: bool) {
        self.shaded = shaded;
        self.relayout();
    }

    pub fn shaded(&self) -> bool {
        self.shaded
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
            shaded: self.shaded,
            hover: self.hover_button(),
            pressed: self.pressed,
        }
    }

    /// Push the window shape to the OS (Windows regions; a no-op elsewhere).
    pub fn apply_shape(&self, window: &winit::window::Window) {
        #[cfg(windows)]
        {
            let rects = self.skin.shape_rects(self.win, self.scale, self.shaded);
            win::set_window_region(window, &rects, self.win.0 as i32, self.win.1 as i32);
        }
        #[cfg(not(windows))]
        let _ = window;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        rt.set_shaded(true);
        assert_eq!(rt.hit_at(150.0, 6.0), Hit::Titlebar);
        assert_eq!(rt.hit_at(0.0, 0.0), Hit::Outside, "bar corner notch");
        rt.set_shaded(false);
        rt.set_geometry((300, 200), 1);
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
                rt.set_shaded(true);
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
}
