//! Application glue: owns the window, renderer, terminal, pty and config and
//! translates winit events into terminal actions.
//!
//! Event flow:
//! * The pty reader thread calls `on_wake`, which posts a coalesced
//!   `UserEvent::PtyWake`. The loop sleeps (`ControlFlow::Wait`) until then.
//! * `PtyWake`: drain pty events (bounded per wake), `Terminal::advance`, write
//!   `drain_responses` back, handle `TermEvent`s, request a redraw if damaged.
//! * `RedrawRequested` renders one frame (vsync paced by the Fifo present mode).
//! * Timers (cursor blink, the synchronized-output poll) use
//!   `ControlFlow::WaitUntil` from `about_to_wait`; nothing busy-loops.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grimalkin_pty::{Pty, PtyEvent, PtyWriter, SpawnConfig};
use grimalkin_vt::{CursorShape, Screen, TermEvent, Terminal};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ResizeDirection, UserAttentionType, Window, WindowId};

use crate::config::{Config, CursorShapeCfg};
use crate::config_edit::ConfigEditor;
use crate::graphics_glue::Graphics;
use crate::input::{self, EncodeState, KeyAction, KeyInput, MouseBtn, MouseEvent, MouseKind, ZoomOp};
use crate::links;
use crate::menu::{AppMenus, MenuAction};
use crate::reload::{self, ReloadCtx};
use crate::theme;
use crate::renderer::{FrameStatus, Renderer, ViewState};
use crate::selection::{CellPos, Selection};
use crate::skin::drag::ResizeDrag;
use crate::skin::hit::{Hit, ResizeDir};
use crate::skin::winstate::{Plan, Rect, WinState};
use crate::skin::{self, Action, Skin, SkinRuntime};
use crate::LaunchOptions;

/// Events delivered to the loop through its `EventLoopProxy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserEvent {
    /// The pty produced output or exited.
    PtyWake,
    /// Something outside the loop (another thread, a timer) wants a redraw.
    #[allow(dead_code)] // sent by future background work (image decoding, ...)
    Redraw,
    /// The config file or a user theme changed on disk (debounced).
    ConfigChanged,
    /// Step `n` of the `GRIMALKIN_DEBUG_SCRIPT` test script.
    Debug(usize),
    /// Cmd+, / Ctrl+, was pressed in the terminal window.
    OpenPreferences,
    /// A menu item (menu bar or context menu) was chosen.
    MenuAction(MenuAction),
}

/// What a secondary window wants after handling an event.
#[allow(dead_code)] // stage 2 (Preferences window) is the first implementer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryFlow {
    Keep,
    /// Drop the window (and so close it).
    Close,
}

/// A window besides the terminal: Preferences, About, ... The app routes its
/// `WindowEvent`s here by `WindowId`; the terminal window keeps its own path.
#[allow(dead_code)] // stage 2 (Preferences window) is the first implementer
pub trait SecondaryWindow {
    fn window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) -> SecondaryFlow;
    /// Ask the window to repaint (also used to re-read the config after it changed).
    fn request_redraw(&self);
    fn window(&self) -> &Window;
}

/// Open a file, folder or URL with the OS default handler.
fn open_in_os(target: impl AsRef<std::ffi::OsStr>) {
    let target = target.as_ref();
    if let Err(e) = open::that_detached(target) {
        log::error!("cannot open {}: {e}", target.to_string_lossy());
    }
}

/// Max bytes of pty output processed per wake so input and redraws stay
/// responsive during floods.
const PTY_BUDGET: usize = 1 << 20;
const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const SYNC_POLL: Duration = Duration::from_millis(16);
const FRAME_RETRY: Duration = Duration::from_millis(16);
const WHEEL_LINES_PER_NOTCH: f64 = 3.0;

pub struct App {
    cfg: Config,
    launch: LaunchOptions,
    proxy: EventLoopProxy<UserEvent>,
    wake_pending: Arc<AtomicBool>,
    running: Option<Running>,
    /// Windows other than the terminal, by id.
    secondary: HashMap<WindowId, Box<dyn SecondaryWindow>>,
    /// Menu bar / context menu (built once the loop runs; main thread only).
    menus: Option<AppMenus>,
    fatal: Option<String>,
    /// `GRIMALKIN_DEBUG_SCRIPT` commands (empty normally).
    debug_cmds: Vec<String>,
}

impl App {
    pub fn new(cfg: Config, launch: LaunchOptions, proxy: EventLoopProxy<UserEvent>) -> App {
        let debug_cmds = crate::debug_script::start(&proxy);
        App {
            cfg,
            launch,
            proxy,
            wake_pending: Arc::new(AtomicBool::new(false)),
            running: None,
            secondary: HashMap::new(),
            menus: None,
            fatal: None,
            debug_cmds,
        }
    }

    /// Show a window besides the terminal: its events are routed to it.
    #[allow(dead_code)] // stage 2 (Preferences window)
    pub fn add_secondary(&mut self, w: Box<dyn SecondaryWindow>) {
        self.secondary.insert(w.window().id(), w);
    }

    /// The config file the app edits and watches (`--config` or the default).
    fn config_path(&self) -> Option<PathBuf> {
        self.launch.config_path.clone().or_else(Config::default_path)
    }

    /// Run `f` on the user's config file and save it; the reload watcher
    /// applies the change to the running window.
    fn edit_config(&self, what: &str, f: impl FnOnce(&mut ConfigEditor) -> std::io::Result<()>) {
        let Some(path) = self.config_path() else {
            log::error!("{what}: no config file location");
            return;
        };
        let result = ConfigEditor::open_at(&path).and_then(|mut ed| {
            f(&mut ed)?;
            ed.save()
        });
        if let Err(e) = result {
            log::error!("{what}: cannot update {}: {e}", path.display());
        }
    }

    /// Write `key = value` to the config, unless it is already set that way
    /// (then only a reload is requested, so a `--theme` override that was just
    /// dropped still takes effect).
    fn set_config_string(&self, key: &str, value: &str) {
        self.edit_config(&format!("set {key}"), |ed| {
            if ed.get(key).and_then(|v| v.as_str()) == Some(value) {
                let _ = self.proxy.send_event(UserEvent::ConfigChanged);
                return Ok(());
            }
            ed.set(key, value)?;
            Ok(())
        });
    }

    fn open_preferences(&mut self) {
        log::info!("Preferences: not implemented yet (stage 2)");
    }

    fn menu_action(&mut self, action: MenuAction) {
        log::debug!("menu action {action:?}");
        match action {
            MenuAction::OpenPreferences => self.open_preferences(),
            MenuAction::SetTheme(name) => {
                // An explicit pick beats the `--theme` launch option.
                if let Some(r) = self.running.as_mut() {
                    r.reload.theme_override = None;
                }
                self.set_config_string("theme", &name);
            }
            MenuAction::SetSkin(name) => {
                if let Some(r) = self.running.as_mut() {
                    r.reload.skin_override = None;
                }
                self.set_config_string("skin", &name);
            }
            MenuAction::ZoomIn | MenuAction::ZoomOut | MenuAction::ZoomReset | MenuAction::Copy | MenuAction::Paste => {
                if let Some(r) = self.running.as_mut() {
                    match action {
                        MenuAction::ZoomIn => r.zoom(ZoomOp::In),
                        MenuAction::ZoomOut => r.zoom(ZoomOp::Out),
                        MenuAction::ZoomReset => r.zoom(ZoomOp::Reset),
                        MenuAction::Copy => r.copy(),
                        _ => r.paste(),
                    }
                }
            }
            MenuAction::OpenConfigFile => {
                // Creates the file from the example first if it is missing.
                let Some(path) = self.config_path() else { return };
                match ConfigEditor::open_at(&path) {
                    Ok(ed) => open_in_os(ed.path()),
                    Err(e) => log::error!("cannot open {}: {e}", path.display()),
                }
            }
            MenuAction::RevealThemesFolder => {
                let Some(dir) = theme::user_dir() else { return };
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    log::error!("cannot create {}: {e}", dir.display());
                    return;
                }
                open_in_os(&dir);
            }
            MenuAction::About => open_in_os(env!("CARGO_PKG_REPOSITORY")),
        }
    }

    /// Set if startup failed (GPU, pty, ...).
    pub fn fatal_error(&self) -> Option<&str> {
        self.fatal.as_deref()
    }

    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<Running, String> {
        let skin = if skin::is_active(&self.cfg.skin) {
            match Skin::load(&self.cfg.skin) {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    log::error!("cannot load skin {:?}: {e}", self.cfg.skin);
                    self.cfg.skin = "none".to_string();
                    None
                }
            }
        } else {
            None
        };
        // The theme's font/cursor settings are folded into the effective config.
        let resolved = theme::resolve_for_skin(&self.cfg, skin.as_deref());
        let palette = skin_palette(resolved.palette, &self.cfg, skin.as_deref());
        let cfg = &resolved.config;
        let mut attrs = Window::default_attributes()
            .with_title(cfg.window.title.as_str())
            .with_inner_size(LogicalSize::new(900.0, 600.0))
            .with_visible(false);
        if cfg.window.opacity < 1.0 {
            attrs = attrs.with_transparent(true);
        }
        if skin.is_some() {
            // The skin draws its own frame; the OS draws nothing. Windows
            // clips to the shape with a region and keeps an opaque swapchain.
            attrs = attrs.with_decorations(false).with_transparent(skin::needs_alpha_surface());
            // No OS maximize (Aero Snap, Win+Up, the system menu): the skin
            // maximizes by hand, and an OS zoom would leave the window
            // rectangle 8 px larger than the client area the chrome and the
            // shape are laid out for.
            #[cfg(windows)]
            {
                use winit::window::WindowButtons;
                attrs = attrs.with_enabled_buttons(WindowButtons::CLOSE | WindowButtons::MINIMIZE);
            }
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::WindowAttributesExtMacOS;
                attrs = attrs.with_has_shadow(false);
            }
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| format!("cannot create window: {e}"))?);
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
            window.set_option_as_alt(if cfg.input.option_as_alt {
                OptionAsAlt::Both
            } else {
                OptionAsAlt::None
            });
        }

        let mut renderer = Renderer::new(window.clone(), cfg)?;
        renderer.set_palette(&palette);
        renderer.set_skin(skin.clone());
        // Size the window for the configured grid now that the cell size is known.
        let (w, h) = renderer.surface_size_for(cfg.window.cols as usize, cfg.window.rows as usize);
        if let Some(size) = window.request_inner_size(PhysicalSize::new(w, h)) {
            renderer.resize(size.width, size.height);
        } else {
            let s = window.inner_size();
            renderer.resize(s.width, s.height);
        }
        let (cols, rows) = renderer.grid_size();
        let skin_rt = skin.map(|sk| {
            let s = window.inner_size();
            let rt = SkinRuntime::new(sk, (s.width, s.height), renderer.skin_scale());
            rt.apply_shape(&window);
            rt
        });

        let mut term = Terminal::new(cols, rows, cfg.scrollback_lines);
        apply_palette(&mut term, &mut renderer, &palette);
        let (cw, ch) = renderer.cell_size_px();
        term.set_cell_pixel_size(cw, ch);

        let (tx, rx) = mpsc::channel();
        let proxy = self.proxy.clone();
        let pending = self.wake_pending.clone();
        let (cmd, args) = match &self.launch.command {
            Some(c) => (c[0].clone(), c[1..].to_vec()),
            None => (cfg.shell.clone(), cfg.shell_args.clone()),
        };
        let pty = Pty::spawn_with(SpawnConfig {
            login_shell: args.is_empty() && self.launch.command.is_none(),
            passthrough: cfg.pty.passthrough_enabled(),
            cmd,
            args,
            cwd: self.launch.working_directory.clone(),
            env: crate::terminfo::child_env(),
            cols: cols as u16,
            rows: rows as u16,
            pixel_width: (cols as u32 * cw).min(u16::MAX as u32) as u16,
            pixel_height: (rows as u32 * ch).min(u16::MAX as u32) as u16,
            tx,
            on_wake: Some(Box::new(move || {
                // Coalesce: at most one PtyWake in flight.
                if !pending.swap(true, Ordering::AcqRel) {
                    let _ = proxy.send_event(UserEvent::PtyWake);
                }
            })),
        })
        .map_err(|e| format!("cannot start shell: {e}"))?;
        let writer = pty.writer();

        let font_pt = cfg.font.size;
        let mut running = Running {
            title: cfg.window.title.clone(),
            skin: skin_rt,
            chrome_cursor: false,
            manual_resize: None,
            scripted_pointer: false,
            win_state: WinState::Normal,
            window,
            renderer,
            term,
            pty,
            rx,
            writer,
            gfx: Graphics::with_cursor_moves(cfg.pty.graphics_cursor.follows_protocol_here()),
            clipboard: None,
            selection: Selection::new(),
            mods: ModifiersState::empty(),
            focused: true,
            shown: false,
            font_pt,
            last_pty_size: (cols, rows),
            mouse_px: (0.0, 0.0),
            left_down: false,
            last_mouse_cell: None,
            wheel_accum: 0.0,
            bell_until: None,
            mouse_inside: false,
            pointer_cursor: false,
            cursor_shape_event: None,
            blink_epoch: Instant::now(),
            blink_deadline: None,
            sync_poll: None,
            redraw_retry: None,
            retries: 0,
            cfg: cfg.clone(),
            reload: ReloadCtx {
                path: self.launch.config_path.clone(),
                theme_override: self.launch.theme.clone(),
                skin_override: self.launch.skin.clone(),
                _watcher: {
                    let proxy = self.proxy.clone();
                    reload::watch(self.launch.config_path.as_deref(), move || {
                        let _ = proxy.send_event(UserEvent::ConfigChanged);
                    })
                },
            },
            proxy: self.proxy.clone(),
            wake_pending: self.wake_pending.clone(),
            exit_requested: false,
            menu_accels: false,
            context_menu_requested: false,
        };
        running.update_skin_min_size();
        Ok(running)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(mut r) => {
                // NSApp exists now: install the menu bar (macOS) and build the
                // context menu.
                self.menus = AppMenus::new(self.proxy.clone(), &r.cfg.theme, &r.cfg.skin);
                r.menu_accels = self.menus.as_ref().is_some_and(AppMenus::has_menu_bar);
                r.redraw();
                self.running = Some(r);
            }
            Err(e) => {
                log::error!("{e}");
                self.fatal = Some(e);
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let cmd = match event {
            UserEvent::Debug(i) => self.debug_cmds.get(i).cloned(),
            _ => None,
        };
        let event = match event {
            UserEvent::OpenPreferences => return self.open_preferences(),
            UserEvent::MenuAction(action) => return self.menu_action(action),
            e => e,
        };
        let Some(r) = self.running.as_mut() else { return };
        match event {
            UserEvent::Debug(_) => {
                if let Some(c) = cmd {
                    r.debug_command(&c);
                }
                if std::mem::take(&mut r.context_menu_requested) {
                    if let Some(m) = self.menus.as_ref() {
                        m.show_context_menu(&r.window);
                    }
                }
            }
            UserEvent::PtyWake => r.on_pty_wake(),
            UserEvent::Redraw => r.window.request_redraw(),
            UserEvent::ConfigChanged => {
                r.reload_config();
                if let Some(m) = self.menus.as_mut() {
                    m.sync(&r.cfg.theme, &r.cfg.skin);
                }
                for w in self.secondary.values() {
                    w.request_redraw();
                }
            }
            UserEvent::OpenPreferences | UserEvent::MenuAction(_) => {}
        }
        if r.exit_requested {
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(w) = self.secondary.get_mut(&id) {
            if w.window_event(event_loop, event) == SecondaryFlow::Close {
                self.secondary.remove(&id);
            }
            return;
        }
        let Some(r) = self.running.as_mut() else { return };
        if r.window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                // A minimized window reports 0x0 on some platforms: keep the
                // grid (and the child's idea of its size) as it was.
                if size.width == 0 || size.height == 0 {
                    return;
                }
                r.renderer.resize(size.width, size.height);
                r.apply_grid_resize();
                r.skin_geometry_changed();
                r.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                r.renderer.set_scale_factor(crate::debug_script::effective_scale(scale_factor), r.cfg.window.padding);
                let s = r.window.inner_size();
                if s.width > 0 && s.height > 0 {
                    r.renderer.resize(s.width, s.height);
                }
                r.apply_grid_resize();
                r.update_skin_min_size();
                r.skin_geometry_changed();
                r.window.request_redraw();
            }
            WindowEvent::RedrawRequested => r.redraw(),
            WindowEvent::Occluded(false) => r.window.request_redraw(),
            WindowEvent::Focused(f) => r.on_focus(f),
            WindowEvent::ModifiersChanged(m) => r.on_modifiers(m.state()),
            WindowEvent::CursorLeft { .. } => r.on_cursor_left(),
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                if !is_synthetic {
                    r.on_key(&event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => r.on_cursor_moved(position.x, position.y),
            WindowEvent::MouseInput { state, button, .. } => r.on_mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => r.on_wheel(delta),
            WindowEvent::DroppedFile(path) => r.on_dropped_file(path),
            _ => {}
        }
        if std::mem::take(&mut r.context_menu_requested) {
            if let Some(m) = self.menus.as_ref() {
                m.show_context_menu(&r.window);
            }
        }
        if r.exit_requested {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(r) = self.running.as_mut() else { return };
        let next = r.poll_timers(Instant::now());
        match next {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// Everything that exists once the window is up.
struct Running {
    /// Skin pointer state; `None` for a normal OS-decorated window.
    skin: Option<SkinRuntime>,
    /// Window title as shown in the skin's title text.
    title: String,
    /// The pointer icon is currently set by the chrome (resize edges, ...).
    chrome_cursor: bool,
    /// Manual edge resize for platforms without `drag_resize_window`.
    manual_resize: Option<ResizeDrag>,
    /// A debug script is driving the pointer: use its window coordinates
    /// instead of the real (global) pointer for manual resizes.
    scripted_pointer: bool,
    /// Normal / maximized / shaded, with what each restores to. The OS zoom
    /// is never used for a skinned window (see `skin::winstate`).
    win_state: WinState,
    window: Arc<Window>,
    renderer: Renderer,
    term: Terminal,
    pty: Pty,
    rx: Receiver<PtyEvent>,
    writer: PtyWriter,
    gfx: Graphics,
    clipboard: Option<arboard::Clipboard>,
    selection: Selection,
    mods: ModifiersState,
    focused: bool,
    /// The window starts hidden and is shown after the first frame.
    shown: bool,
    font_pt: f32,
    last_pty_size: (usize, usize),
    mouse_px: (f64, f64),
    left_down: bool,
    /// Cell of the last reported mouse event (motion is reported per cell).
    last_mouse_cell: Option<(usize, usize)>,
    wheel_accum: f64,
    /// The visual bell flash ends at this time.
    bell_until: Option<Instant>,
    /// The pointer is inside the window (link hover needs to know).
    mouse_inside: bool,
    /// The pointer cursor is currently shown for a hovered link.
    pointer_cursor: bool,
    /// Cursor shape last set by the program (DECSCUSR); `None` = config default.
    cursor_shape_event: Option<CursorShape>,
    blink_epoch: Instant,
    blink_deadline: Option<Instant>,
    sync_poll: Option<Instant>,
    redraw_retry: Option<Instant>,
    /// Consecutive `FrameStatus::Retry` results.
    retries: u8,
    cfg: Config,
    reload: ReloadCtx,
    proxy: EventLoopProxy<UserEvent>,
    wake_pending: Arc<AtomicBool>,
    exit_requested: bool,
    /// The menu bar owns the Cmd+C/V/+/-/0/, accelerators (macOS): ignore
    /// those key events here so an action never runs twice.
    menu_accels: bool,
    /// A right click asked for the context menu (at the pointer); the
    /// app shows it (it owns the menus) once the event is handled.
    context_menu_requested: bool,
}

/// The skin's `viewport_background` stands in for the terminal background
/// only when the user chose neither a theme nor a `[colors] background`;
/// an explicit theme or colour always wins (docs/SKINS.md).
fn skin_palette(mut p: theme::Palette, raw: &Config, skin: Option<&Skin>) -> theme::Palette {
    let user_chose = raw.colors.background.is_some() || raw.theme != theme::DEFAULT_THEME;
    if let (false, Some(bg)) = (user_chose, skin.and_then(|s| s.def.frame.viewport_background)) {
        p.background = bg;
    }
    p
}

/// Push a resolved theme into the terminal (default colors + ANSI palette)
/// and the renderer (selection, cursor text, link colors).
fn apply_palette(term: &mut Terminal, renderer: &mut Renderer, p: &theme::Palette) {
    term.set_default_colors(p.foreground.tuple(), p.background.tuple(), p.cursor.map(|c| c.tuple()));
    for (i, c) in p.ansi.iter().enumerate() {
        term.set_palette_color(i as u8, c.tuple());
    }
    renderer.set_palette(p);
}

impl Drop for Running {
    fn drop(&mut self) {
        // Window closed (or the child exited): make sure no child outlives us.
        // Bounded so closing the window can never hang.
        self.pty.shutdown(Duration::from_millis(300));
    }
}

impl Running {
    // -- pty ---------------------------------------------------------------

    fn on_pty_wake(&mut self) {
        // Clear before draining so a wake arriving mid-drain re-posts.
        self.wake_pending.store(false, Ordering::Release);
        let mut budget = PTY_BUDGET;
        let mut got_output = false;
        let mut more = false;
        loop {
            match self.rx.try_recv() {
                Ok(PtyEvent::Output(bytes)) => {
                    self.gfx.feed(&mut self.term, &bytes);
                    got_output = true;
                    budget = budget.saturating_sub(bytes.len());
                    if budget == 0 {
                        more = true;
                        break;
                    }
                }
                Ok(PtyEvent::Exited(code)) => {
                    log::info!("child exited ({code:?})");
                    self.exit_requested = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.exit_requested = true;
                    break;
                }
            }
        }
        if more && !self.wake_pending.swap(true, Ordering::AcqRel) {
            let _ = self.proxy.send_event(UserEvent::PtyWake);
        }
        if got_output {
            self.after_advance();
        }
    }

    /// Respond to the terminal, handle its events, and schedule a redraw.
    fn after_advance(&mut self) {
        let mut need_redraw = false;
        let events = self.gfx.drain_events(&mut self.term);
        need_redraw |= self.gfx.take_dirty();
        for ev in events {
            match ev {
                TermEvent::Title(t) => {
                    if self.cfg.window.dynamic_title {
                        let title = if t.is_empty() { self.cfg.window.title.clone() } else { t };
                        self.window.set_title(&title);
                        if self.skin.is_some() && title != self.title {
                            self.title = title;
                            need_redraw = true;
                        }
                    }
                }
                TermEvent::Bell => {
                    if !self.focused {
                        self.window.request_user_attention(Some(UserAttentionType::Informational));
                    }
                    need_redraw |= self.ring_bell();
                }
                TermEvent::ClipboardSet(text) => {
                    if self.cfg.clipboard.osc52_write {
                        self.set_clipboard(text);
                    }
                }
                TermEvent::ClipboardQuery => {
                    let text = if self.cfg.clipboard.osc52_read {
                        self.clipboard().and_then(|cb| cb.get_text().ok()).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    self.term.respond_clipboard(&text);
                }
                TermEvent::CwdChanged(_) | TermEvent::SetMouseMode => {}
                TermEvent::ColorChanged => {
                    self.renderer.invalidate();
                    need_redraw = true;
                }
                TermEvent::CursorShape(s) => {
                    self.cursor_shape_event = Some(s);
                    need_redraw = true;
                }
                // Handled by `Graphics` (graphics_glue.rs) as the bytes are fed.
                TermEvent::GraphicsCommand(_) => {}
            }
        }
        // The program wrote over (or switched away from) the selected text.
        if self.selection.is_active() && self.selection.validate(&self.term) {
            self.renderer.invalidate();
            need_redraw = true;
        }
        let resp = self.term.drain_responses();
        if !resp.is_empty() {
            let _ = self.writer.write_all(&resp);
        }
        self.reset_blink();
        if self.term.synchronized_output() {
            // Frame is being assembled; poll for its end (or the 1s timeout).
            self.sync_poll.get_or_insert(Instant::now() + SYNC_POLL);
        } else {
            self.sync_poll = None;
            if need_redraw || self.term.is_dirty() {
                self.window.request_redraw();
            }
        }
    }

    fn set_clipboard(&mut self, text: String) {
        if let Some(cb) = self.clipboard() {
            if let Err(e) = cb.set_text(text) {
                log::warn!("clipboard set failed: {e}");
            }
        }
    }

    /// Visual and audible bell per config. Returns true if a redraw is needed.
    fn ring_bell(&mut self) -> bool {
        if self.cfg.bell.audible {
            crate::bell::beep();
        }
        if self.cfg.bell.visual && self.cfg.bell.visual_ms > 0 {
            self.bell_until = Some(Instant::now() + Duration::from_millis(self.cfg.bell.visual_ms as u64));
            return true;
        }
        false
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => log::warn!("clipboard unavailable: {e}"),
            }
        }
        self.clipboard.as_mut()
    }

    fn send(&mut self, bytes: &[u8]) {
        if self.writer.write_all(bytes).is_err() {
            log::debug!("pty write failed (child gone?)");
        }
    }

    // -- layout ------------------------------------------------------------

    /// Re-derive the grid from the surface size and tell the terminal + pty.
    fn apply_grid_resize(&mut self) {
        let (cols, rows) = self.renderer.grid_size();
        let (cw, ch) = self.renderer.cell_size_px();
        self.term.set_cell_pixel_size(cw, ch);
        if self.term.size() != (cols, rows) {
            self.term.resize(cols, rows);
            self.selection.clear();
            self.renderer.invalidate();
        }
        if self.last_pty_size != (cols, rows) {
            self.last_pty_size = (cols, rows);
            let clamp16 = |v: u64| v.min(u16::MAX as u64) as u16;
            let _ = self.pty.resize(
                clamp16(cols as u64).max(1),
                clamp16(rows as u64).max(1),
                clamp16(cols as u64 * cw as u64),
                clamp16(rows as u64 * ch as u64),
            );
        }
    }

    /// The config file or a theme changed: re-read it and apply the theme,
    /// colors, font and cursor settings without restarting. A file that does
    /// not parse is reported and the running settings stay.
    fn reload_config(&mut self) {
        let mut raw = match Config::try_load(self.reload.path.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                log::error!("config reload failed, keeping current settings: {e}");
                return;
            }
        };
        if let Some(t) = &self.reload.theme_override {
            raw.theme = t.clone();
        }
        if let Some(s) = &self.reload.skin_override {
            raw.skin = s.clone();
        }
        self.swap_skin(&raw.skin);
        let skin = self.skin.as_ref().map(|r| r.skin.clone());
        let resolved = theme::resolve_for_skin(&raw, skin.as_deref());
        log::info!("config reloaded (theme: {})", resolved.name);
        let palette = skin_palette(resolved.palette, &raw, skin.as_deref());
        apply_palette(&mut self.term, &mut self.renderer, &palette);
        let new = resolved.config;
        let old = std::mem::replace(&mut self.cfg, new);
        let new = &self.cfg;

        let font_changed = new.font.family != old.font.family
            || new.font.line_height != old.font.line_height
            || new.font.size != old.font.size;
        let padding_changed = new.window.padding != old.window.padding;
        if font_changed {
            // A zoomed window keeps its zoom unless the configured size changed.
            let pt = if new.font.size != old.font.size { new.font.size } else { self.font_pt };
            self.font_pt = pt;
            self.renderer.set_font(&new.font.family, pt, new.font.line_height);
        }
        if padding_changed {
            self.renderer.set_scale_factor(crate::debug_script::effective_scale(self.window.scale_factor()), new.window.padding);
        }
        if font_changed || padding_changed {
            let s = self.window.inner_size();
            if s.width > 0 && s.height > 0 {
                self.renderer.resize(s.width, s.height);
            }
            self.apply_grid_resize();
        }
        self.renderer.invalidate();
        self.window.request_redraw();
    }

    /// Hot-swap the window skin on a config reload: a different skin name, or
    /// any skin loaded from a user directory (so its files can be edited live).
    /// Going between "no skin" and a skin needs a different window (decorations,
    /// surface alpha) and waits for the next launch.
    fn swap_skin(&mut self, name: &str) {
        let wanted = skin::is_active(name);
        let Some(cur) = self.skin.as_ref() else {
            if wanted {
                log::info!("skin {name:?} applies on the next launch (this window has no skin)");
            }
            return;
        };
        if !wanted {
            log::info!("skin \"none\" applies on the next launch");
            return;
        }
        if name == self.cfg.skin && cur.skin.origin.starts_with("built-in") {
            return;
        }
        if self.win_state.is_shaded() {
            log::info!("skin change deferred: restore the shaded window first");
            return;
        }
        let new = match Skin::load(name) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                log::error!("cannot load skin {name:?}, keeping the current one: {e}");
                return;
            }
        };
        self.renderer.set_skin(Some(new.clone()));
        let size = self.window.inner_size();
        self.skin = Some(SkinRuntime::new(new, (size.width, size.height), self.renderer.skin_scale()));
        self.renderer.resize(size.width.max(1), size.height.max(1));
        self.apply_grid_resize();
        self.update_skin_min_size();
        self.skin_geometry_changed();
    }

    fn zoom(&mut self, op: ZoomOp) {
        let new = match op {
            ZoomOp::In => self.font_pt + 1.0,
            ZoomOp::Out => self.font_pt - 1.0,
            ZoomOp::Reset => self.cfg.font.size,
        }
        .clamp(6.0, 96.0);
        if (new - self.font_pt).abs() < f32::EPSILON {
            return;
        }
        self.font_pt = new;
        self.renderer.set_font_size(new);
        self.apply_grid_resize();
        self.window.request_redraw();
    }

    // -- drawing -----------------------------------------------------------

    fn cursor_blinking(&self) -> bool {
        self.focused && self.term.cursor_visible() && (self.cfg.cursor.blink || self.term.cursor_blink())
    }

    fn blink_on(&self, now: Instant) -> bool {
        if !self.cursor_blinking() {
            return true;
        }
        let n = now.saturating_duration_since(self.blink_epoch).as_millis() / BLINK_INTERVAL.as_millis();
        n.is_multiple_of(2)
    }

    fn reset_blink(&mut self) {
        self.blink_epoch = Instant::now();
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let shape = self.cursor_shape_event.unwrap_or(match self.cfg.cursor.shape {
            CursorShapeCfg::Block => CursorShape::Block,
            CursorShapeCfg::Underline => CursorShape::Underline,
            CursorShapeCfg::Bar => CursorShape::Bar,
        });
        let hover = if self.link_hover_enabled() && self.mouse_inside {
            links::link_at(&self.term, self.cell_at(self.mouse_px))
        } else {
            None
        };
        self.set_pointer_cursor(hover.is_some());
        let flash = if self.bell_until.is_some_and(|t| now < t) { 1.0 } else { 0.0 };
        let view = ViewState {
            focused: self.focused,
            cursor_on: self.blink_on(now),
            cursor_shape: shape,
            selection: &self.selection,
            hover: hover.as_ref(),
            flash,
            chrome: self.skin.as_ref().map(|s| s.view(&self.title)),
        };
        let status = self.renderer.render(&mut self.term, &view, self.gfx.protocol_mut());
        log::debug!("frame status: {status:?}");
        match status {
            // The surface was reconfigured: try again right away, but if that
            // keeps failing back off to a timer instead of spinning.
            FrameStatus::Retry => {
                self.retries = self.retries.saturating_add(1);
                if self.retries <= 3 {
                    self.window.request_redraw();
                } else {
                    self.redraw_retry = Some(now + FRAME_RETRY);
                }
            }
            FrameStatus::Timeout => self.redraw_retry = Some(now + FRAME_RETRY),
            // Occluded: wait for WindowEvent::Occluded(false).
            FrameStatus::Occluded => {}
            FrameStatus::Presented => {}
        }
        if matches!(status, FrameStatus::Presented | FrameStatus::Occluded) {
            self.retries = 0;
            // Benchmark aid: quit once the first frame is on screen.
            if std::env::var_os("GRIMALKIN_EXIT_AFTER_FRAME").is_some() {
                self.exit_requested = true;
            }
        }
        if !self.shown {
            self.shown = true;
            self.window.set_visible(true);
            if !matches!(status, FrameStatus::Presented) {
                self.window.request_redraw();
            }
        }
    }

    /// Run due timers; returns the next wakeup time, if any.
    fn poll_timers(&mut self, now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut merge = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));

        // Synchronized output (mode 2026): poll until it ends or times out.
        if let Some(t) = self.sync_poll {
            if now >= t {
                self.term.check_sync_timeout(Duration::from_secs(1));
                if self.term.synchronized_output() {
                    self.sync_poll = Some(now + SYNC_POLL);
                } else {
                    self.sync_poll = None;
                    self.window.request_redraw();
                }
            }
            if let Some(t) = self.sync_poll {
                merge(t);
            }
        }

        // Visual bell: redraw once when the flash is over.
        if let Some(t) = self.bell_until {
            if now >= t {
                self.bell_until = None;
                self.window.request_redraw();
            } else {
                merge(t);
            }
        }

        // A frame could not be acquired: retry shortly.
        if let Some(t) = self.redraw_retry {
            if now >= t {
                self.redraw_retry = None;
                self.window.request_redraw();
            } else {
                merge(t);
            }
        }

        // Cursor blink: wake at every phase boundary, only while blinking.
        if self.cursor_blinking() && self.term.display_offset() == 0 {
            let elapsed = now.saturating_duration_since(self.blink_epoch);
            let n = elapsed.as_millis() / BLINK_INTERVAL.as_millis();
            let boundary = self.blink_epoch + BLINK_INTERVAL * (n as u32 + 1);
            if self.blink_deadline.is_some_and(|d| now >= d) {
                self.window.request_redraw();
            }
            self.blink_deadline = Some(boundary);
            merge(boundary);
        } else {
            self.blink_deadline = None;
        }
        next
    }

    fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.end_manual_resize();
        }
        if self.term.focus_reporting() {
            self.send(if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
        self.reset_blink();
        self.window.request_redraw();
    }

    // -- keyboard ----------------------------------------------------------

    fn on_key(&mut self, ev: &winit::event::KeyEvent) {
        let st = EncodeState {
            app_cursor: self.term.app_cursor_keys(),
            kitty_flags: self.term.keyboard_flags(),
            alt_screen: self.term.screen() == Screen::Alt,
            alt_is_meta: !input::IS_MAC || self.cfg.input.option_as_alt,
        };
        match input::translate(&KeyInput::from_event(ev), self.mods, &st) {
            KeyAction::None => {}
            KeyAction::Send(bytes) => {
                if self.term.scroll_display_to_bottom() {
                    self.window.request_redraw();
                }
                self.reset_blink();
                self.send(&bytes);
            }
            // With menu accelerators the menu already ran these for the same
            // keystroke; Ctrl+Shift+C and friends still come through here.
            KeyAction::Copy | KeyAction::Paste | KeyAction::Zoom(_) | KeyAction::OpenPreferences
                if self.menu_accels && self.mods.super_key() => {}
            KeyAction::Copy => self.copy(),
            KeyAction::Paste => self.paste(),
            KeyAction::Zoom(op) => self.zoom(op),
            KeyAction::OpenPreferences => {
                let _ = self.proxy.send_event(UserEvent::OpenPreferences);
            }
            KeyAction::ScrollPages(p) => {
                let page = self.term.rows().saturating_sub(1).max(1) as i32;
                if self.term.scroll_display(p * page) {
                    self.window.request_redraw();
                }
            }
        }
    }

    fn copy(&mut self) {
        if let Some(text) = self.selection.selected_text(&self.term) {
            self.set_clipboard(text);
        }
    }

    /// Copy-on-select: put the selection into the PRIMARY selection (X11 and
    /// Wayland only; other platforms have no such thing).
    #[cfg(target_os = "linux")]
    fn copy_primary(&mut self) {
        use arboard::{LinuxClipboardKind, SetExtLinux};
        if let Some(text) = self.selection.selected_text(&self.term) {
            if let Some(cb) = self.clipboard() {
                if let Err(e) = cb.set().clipboard(LinuxClipboardKind::Primary).text(text) {
                    log::debug!("primary selection set failed: {e}");
                }
            }
        }
    }

    /// Middle-click paste from the PRIMARY selection.
    #[cfg(target_os = "linux")]
    fn paste_primary(&mut self) {
        use arboard::{GetExtLinux, LinuxClipboardKind};
        let text = self
            .clipboard()
            .and_then(|cb| cb.get().clipboard(LinuxClipboardKind::Primary).text().ok());
        if let Some(text) = text {
            self.paste_text(&text);
        }
    }

    // -- links -------------------------------------------------------------

    /// Ctrl (Cmd on macOS) is the modifier that activates links.
    fn link_mod(&self, mods: ModifiersState) -> bool {
        if input::IS_MAC {
            mods.super_key()
        } else {
            mods.control_key()
        }
    }

    /// Links are active while the modifier is held, unless an application has
    /// mouse reporting on (then Shift is needed too, as for selection).
    fn link_hover_enabled(&self) -> bool {
        self.link_mod(self.mods)
            && (self.term.mouse_mode() == grimalkin_vt::MouseMode::None || self.mods.shift_key())
    }

    fn set_pointer_cursor(&mut self, on: bool) {
        if on != self.pointer_cursor {
            self.pointer_cursor = on;
            self.window
                .set_cursor(if on { CursorIcon::Pointer } else { CursorIcon::Default });
        }
    }

    /// Open the link under the pointer, if links are active. Returns true if
    /// one was found (the click is then consumed).
    fn try_open_link(&mut self) -> bool {
        if !self.link_hover_enabled() {
            return false;
        }
        match links::link_at(&self.term, self.cell_at(self.mouse_px)) {
            Some(l) => {
                links::open(&l.uri);
                true
            }
            None => false,
        }
    }

    fn on_modifiers(&mut self, mods: ModifiersState) {
        let (was, is) = (self.link_mod(self.mods), self.link_mod(mods));
        self.mods = mods;
        if was != is || self.pointer_cursor {
            self.window.request_redraw();
        }
    }

    fn on_cursor_left(&mut self) {
        self.mouse_inside = false;
        if self.skin.as_mut().is_some_and(|s| s.pointer_left()) {
            self.window.request_redraw();
        }
        if self.pointer_cursor {
            self.window.request_redraw();
        }
    }

    fn paste(&mut self) {
        let text = match self.clipboard().map(|cb| cb.get_text()) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                log::debug!("clipboard read failed: {e}");
                return;
            }
            None => return,
        };
        self.paste_text(&text);
    }

    fn paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = input::paste_bytes(text, self.term.bracketed_paste());
        if self.term.scroll_display_to_bottom() {
            self.window.request_redraw();
        }
        self.send(&bytes);
    }

    fn on_dropped_file(&mut self, path: PathBuf) {
        let s = path.to_string_lossy();
        let quoted: String = s
            .chars()
            .flat_map(|c| {
                if c.is_alphanumeric() || "/._-~:+@%".contains(c) {
                    vec![c]
                } else {
                    vec!['\\', c]
                }
            })
            .collect();
        self.paste_text(&format!("{quoted} "));
    }

    // -- test hooks ----------------------------------------------------------

    /// One command of `GRIMALKIN_DEBUG_SCRIPT` (see `debug_script`): runs
    /// through the same handlers real input uses.
    fn debug_command(&mut self, cmd: &str) {
        log::info!("debug script: {cmd}");
        self.debug_command_inner(cmd);
        let s = self.window.inner_size();
        log::info!("debug script: inner size {}x{}", s.width, s.height);
        self.debug_report();
    }

    /// Assert after every scripted step that the chrome, the grid freeze and
    /// the window agree (`consistent=true`), and log the state.
    fn debug_report(&self) {
        let s = self.window.inner_size();
        let Some(sk) = self.skin.as_ref() else { return };
        let bar = self.shade_bar();
        let want = s.height <= bar;
        let ok = sk.shaded() == want && self.renderer.chrome_shaded() == want;
        log::info!(
            "debug script: state {:?} shaded(chrome)={} shaded(renderer)={} window {}x{} bar {} consistent={}",
            self.win_state,
            sk.shaded(),
            self.renderer.chrome_shaded(),
            s.width,
            s.height,
            bar,
            ok
        );
    }

    fn debug_command_inner(&mut self, cmd: &str) {
        let (name, arg) = cmd.split_once(':').unwrap_or((cmd, ""));
        let button_centre = |me: &Self, name: &str| {
            let kind = match name {
                "close" => skin::hit::ButtonKind::Close,
                "minimize" => skin::hit::ButtonKind::Minimize,
                "maximize" => skin::hit::ButtonKind::Maximize,
                _ => skin::hit::ButtonKind::Shade,
            };
            let r = me.skin.as_ref()?.layout.button_rect(kind)?;
            Some((r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0))
        };
        match name {
            "move" => {
                self.scripted_pointer = true;
                if let Some((x, y)) = arg.split_once(',').and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))) {
                    self.on_cursor_moved(x, y);
                }
            }
            "hover" | "press" => {
                self.scripted_pointer = true;
                if let Some((x, y)) = button_centre(self, arg) {
                    self.on_cursor_moved(x, y);
                    if name == "press" {
                        self.on_mouse_button(ElementState::Pressed, MouseButton::Left);
                    }
                }
            }
            "down" => self.on_mouse_button(ElementState::Pressed, MouseButton::Left),
            "up" => self.on_mouse_button(ElementState::Released, MouseButton::Left),
            "click" | "dblclick" => {
                for _ in 0..if name == "click" { 1 } else { 2 } {
                    self.on_mouse_button(ElementState::Pressed, MouseButton::Left);
                    self.on_mouse_button(ElementState::Released, MouseButton::Left);
                }
            }
            // `menu:ID` runs a menu action (ids as in menu.rs) through the real
            // menu path; `rightclick` opens the context menu at the pointer.
            "menu" => {
                if let Some(action) = MenuAction::from_id(arg) {
                    let _ = self.proxy.send_event(UserEvent::MenuAction(action));
                }
            }
            "rightclick" => self.on_mouse_button(ElementState::Pressed, MouseButton::Right),
            "shade" => self.run_skin_action(Action::ToggleShade),
            "size" => {
                if let Some((w, h)) = arg.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))) {
                    if let Some(size) = self.window.request_inner_size(PhysicalSize::new(w, h)) {
                        self.renderer.resize(size.width, size.height);
                        self.apply_grid_resize();
                        self.skin_geometry_changed();
                        self.window.request_redraw();
                    }
                }
            }
            "check" => {
                // Step delays let the window system deliver its Resized
                // events first: the chrome must match the window by now.
                let s = self.window.inner_size();
                let want = s.height <= self.shade_bar();
                let ok = self.skin.as_ref().is_none_or(|sk| sk.shaded() == want && self.renderer.chrome_shaded() == want);
                log::info!("debug script: CHECK {}", if ok { "ok" } else { "FAIL" });
            }
            "maximize" => self.run_skin_action(Action::ToggleMaximize),
            "dump" => {
                self.renderer.set_dump_path(Some(PathBuf::from(arg)));
                self.window.request_redraw();
            }
            "exit" => self.exit_requested = true,
            _ => log::warn!("debug script: unknown command {cmd:?}"),
        }
    }

    // -- skin --------------------------------------------------------------

    /// Smallest window that keeps the skin's fixed frame parts intact.
    fn skin_min_size(&self) -> Option<PhysicalSize<u32>> {
        let sk = self.skin.as_ref()?;
        let (w, h) = sk.skin.min_window(sk.scale(), self.renderer.cell_size_px());
        Some(PhysicalSize::new(w, h))
    }

    fn update_skin_min_size(&mut self) {
        if self.skin.is_some() && !self.win_state.is_shaded() {
            self.window.set_min_inner_size(self.skin_min_size());
        }
    }

    /// The window size or display scale changed: re-layout the chrome and
    /// (Windows) re-clip the window to the shape.
    fn skin_geometry_changed(&mut self) {
        let scale = self.renderer.skin_scale();
        let size = self.window.inner_size();
        let maximized = self.win_state.is_maximized();
        if let Some(sk) = self.skin.as_mut() {
            sk.set_geometry((size.width, size.height), scale);
            sk.set_resizable(!maximized);
            sk.apply_shape(&self.window);
        }
    }

    /// Returns true if the pointer motion belongs to the chrome.
    fn skin_pointer_moved(&mut self, x: f64, y: f64) -> bool {
        if self.manual_resize.is_some() {
            // The button is the authority: if the system says it is up (a
            // mouse-up we never saw), the drag is over.
            if !self.scripted_pointer && crate::pointer::left_button_down() == Some(false) {
                self.end_manual_resize();
            } else {
                self.manual_resize_to(x, y);
                return true;
            }
        }
        let Some(sk) = self.skin.as_mut() else { return false };
        let (redraw, icon) = sk.hover_cursor(x, y);
        let hit = sk.hover();
        if redraw {
            self.window.request_redraw();
        }
        self.apply_chrome_cursor(icon);
        // A selection drag that wandered over the frame keeps going.
        hit != Hit::Viewport && !self.left_down
    }

    /// Show the chrome's icon, or give the pointer back to the viewport.
    fn apply_chrome_cursor(&mut self, icon: Option<CursorIcon>) {
        match icon {
            Some(icon) => {
                self.window.set_cursor(icon);
                self.chrome_cursor = true;
            }
            None => {
                if self.chrome_cursor {
                    self.chrome_cursor = false;
                    self.window
                        .set_cursor(if self.pointer_cursor { CursorIcon::Pointer } else { CursorIcon::Default });
                }
            }
        }
    }

    /// The manual resize is over (button released, focus lost, ...): drop the
    /// latched state and put the pointer icon back to what is under it.
    fn end_manual_resize(&mut self) {
        if self.manual_resize.take().is_none() {
            return;
        }
        let (x, y) = self.mouse_px;
        let icon = self.skin.as_mut().and_then(|sk| sk.hover_cursor(x, y).1);
        self.apply_chrome_cursor(icon);
        self.window.request_redraw();
    }

    /// Returns true if the left-button event belongs to the chrome.
    fn skin_mouse_button(&mut self, pressed: bool) -> bool {
        let (x, y) = self.mouse_px;
        if !pressed && self.manual_resize.is_some() {
            self.end_manual_resize();
            return true;
        }
        let Some(sk) = self.skin.as_mut() else { return false };
        let action = if pressed {
            if sk.hit_at(x, y) == Hit::Viewport {
                return false;
            }
            sk.press(x, y, Instant::now())
        } else {
            if sk.pressed_button().is_none() {
                return false;
            }
            sk.release(x, y)
        };
        self.window.request_redraw();
        if let Some(a) = action {
            self.run_skin_action(a);
        }
        true
    }

    fn run_skin_action(&mut self, action: Action) {
        match action {
            Action::Close => self.exit_requested = true,
            Action::Minimize => self.window.set_minimized(true),
            Action::ToggleMaximize => self.toggle_maximize(),
            Action::ToggleShade => self.toggle_shade(),
            // A maximized window stays where it is.
            Action::DragMove if self.win_state.is_maximized() => {}
            Action::DragMove => {
                if let Err(e) = self.window.drag_window() {
                    log::debug!("drag_window: {e}");
                }
            }
            Action::DragResize(dir) => {
                let d = match dir {
                    ResizeDir::N => ResizeDirection::North,
                    ResizeDir::S => ResizeDirection::South,
                    ResizeDir::E => ResizeDirection::East,
                    ResizeDir::W => ResizeDirection::West,
                    ResizeDir::NE => ResizeDirection::NorthEast,
                    ResizeDir::NW => ResizeDirection::NorthWest,
                    ResizeDir::SE => ResizeDirection::SouthEast,
                    ResizeDir::SW => ResizeDirection::SouthWest,
                };
                if self.window.drag_resize_window(d).is_err() {
                    self.start_manual_resize(dir);
                }
            }
        }
    }

    /// The pointer on the screen, physical pixels. The system's global
    /// position when there is one: window-local coordinates are re-derived
    /// against a frame that is changing during the drag and cannot be trusted
    /// for it. Falls back to the window position plus the local position.
    fn screen_pointer(&self, local: (f64, f64)) -> Option<(f64, f64)> {
        if !self.scripted_pointer {
            if let Some((gx, gy)) = crate::pointer::global_position() {
                let k = self.window.scale_factor();
                return Some((gx * k, gy * k));
            }
        }
        let p = self.window.inner_position().ok()?;
        Some((p.x as f64 + local.0, p.y as f64 + local.1))
    }

    /// Latch a pointer-driven resize (platforms without `drag_resize_window`).
    /// The direction, pointer anchor, window origin and size are captured
    /// here and kept until the button is released.
    fn start_manual_resize(&mut self, dir: ResizeDir) {
        let Some(anchor) = self.screen_pointer(self.mouse_px) else { return };
        let Ok(origin) = self.window.outer_position() else { return };
        let size = self.window.inner_size();
        let min = self.skin_min_size().unwrap_or(PhysicalSize::new(64, 64));
        log::debug!("manual resize {dir:?} start: size {}x{} origin {},{}", size.width, size.height, origin.x, origin.y);
        let drag = ResizeDrag::begin(dir, anchor, (origin.x, origin.y), (size.width, size.height), (min.width, min.height));
        self.manual_resize = Some(drag);
        // Keep the resize icon for the whole drag, wherever the pointer goes.
        let icon = self.skin.as_ref().and_then(|sk| sk.cursor_for(Hit::ResizeEdge(drag.dir())));
        self.apply_chrome_cursor(icon);
    }

    fn manual_resize_to(&mut self, x: f64, y: f64) {
        let Some(m) = self.manual_resize else { return };
        let Some(pointer) = self.screen_pointer((x, y)) else { return };
        let (pos, (w, h)) = m.target(pointer);
        let size = PhysicalSize::new(w, h);
        if self.window.inner_size() != size {
            // Resize first, then put the frame where the drag says it belongs:
            // AppKit keeps the bottom-left corner fixed on a resize, which
            // would otherwise make the top edge (and so the pointer position
            // relative to the window) slide with every step.
            if let Some(s) = self.window.request_inner_size(size) {
                self.renderer.resize(s.width, s.height);
                self.apply_grid_resize();
                self.skin_geometry_changed();
                self.window.request_redraw();
            }
        }
        if self.window.outer_position().is_ok_and(|p| (p.x, p.y) != pos) {
            self.window.set_outer_position(PhysicalPosition::new(pos.0, pos.1));
        }
    }

    /// The window as the state machine sees it: outer position and inner size.
    fn window_rect(&self) -> Rect {
        let p = self.window.outer_position().unwrap_or_default();
        let s = self.window.inner_size();
        Rect::new(p.x, p.y, s.width, s.height)
    }

    /// Height of the shade bar in physical pixels.
    fn shade_bar(&self) -> u32 {
        self.skin.as_ref().map_or(1, |sk| (sk.skin.shade_height() * sk.scale()).max(1))
    }

    /// Maximize / restore, done by hand: fill the monitor's work area, or go
    /// back to where the window was. Maximizing a shaded window unshades it.
    fn toggle_maximize(&mut self) {
        if self.skin.is_none() {
            return;
        }
        let plan = self.win_state.toggle_maximize(self.window_rect(), skin::work_area(&self.window));
        self.apply_plan(plan);
    }

    /// Windowshade: collapse the window to the skin's shade bar, or restore
    /// the size it had before. The terminal keeps running; its grid (and the
    /// child's idea of the window size) stay as they were.
    fn toggle_shade(&mut self) {
        if self.skin.is_none() {
            return;
        }
        let plan = self.win_state.toggle_shade(self.window_rect(), self.shade_bar(), skin::work_area(&self.window));
        self.apply_plan(plan);
    }

    /// Move to the next window state and put the window where it says. The
    /// chrome and the grid follow from the resulting window size.
    fn apply_plan(&mut self, plan: Plan) {
        log::debug!("window state {:?} -> {:?}, target {:?}", self.win_state, plan.state, plan.target);
        self.win_state = plan.state;
        let t = plan.target;
        let min = if plan.state.is_shaded() {
            let min_w = self.skin_min_size().map_or(1, |m| m.width);
            Some(PhysicalSize::new(min_w, self.shade_bar()))
        } else {
            self.skin_min_size()
        };
        // The minimum size must never sit above the size being asked for:
        // lower it before shrinking, raise it after growing.
        if plan.state.is_shaded() {
            self.window.set_min_inner_size(min);
        }
        if let Some(size) = self.window.request_inner_size(PhysicalSize::new(t.w, t.h)) {
            self.renderer.resize(size.width, size.height);
        }
        // AppKit keeps the bottom-left corner on a resize: place the frame last.
        self.window.set_outer_position(PhysicalPosition::new(t.x, t.y));
        if !plan.state.is_shaded() {
            self.window.set_min_inner_size(min);
        }
        self.apply_grid_resize();
        self.skin_geometry_changed();
        self.window.request_redraw();
    }

    // -- mouse -------------------------------------------------------------

    fn cell_at(&self, px: (f64, f64)) -> CellPos {
        let (cw, ch) = self.renderer.cell_size_px();
        let (ox, oy) = self.renderer.grid_origin_px();
        let (cols, rows) = self.term.size();
        let col = (((px.0 - ox as f64) / cw as f64).floor().max(0.0) as usize).min(cols.saturating_sub(1));
        let row = (((px.1 - oy as f64) / ch as f64).floor().max(0.0) as usize).min(rows.saturating_sub(1));
        CellPos { col, row }
    }

    fn mouse_event(&self, kind: MouseKind, button: MouseBtn) -> MouseEvent {
        let pos = self.cell_at(self.mouse_px);
        MouseEvent {
            kind,
            button,
            col: pos.col,
            row: pos.row,
            px: self.mouse_px,
            mods: self.mods,
        }
    }

    /// Offer an event to mouse reporting. Shift bypasses reporting. Returns
    /// true if the event belongs to the application (reporting is active),
    /// whether or not anything was sent, so it never also drives selection.
    fn report_mouse(&mut self, ev: &MouseEvent) -> bool {
        use grimalkin_vt::MouseMode;
        if self.term.mouse_mode() == MouseMode::None || self.mods.shift_key() {
            return false;
        }
        if ev.kind == MouseKind::Motion && self.last_mouse_cell == Some((ev.col, ev.row)) {
            return true;
        }
        if let Some(bytes) = input::encode_mouse(self.term.mouse_mode(), self.term.mouse_encoding(), ev) {
            self.last_mouse_cell = Some((ev.col, ev.row));
            self.send(&bytes);
        }
        true
    }

    fn on_cursor_moved(&mut self, x: f64, y: f64) {
        self.mouse_px = (x, y);
        self.mouse_inside = true;
        if self.skin_pointer_moved(x, y) {
            return;
        }
        // Link hover underline / pointer cursor follow the mouse.
        if self.link_hover_enabled() || self.pointer_cursor {
            self.window.request_redraw();
        }
        let btn = if self.left_down { MouseBtn::Left } else { MouseBtn::None };
        let ev = self.mouse_event(MouseKind::Motion, btn);
        if self.report_mouse(&ev) {
            return;
        }
        if self.left_down {
            self.autoscroll_selection(y);
            let pos = self.cell_at(self.mouse_px);
            if self.selection.on_drag(&self.term, pos) {
                self.renderer.invalidate();
                self.window.request_redraw();
            }
        }
    }

    /// Dragging above/below the text area scrolls the view (one line per
    /// mouse event) so a selection can reach into the scrollback.
    fn autoscroll_selection(&mut self, y: f64) {
        if !self.selection.is_active() || self.term.screen() == Screen::Alt {
            return;
        }
        let (_, ch) = self.renderer.cell_size_px();
        let top = self.renderer.grid_origin_px().1 as f64;
        let bottom = top + self.term.rows() as f64 * ch as f64;
        let delta = if y < top {
            1
        } else if y >= bottom {
            -1
        } else {
            return;
        };
        if self.term.scroll_display(delta) {
            self.renderer.invalidate();
            self.window.request_redraw();
        }
    }

    fn on_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let btn = match button {
            MouseButton::Left => MouseBtn::Left,
            MouseButton::Middle => MouseBtn::Middle,
            MouseButton::Right => MouseBtn::Right,
            _ => return,
        };
        let pressed = state == ElementState::Pressed;
        // Right click opens the context menu unless the program is using the
        // mouse (Shift always bypasses reporting, so Shift+right click opens it).
        if button == MouseButton::Right && (self.term.mouse_mode() == grimalkin_vt::MouseMode::None || self.mods.shift_key()) {
            if pressed {
                self.context_menu_requested = true;
            }
            return;
        }
        if button == MouseButton::Left && self.skin_mouse_button(pressed) {
            return;
        }
        if button == MouseButton::Left {
            self.left_down = pressed;
        }
        let ev = self.mouse_event(if pressed { MouseKind::Press } else { MouseKind::Release }, btn);
        if self.report_mouse(&ev) {
            return;
        }
        if pressed && button == MouseButton::Left && self.try_open_link() {
            return;
        }
        #[cfg(target_os = "linux")]
        if pressed && button == MouseButton::Middle {
            self.paste_primary();
            return;
        }
        let pos = self.cell_at(self.mouse_px);
        let changed = if pressed {
            self.selection.on_press(&self.term, pos, button, self.mods, Instant::now())
        } else {
            self.selection.on_release(&self.term, pos, button)
        };
        #[cfg(target_os = "linux")]
        if !pressed && self.selection.take_completed() {
            self.copy_primary();
        }
        if changed {
            self.renderer.invalidate();
            self.window.request_redraw();
        }
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let (_, ch) = self.renderer.cell_size_px();
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y as f64 * WHEEL_LINES_PER_NOTCH,
            MouseScrollDelta::PixelDelta(p) => p.y / ch as f64,
        };
        self.wheel_accum += lines;
        let whole = self.wheel_accum.trunc();
        if whole == 0.0 {
            return;
        }
        self.wheel_accum -= whole;
        let n = whole as i32;

        // Mouse reporting gets first refusal (one event per line).
        let btn = if n > 0 { MouseBtn::WheelUp } else { MouseBtn::WheelDown };
        let ev = self.mouse_event(MouseKind::Wheel, btn);
        let mut reported = false;
        for _ in 0..n.unsigned_abs() {
            reported |= self.report_mouse(&ev);
        }
        if reported {
            return;
        }
        if self.term.mouse_mode() != grimalkin_vt::MouseMode::None && !self.mods.shift_key() {
            // Mouse tracking is on but reporting declined: do nothing rather
            // than scroll underneath the application.
            return;
        }
        if self.term.screen() == Screen::Alt {
            let bytes = input::wheel_arrows(n, self.term.app_cursor_keys());
            self.send(&bytes);
        } else if self.term.scroll_display(n) {
            self.window.request_redraw();
        }
    }
}

#[cfg(test)]
mod skin_palette_tests {
    use super::*;
    use crate::config::Rgb;

    fn palette(cfg: &Config, skin: &Skin) -> theme::Palette {
        skin_palette(theme::resolve_in(cfg, None).palette, cfg, Some(skin))
    }

    #[test]
    fn skin_background_yields_to_theme_and_user_colors() {
        let skin = Skin::load_from("grimalkin", None).expect("built-in skin");
        let want = skin.def.frame.viewport_background.expect("grimalkin sets a viewport background");

        // No theme and no [colors] background: the skin's viewport colour.
        let mut cfg = Config::default();
        assert_eq!(palette(&cfg, &skin).background, want);
        // No skin: the theme's own background.
        let plain = skin_palette(theme::resolve_in(&cfg, None).palette, &cfg, None);
        assert_ne!(plain.background, want);

        // An explicit theme wins over the skin.
        cfg.theme = "dracula".into();
        assert_ne!(palette(&cfg, &skin).background, want);

        // So does an explicit [colors] background.
        let mut cfg = Config::default();
        cfg.colors.background = Some(Rgb(1, 2, 3));
        assert_eq!(palette(&cfg, &skin).background, Rgb(1, 2, 3));
    }
}
