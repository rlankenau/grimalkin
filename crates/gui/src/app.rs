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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grimalkin_pty::{Pty, PtyEvent, PtyWriter, SpawnConfig};
use grimalkin_vt::{CursorShape, Screen, TermEvent, Terminal};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{UserAttentionType, Window, WindowId};

use crate::config::{Config, CursorShapeCfg};
use crate::graphics_glue::Graphics;
use crate::input::{self, EncodeState, KeyAction, KeyInput, MouseBtn, MouseEvent, MouseKind, ZoomOp};
use crate::renderer::{FrameStatus, Renderer, ViewState};
use crate::selection::{CellPos, Selection};
use crate::LaunchOptions;

/// Events delivered to the loop through its `EventLoopProxy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEvent {
    /// The pty produced output or exited.
    PtyWake,
    /// Something outside the loop (another thread, a timer) wants a redraw.
    #[allow(dead_code)] // sent by future background work (image decoding, ...)
    Redraw,
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
    fatal: Option<String>,
}

impl App {
    pub fn new(cfg: Config, launch: LaunchOptions, proxy: EventLoopProxy<UserEvent>) -> App {
        App {
            cfg,
            launch,
            proxy,
            wake_pending: Arc::new(AtomicBool::new(false)),
            running: None,
            fatal: None,
        }
    }

    /// Set if startup failed (GPU, pty, ...).
    pub fn fatal_error(&self) -> Option<&str> {
        self.fatal.as_deref()
    }

    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<Running, String> {
        let cfg = &self.cfg;
        let mut attrs = Window::default_attributes()
            .with_title("grimalkin")
            .with_inner_size(LogicalSize::new(900.0, 600.0))
            .with_visible(false);
        if cfg.window.opacity < 1.0 {
            attrs = attrs.with_transparent(true);
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
        // Size the window for the configured grid now that the cell size is known.
        let (w, h) = renderer.surface_size_for(cfg.window.cols as usize, cfg.window.rows as usize);
        if let Some(size) = window.request_inner_size(PhysicalSize::new(w, h)) {
            renderer.resize(size.width, size.height);
        } else {
            let s = window.inner_size();
            renderer.resize(s.width, s.height);
        }
        let (cols, rows) = renderer.grid_size();

        let mut term = Terminal::new(cols, rows, cfg.scrollback_lines);
        let fg = cfg.colors.foreground.tuple();
        let bg = cfg.colors.background.tuple();
        term.set_default_colors(fg, bg, Some(cfg.colors.cursor.tuple()));
        for (i, c) in cfg.colors.palette16().iter().enumerate() {
            term.set_palette_color(i as u8, c.tuple());
        }
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
            cmd,
            args,
            cwd: self.launch.working_directory.clone(),
            env: Vec::new(),
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
        Ok(Running {
            window,
            renderer,
            term,
            pty,
            rx,
            writer,
            gfx: Graphics::new(),
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
            cursor_shape_event: None,
            blink_epoch: Instant::now(),
            blink_deadline: None,
            sync_poll: None,
            redraw_retry: None,
            cfg: cfg.clone(),
            proxy: self.proxy.clone(),
            wake_pending: self.wake_pending.clone(),
            exit_requested: false,
        })
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(mut r) => {
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
        let Some(r) = self.running.as_mut() else { return };
        match event {
            UserEvent::PtyWake => r.on_pty_wake(),
            UserEvent::Redraw => r.window.request_redraw(),
        }
        if r.exit_requested {
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = self.running.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                r.renderer.resize(size.width, size.height);
                r.apply_grid_resize();
                r.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                r.renderer.set_scale_factor(scale_factor, r.cfg.window.padding);
                let s = r.window.inner_size();
                r.renderer.resize(s.width, s.height);
                r.apply_grid_resize();
                r.window.request_redraw();
            }
            WindowEvent::RedrawRequested => r.redraw(),
            WindowEvent::Occluded(false) => r.window.request_redraw(),
            WindowEvent::Focused(f) => r.on_focus(f),
            WindowEvent::ModifiersChanged(m) => r.mods = m.state(),
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
    /// Cursor shape last set by the program (DECSCUSR); `None` = config default.
    cursor_shape_event: Option<CursorShape>,
    blink_epoch: Instant,
    blink_deadline: Option<Instant>,
    sync_poll: Option<Instant>,
    redraw_retry: Option<Instant>,
    cfg: Config,
    proxy: EventLoopProxy<UserEvent>,
    wake_pending: Arc<AtomicBool>,
    exit_requested: bool,
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
                TermEvent::Title(t) => self.window.set_title(if t.is_empty() { "grimalkin" } else { &t }),
                TermEvent::Bell => {
                    if !self.focused {
                        self.window.request_user_attention(Some(UserAttentionType::Informational));
                    }
                }
                TermEvent::ClipboardSet(text) => {
                    if let Some(cb) = self.clipboard() {
                        if let Err(e) = cb.set_text(text) {
                            log::warn!("clipboard set failed: {e}");
                        }
                    }
                }
                TermEvent::ClipboardQuery => {
                    let text = self.clipboard().and_then(|cb| cb.get_text().ok()).unwrap_or_default();
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
            let _ = self.pty.resize(
                cols as u16,
                rows as u16,
                (cols as u32 * cw).min(u16::MAX as u32) as u16,
                (rows as u32 * ch).min(u16::MAX as u32) as u16,
            );
        }
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
        let view = ViewState {
            focused: self.focused,
            cursor_on: self.blink_on(now),
            cursor_shape: shape,
            selection: &self.selection,
        };
        let status = self.renderer.render(&mut self.term, &view, self.gfx.protocol_mut());
        log::debug!("frame status: {status:?}");
        match status {
            FrameStatus::Retry => self.window.request_redraw(),
            FrameStatus::Timeout => self.redraw_retry = Some(now + FRAME_RETRY),
            // Occluded: wait for WindowEvent::Occluded(false).
            FrameStatus::Presented | FrameStatus::Occluded => {}
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
            KeyAction::Copy => self.copy(),
            KeyAction::Paste => self.paste(),
            KeyAction::Zoom(op) => self.zoom(op),
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
            if let Some(cb) = self.clipboard() {
                if let Err(e) = cb.set_text(text) {
                    log::warn!("clipboard set failed: {e}");
                }
            }
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

    // -- mouse -------------------------------------------------------------

    fn cell_at(&self, px: (f64, f64)) -> CellPos {
        let (cw, ch) = self.renderer.cell_size_px();
        let pad = self.renderer.padding_px() as f64;
        let (cols, rows) = self.term.size();
        let col = (((px.0 - pad) / cw as f64).floor().max(0.0) as usize).min(cols.saturating_sub(1));
        let row = (((px.1 - pad) / ch as f64).floor().max(0.0) as usize).min(rows.saturating_sub(1));
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
        let btn = if self.left_down { MouseBtn::Left } else { MouseBtn::None };
        let ev = self.mouse_event(MouseKind::Motion, btn);
        if self.report_mouse(&ev) {
            return;
        }
        if self.left_down {
            let pos = self.cell_at(self.mouse_px);
            // Selection hook (no-op until the selection agent fills it in).
            if self.selection.on_drag(&self.term, pos) {
                self.renderer.invalidate();
                self.window.request_redraw();
            }
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
        if button == MouseButton::Left {
            self.left_down = pressed;
        }
        let ev = self.mouse_event(if pressed { MouseKind::Press } else { MouseKind::Release }, btn);
        if self.report_mouse(&ev) {
            return;
        }
        let pos = self.cell_at(self.mouse_px);
        // Selection hooks (no-ops until the selection agent fills them in).
        let changed = if pressed {
            self.selection.on_press(&self.term, pos, button, self.mods, Instant::now())
        } else {
            self.selection.on_release(&self.term, pos, button)
        };
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
