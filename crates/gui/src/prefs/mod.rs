//! The Preferences window: a second winit window drawn with egui.
//!
//! Every control writes straight into the user's `grimalkin.toml` (see
//! `model::EditQueue`); the terminal's existing config watcher then applies the
//! change live. The window never touches the terminal directly, apart from
//! theme and skin picks and the "open folder" buttons, which go through the
//! app's menu actions.
//!
//! GPU: egui-wgpu 0.35 is the newest egui that builds on Rust 1.92 and it
//! needs wgpu 29, while the terminal renderer uses wgpu 30. The two majors
//! coexist in one binary; this window has its own `wgpu` instance and device
//! (inside egui-wgpu's `Painter`) and shares nothing with the terminal.
//!
//! The window only redraws on its own events (input, config changes, jobs
//! finishing), so an idle Preferences window costs nothing.

mod model;
mod state;
mod ui;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_wgpu::{RendererOptions, WgpuConfiguration, winit::Painter};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::Window;

use crate::app::{SecondaryFlow, SecondaryWindow, UserEvent};
use crate::menu::MenuAction;
use state::Prefs;

pub const TITLE: &str = "grimalkin Preferences";
const SIZE: (f64, f64) = (560.0, 640.0);
const MIN_SIZE: (f64, f64) = (460.0, 420.0);

/// What the app tells the window when it opens it.
pub struct OpenArgs {
    /// The config file the terminal watches (`--config` or the default).
    pub config_path: Option<PathBuf>,
    pub proxy: EventLoopProxy<UserEvent>,
    /// The terminal window's outer position and size (physical px) to centre
    /// on, if known.
    pub anchor: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
}

pub struct PreferencesWindow {
    window: Arc<Window>,
    ctx: egui::Context,
    state: egui_winit::State,
    painter: Painter,
    prefs: Prefs,
    mods: ModifiersState,
    shown: bool,
    next_repaint: Option<Instant>,
}

impl PreferencesWindow {
    pub fn open(event_loop: &ActiveEventLoop, args: OpenArgs) -> Result<PreferencesWindow, String> {
        let mut attrs = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(LogicalSize::new(SIZE.0, SIZE.1))
            .with_min_inner_size(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1))
            .with_visible(false);
        if let Some(pos) = args.anchor.and_then(|(p, s)| centred(event_loop, p, s)) {
            attrs = attrs.with_position(pos);
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| format!("cannot create the Preferences window: {e}"))?);

        let ctx = egui::Context::default();
        style(&ctx);
        {
            let w = window.clone();
            ctx.set_request_repaint_callback(move |info| {
                // Zero delay: repaint now. Longer delays are polled by the app
                // through `next_repaint` after each frame.
                if info.delay.is_zero() {
                    w.request_redraw();
                }
            });
        }
        let state = egui_winit::State::new(ctx.clone(), ViewportId::ROOT, window.as_ref(), Some(window.scale_factor() as f32), window.theme(), None);

        let mut config = WgpuConfiguration::default();
        if let egui_wgpu::WgpuSetup::CreateNew(c) = &mut config.wgpu_setup {
            c.power_preference = egui_wgpu::wgpu::PowerPreference::LowPower;
        }
        let mut painter = pollster::block_on(Painter::new(ctx.clone(), config, false, RendererOptions::default()));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone()))).map_err(|e| format!("Preferences window: no GPU surface: {e}"))?;

        let proxy = args.proxy.clone();
        let dispatch = Box::new(move |a: MenuAction| {
            let _ = proxy.send_event(UserEvent::MenuAction(a));
        });
        let proxy = args.proxy.clone();
        let applied = Box::new(move || {
            let _ = proxy.send_event(UserEvent::ConfigChanged);
        });
        let w = window.clone();
        let repaint = Arc::new(move || w.request_redraw());
        let prefs = Prefs::new(args.config_path, dispatch, applied, repaint);

        let mut win = PreferencesWindow { window, ctx, state, painter, prefs, mods: ModifiersState::empty(), shown: false, next_repaint: None };
        win.redraw(); // first frame before the window appears: no white flash
        win.window.set_visible(true);
        win.window.focus_window();
        win.shown = true;
        Ok(win)
    }

    fn redraw(&mut self) {
        self.next_repaint = None;
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let now = Instant::now();
        let raw = self.state.take_egui_input(&self.window);
        let prefs = &mut self.prefs;
        let full = self.ctx.run_ui(raw, |ui| ui::show(ui, prefs));
        self.state.handle_platform_output(&self.window, full.platform_output);

        // A drag in progress holds writes back (they go out ~10 times a second
        // and once more on release).
        let settled = !self.ctx.input(|i| i.pointer.any_down());
        self.prefs.editing_text = self.ctx.egui_wants_keyboard_input();
        self.prefs.tick(now, settled);
        if let Some(t) = self.prefs.queue.due_at() {
            self.next_repaint = Some(t);
        }

        let ppp = full.pixels_per_point;
        let prims = self.ctx.tessellate(full.shapes, ppp);
        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.painter.on_window_resized(ViewportId::ROOT, w, h);
        }
        let bg = self.ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
        self.painter.paint_and_update_textures(ViewportId::ROOT, ppp, bg, &prims, &full.textures_delta, Vec::new(), &self.window);

        if let Some(v) = full.viewport_output.get(&ViewportId::ROOT) {
            if v.repaint_delay.is_zero() {
                self.window.request_redraw();
            } else if v.repaint_delay < Duration::from_secs(3600) {
                let t = now + v.repaint_delay;
                self.next_repaint = Some(self.next_repaint.map_or(t, |n| n.min(t)));
            }
        }
    }

    /// Escape and Cmd/Ctrl+W close the window, unless a text box or a popup
    /// needs the key first.
    fn closes(&self, event: &winit::event::KeyEvent) -> bool {
        if event.state != ElementState::Pressed || event.repeat {
            return false;
        }
        let w_mod = if cfg!(target_os = "macos") { self.mods.super_key() } else { self.mods.control_key() };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => !self.ctx.egui_wants_keyboard_input() && !self.ctx.any_popup_open(),
            Key::Character(c) => w_mod && c.eq_ignore_ascii_case("w"),
            _ => false,
        }
    }
}

/// Where to put a `SIZE` window so it is centred on the terminal window,
/// clamped to the monitor it is on.
fn centred(event_loop: &ActiveEventLoop, pos: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> Option<PhysicalPosition<i32>> {
    let monitor = event_loop.available_monitors().find(|m| {
        let (mp, ms) = (m.position(), m.size());
        pos.x >= mp.x && pos.y >= mp.y && pos.x < mp.x + ms.width as i32 && pos.y < mp.y + ms.height as i32
    });
    let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor());
    let (w, h) = ((SIZE.0 * scale) as i32, (SIZE.1 * scale) as i32);
    let mut x = pos.x + (size.width as i32 - w) / 2;
    let mut y = pos.y + (size.height as i32 - h) / 2;
    if let Some(m) = monitor {
        let (mp, ms) = (m.position(), m.size());
        x = x.clamp(mp.x, (mp.x + ms.width as i32 - w).max(mp.x));
        y = y.clamp(mp.y, (mp.y + ms.height as i32 - h).max(mp.y));
    }
    Some(PhysicalPosition::new(x, y))
}

/// Spacing and text sizes; colours stay egui's light/dark defaults, which
/// follow the system theme.
fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.slider_width = 140.0;
        s.spacing.combo_width = 232.0;
        s.spacing.window_margin = egui::Margin::same(12);
        for (style, size) in [(egui::TextStyle::Body, 14.0), (egui::TextStyle::Button, 14.0), (egui::TextStyle::Heading, 21.0), (egui::TextStyle::Small, 12.0)] {
            if let Some(f) = s.text_styles.get_mut(&style) {
                f.size = size;
            }
        }
    });
}

impl SecondaryWindow for PreferencesWindow {
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) -> SecondaryFlow {
        match &event {
            WindowEvent::CloseRequested => return SecondaryFlow::Close,
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event: key, is_synthetic: false, .. } if self.closes(key) => return SecondaryFlow::Close,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.window.request_redraw(),
            WindowEvent::Occluded(false) => self.window.request_redraw(),
            WindowEvent::RedrawRequested => {
                self.redraw();
                return SecondaryFlow::Keep;
            }
            _ => {}
        }
        let r = self.state.on_window_event(&self.window, &event);
        if r.repaint {
            self.window.request_redraw();
        }
        SecondaryFlow::Keep
    }

    fn request_redraw(&self) {
        self.window.request_redraw();
    }

    fn config_changed(&mut self) {
        self.prefs.config_changed();
        self.window.request_redraw();
    }

    fn next_repaint(&self) -> Option<Instant> {
        self.next_repaint
    }

    fn menu_action(&mut self, action: &MenuAction) -> bool {
        // The menu bar owns Cmd+C / Cmd+V, so a text box never sees those keys;
        // hand them to egui as events.
        match action {
            MenuAction::Copy => self.state.egui_input_mut().events.push(egui::Event::Copy),
            MenuAction::Paste => {
                if let Some(text) = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok()) {
                    self.state.egui_input_mut().events.push(egui::Event::Paste(text));
                }
            }
            MenuAction::ZoomIn | MenuAction::ZoomOut | MenuAction::ZoomReset => {}
            _ => return false,
        }
        self.window.request_redraw();
        true
    }

    fn window(&self) -> &Window {
        &self.window
    }
}
