//! `vte::Perform` implementation: C0, ESC, CSI, OSC dispatch.

use crate::cell::*;
use crate::term::{KB_STACK_LIMIT, *};
use crate::util;
use vte::{Params, Perform};

fn arg(ps: &[u16], i: usize, default: usize) -> usize {
    match ps.get(i) {
        Some(&v) if v != 0 => v as usize,
        _ => default,
    }
}
fn arg0(ps: &[u16], i: usize) -> usize {
    ps.get(i).copied().unwrap_or(0) as usize
}

impl Perform for Terminal {
    fn print(&mut self, c: char) {
        self.print_char(c);
    }

    fn execute(&mut self, byte: u8) {
        self.last_cell = None;
        match byte {
            0x07 => self.events.push(TermEvent::Bell),
            0x08 => {
                self.pending_wrap = false;
                self.cursor.col = self.cursor.col.saturating_sub(1);
            }
            0x09 => {
                self.pending_wrap = false;
                self.cursor.col = self.next_tab(self.cursor.col);
            }
            0x0a..=0x0c => self.linefeed(),
            0x0d => {
                self.pending_wrap = false;
                self.cursor.col = 0;
            }
            0x0e => self.gl = 1,
            0x0f => self.gl = 0,
            _ => {}
        }
    }

    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {
        self.last_cell = None;
    }
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.last_cell = None;
        if ignore {
            return;
        }
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.cursor.col = 0;
                self.linefeed();
            }
            ([], b'M') => self.reverse_index(),
            ([], b'H') => {
                let c = self.cursor.col;
                self.tabs[c] = true;
            }
            ([], b'c') => self.full_reset(),
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([b'#'], b'8') => {
                // DECALN
                let style = Style::default();
                for r in 0..self.rows {
                    for c in 0..self.cols {
                        *self.grid_mut().cell_mut(r, c) =
                            Cell::with_style('E', &style, CellWidth::Single);
                    }
                    self.mark_row(r);
                }
                self.cursor = Cursor::default();
                self.pending_wrap = false;
                self.region_top = 0;
                self.region_bottom = self.rows - 1;
            }
            ([i @ (b'(' | b')' | b'*' | b'+')], f) => {
                let slot = match i {
                    b'(' => 0,
                    b')' => 1,
                    b'*' => 2,
                    _ => 3,
                };
                self.charsets[slot] = if f == b'0' {
                    Charset::DecGraphics
                } else {
                    Charset::Ascii
                };
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, inter: &[u8], ignore: bool, action: char) {
        self.last_cell = None;
        if ignore {
            return;
        }
        let ps: Vec<u16> = params.iter().map(|p| p.first().copied().unwrap_or(0)).collect();
        if !matches!(action, 'm' | 'h' | 'l' | 'n' | 'c' | 't' | 'q' | 'u' | 'p' | 'b') {
            self.pending_wrap = false;
        }
        let rows = self.rows;
        let cols = self.cols;
        match (inter, action) {
            ([], 'A') => {
                let n = arg(&ps, 0, 1);
                let top = if self.cursor.row >= self.region_top {
                    self.region_top
                } else {
                    0
                };
                self.cursor.row = self.cursor.row.saturating_sub(n).max(top);
            }
            ([], 'B' | 'e') => {
                let n = arg(&ps, 0, 1);
                let bottom = if self.cursor.row <= self.region_bottom {
                    self.region_bottom
                } else {
                    rows - 1
                };
                self.cursor.row = (self.cursor.row + n).min(bottom);
            }
            ([], 'C' | 'a') => {
                self.cursor.col = (self.cursor.col + arg(&ps, 0, 1)).min(cols - 1);
            }
            ([], 'D') => {
                self.cursor.col = self.cursor.col.saturating_sub(arg(&ps, 0, 1));
            }
            ([], 'E') => {
                let n = arg(&ps, 0, 1);
                let bottom = if self.cursor.row <= self.region_bottom {
                    self.region_bottom
                } else {
                    rows - 1
                };
                self.cursor.row = (self.cursor.row + n).min(bottom);
                self.cursor.col = 0;
            }
            ([], 'F') => {
                let n = arg(&ps, 0, 1);
                let top = if self.cursor.row >= self.region_top {
                    self.region_top
                } else {
                    0
                };
                self.cursor.row = self.cursor.row.saturating_sub(n).max(top);
                self.cursor.col = 0;
            }
            ([], 'G' | '`') => {
                self.cursor.col = (arg(&ps, 0, 1) - 1).min(cols - 1);
            }
            ([], 'H' | 'f') => {
                self.set_cursor_pos(arg(&ps, 0, 1) - 1, arg(&ps, 1, 1) - 1);
            }
            ([], 'd') => {
                let col = self.cursor.col;
                self.set_cursor_pos(arg(&ps, 0, 1) - 1, col);
            }
            ([], 'I') => {
                for _ in 0..arg(&ps, 0, 1) {
                    self.cursor.col = self.next_tab(self.cursor.col);
                }
            }
            ([], 'Z') => {
                for _ in 0..arg(&ps, 0, 1) {
                    self.cursor.col = self.prev_tab(self.cursor.col);
                }
            }
            ([], 'g') => match arg0(&ps, 0) {
                0 => {
                    let c = self.cursor.col;
                    self.tabs[c] = false;
                }
                3 => self.tabs.iter_mut().for_each(|t| *t = false),
                _ => {}
            },
            ([], 'J') => {
                let (r, c) = (self.cursor.row, self.cursor.col);
                match arg0(&ps, 0) {
                    0 => {
                        self.erase_cells(r, c, cols);
                        self.erase_rows(r + 1, rows);
                    }
                    1 => {
                        self.erase_rows(0, r);
                        self.erase_cells(r, 0, c + 1);
                    }
                    2 => self.erase_rows(0, rows),
                    3 => {
                        self.grid_mut().clear_scrollback();
                        self.scroll_display_to_bottom();
                        self.mark_all();
                    }
                    _ => {}
                }
            }
            ([], 'K') => {
                let (r, c) = (self.cursor.row, self.cursor.col);
                match arg0(&ps, 0) {
                    0 => self.erase_cells(r, c, cols),
                    1 => self.erase_cells(r, 0, c + 1),
                    2 => {
                        self.erase_cells(r, 0, cols);
                    }
                    _ => {}
                }
            }
            ([], 'X') => {
                let (r, c) = (self.cursor.row, self.cursor.col);
                self.erase_cells(r, c, c + arg(&ps, 0, 1));
            }
            ([], '@') => self.insert_chars(arg(&ps, 0, 1)),
            ([], 'P') => self.delete_chars(arg(&ps, 0, 1)),
            ([], 'L') => {
                let r = self.cursor.row;
                if r >= self.region_top && r <= self.region_bottom {
                    let bg = self.pen.bg;
                    let bottom = self.region_bottom;
                    self.grid_mut().insert_lines(r, bottom, arg(&ps, 0, 1), bg);
                    self.cursor.col = 0;
                    self.mark_rows(r, bottom);
                }
            }
            ([], 'M') => {
                let r = self.cursor.row;
                if r >= self.region_top && r <= self.region_bottom {
                    let bg = self.pen.bg;
                    let bottom = self.region_bottom;
                    self.grid_mut().delete_lines(r, bottom, arg(&ps, 0, 1), bg);
                    self.cursor.col = 0;
                    self.mark_rows(r, bottom);
                }
            }
            ([], 'S') => {
                let (t, b) = (self.region_top, self.region_bottom);
                self.scroll_up(t, b, arg(&ps, 0, 1));
            }
            ([], 'T') => {
                if ps.len() <= 1 {
                    let (t, b) = (self.region_top, self.region_bottom);
                    self.scroll_down(t, b, arg(&ps, 0, 1));
                }
            }
            ([], 'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..arg(&ps, 0, 1).min(65535) {
                        self.print_char(c);
                    }
                    self.last_cell = None;
                }
            }
            ([], 'r') => {
                let top = arg(&ps, 0, 1);
                // xterm/kitty clamp an oversized bottom margin (apps with a
                // stale idea of the window height) rather than ignoring it.
                let bot = arg(&ps, 1, rows).min(rows);
                if top < bot {
                    self.region_top = top - 1;
                    self.region_bottom = bot - 1;
                    self.set_cursor_pos(0, 0);
                }
            }
            ([], 's') => {
                if ps.is_empty() || (ps.len() == 1 && ps[0] == 0) {
                    self.save_cursor();
                }
            }
            ([], 'u') => self.restore_cursor(),
            ([], 'm') => self.sgr(params),
            ([], 'h') | ([], 'l') => {
                let on = action == 'h';
                for &p in &ps {
                    if p == 4 {
                        self.modes.insert = on;
                    }
                }
            }
            ([b'?'], 'h') | ([b'?'], 'l') => {
                let on = action == 'h';
                for &p in &ps {
                    self.set_private_mode(p, on);
                }
            }
            ([], 'n') => match arg0(&ps, 0) {
                5 => self.push_response(b"\x1b[0n"),
                6 => self.report_cursor(false),
                _ => {}
            },
            ([b'?'], 'n') => {
                if arg0(&ps, 0) == 6 {
                    self.report_cursor(true)
                }
            }
            ([], 'c') => {
                if arg0(&ps, 0) == 0 {
                    self.push_response(b"\x1b[?62;22c");
                }
            }
            ([b'>'], 'c') => {
                if arg0(&ps, 0) == 0 {
                    self.push_response(b"\x1b[>1;4000;0c");
                }
            }
            ([b'>'], 'q') => {
                let r = format!("\x1bP>|grimalkin {}\x1b\\", env!("CARGO_PKG_VERSION"));
                self.push_response(r.as_bytes());
            }
            ([b' '], 'q') => {
                let (shape, blink) = match arg0(&ps, 0) {
                    0 | 1 => (CursorShape::Block, true),
                    2 => (CursorShape::Block, false),
                    3 => (CursorShape::Underline, true),
                    4 => (CursorShape::Underline, false),
                    5 => (CursorShape::Bar, true),
                    6 => (CursorShape::Bar, false),
                    _ => return,
                };
                self.modes.cursor_blink = blink;
                if shape != self.cursor_shape {
                    self.cursor_shape = shape;
                }
                self.events.push(TermEvent::CursorShape(shape));
            }
            ([b'!'], 'p') => self.soft_reset(),
            ([b'?', b'$'], 'p') => {
                let m = arg0(&ps, 0);
                let state = match self.private_mode_state(m as u16) {
                    Some(true) => 1,
                    Some(false) => 2,
                    None => 0,
                };
                let r = format!("\x1b[?{};{}$y", m, state);
                self.push_response(r.as_bytes());
            }
            ([], 't') => {
                let (cw, ch) = self.cell_px;
                match arg0(&ps, 0) {
                    14 => {
                        let r = format!(
                            "\x1b[4;{};{}t",
                            ch as usize * self.rows,
                            cw as usize * self.cols
                        );
                        self.push_response(r.as_bytes());
                    }
                    16 => {
                        let r = format!("\x1b[6;{};{}t", ch, cw);
                        self.push_response(r.as_bytes());
                    }
                    18 => {
                        let r = format!("\x1b[8;{};{}t", self.rows, self.cols);
                        self.push_response(r.as_bytes());
                    }
                    _ => {}
                }
            }
            // Kitty keyboard protocol.
            ([b'>'], 'u') => {
                let flags = (arg0(&ps, 0) & 0x1f) as u8;
                let i = self.sidx();
                let kb = &mut self.kb[i];
                if kb.stack.len() >= KB_STACK_LIMIT {
                    kb.stack.remove(0);
                }
                kb.stack.push(kb.flags);
                kb.flags = flags;
            }
            ([b'<'], 'u') => {
                let n = arg(&ps, 0, 1);
                let i = self.sidx();
                let kb = &mut self.kb[i];
                for _ in 0..n {
                    match kb.stack.pop() {
                        Some(f) => kb.flags = f,
                        None => {
                            kb.flags = 0;
                            break;
                        }
                    }
                }
            }
            ([b'='], 'u') => {
                let flags = (arg0(&ps, 0) & 0x1f) as u8;
                let mode = arg(&ps, 1, 1);
                let i = self.sidx();
                let kb = &mut self.kb[i];
                match mode {
                    1 => kb.flags = flags,
                    2 => kb.flags |= flags,
                    3 => kb.flags &= !flags,
                    _ => {}
                }
            }
            ([b'?'], 'u') => {
                let r = format!("\x1b[?{}u", self.keyboard_flags());
                self.push_response(r.as_bytes());
            }
            _ => {}
        }
        self.clamp_cursor();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.last_cell = None;
        let Some(cmd) = params.first() else { return };
        let st: &str = if bell_terminated { "\x07" } else { "\x1b\\" };
        let rest_joined = |from: usize| -> Vec<u8> {
            let mut v = Vec::new();
            for (i, p) in params.iter().enumerate().skip(from) {
                if i > from {
                    v.push(b';');
                }
                v.extend_from_slice(p);
            }
            v
        };
        match *cmd {
            b"0" | b"2" => {
                let t = String::from_utf8_lossy(&rest_joined(1)).into_owned();
                self.title = t.clone();
                self.events.push(TermEvent::Title(t));
            }
            b"1" => {}
            b"4" => {
                let mut changed = false;
                let mut i = 1;
                while i + 1 < params.len() {
                    let idx = std::str::from_utf8(params[i])
                        .ok()
                        .and_then(|s| s.parse::<usize>().ok());
                    if let Some(idx) = idx.filter(|&x| x < 256) {
                        if params[i + 1] == b"?" {
                            let c = self.colors.palette[idx];
                            let r = format!("\x1b]4;{};{}{}", idx, util::format_color_spec(c), st);
                            self.push_response(r.as_bytes());
                        } else if let Some(c) = util::parse_color_spec(params[i + 1]) {
                            self.colors.palette[idx] = c;
                            changed = true;
                        }
                    }
                    i += 2;
                }
                if changed {
                    self.mark_all();
                    self.events.push(TermEvent::ColorChanged);
                }
            }
            b"104" => {
                if params.len() <= 1 {
                    for i in 0..256 {
                        self.reset_palette_entry(i);
                    }
                } else {
                    for p in &params[1..] {
                        if let Some(i) = std::str::from_utf8(p)
                            .ok()
                            .and_then(|s| s.parse::<usize>().ok())
                            .filter(|&i| i < 256)
                        {
                            self.reset_palette_entry(i);
                        }
                    }
                }
                self.mark_all();
                self.events.push(TermEvent::ColorChanged);
            }
            b"10" | b"11" | b"12" => {
                let first = match *cmd {
                    b"10" => 10u8,
                    b"11" => 11,
                    _ => 12,
                };
                let mut changed = false;
                for (k, spec) in params[1..].iter().enumerate() {
                    let which = first + k as u8;
                    if which > 12 {
                        break;
                    }
                    if *spec == b"?" {
                        let c = match which {
                            10 => self.colors.foreground,
                            11 => self.colors.background,
                            _ => self.colors.cursor.unwrap_or(self.colors.foreground),
                        };
                        let r = format!("\x1b]{};{}{}", which, util::format_color_spec(c), st);
                        self.push_response(r.as_bytes());
                    } else if let Some(c) = util::parse_color_spec(spec) {
                        match which {
                            10 => self.colors.foreground = c,
                            11 => self.colors.background = c,
                            _ => self.colors.cursor = Some(c),
                        }
                        changed = true;
                    }
                }
                if changed {
                    self.mark_all();
                    self.events.push(TermEvent::ColorChanged);
                }
            }
            b"110" | b"111" | b"112" => {
                let which = match *cmd {
                    b"110" => 10,
                    b"111" => 11,
                    _ => 12,
                };
                self.reset_dyn_color(which);
                self.mark_all();
                self.events.push(TermEvent::ColorChanged);
            }
            b"7" => {
                let uri = String::from_utf8_lossy(&rest_joined(1)).into_owned();
                let path = util::parse_cwd_uri(&uri);
                self.cwd = Some(path.clone());
                self.events.push(TermEvent::CwdChanged(path));
            }
            b"8" => {
                if params.len() < 3 {
                    return;
                }
                let uri = String::from_utf8_lossy(&rest_joined(2)).into_owned();
                if uri.is_empty() {
                    self.pen.hyperlink_id = None;
                } else {
                    let id = String::from_utf8_lossy(params[1])
                        .split(':')
                        .find_map(|kv| kv.strip_prefix("id=").map(|s| s.to_string()))
                        .filter(|s| !s.is_empty());
                    self.pen.hyperlink_id = self.intern_hyperlink(id, uri);
                }
            }
            b"52" => {
                if params.len() < 3 {
                    return;
                }
                let sel = String::from_utf8_lossy(params[1]).into_owned();
                self.clip_selection = if sel.is_empty() { "c".into() } else { sel };
                if params[2] == b"?" {
                    self.events.push(TermEvent::ClipboardQuery);
                } else if let Some(bytes) = util::base64_decode(params[2]) {
                    self.events.push(TermEvent::ClipboardSet(
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ));
                }
            }
            b"133" => {
                let Some(k) = params.get(1).and_then(|p| p.first()) else {
                    return;
                };
                let mark = match k {
                    b'A' => LineMark {
                        kind: MarkKind::PromptStart,
                        exit_code: None,
                    },
                    b'B' => LineMark {
                        kind: MarkKind::PromptEnd,
                        exit_code: None,
                    },
                    b'C' => LineMark {
                        kind: MarkKind::OutputStart,
                        exit_code: None,
                    },
                    b'D' => LineMark {
                        kind: MarkKind::CommandEnd,
                        exit_code: params
                            .get(2)
                            .and_then(|p| std::str::from_utf8(p).ok())
                            .and_then(|s| s.parse::<i32>().ok()),
                    },
                    _ => return,
                };
                self.add_mark(mark);
            }
            _ => {}
        }
    }
}

impl Terminal {
    fn report_cursor(&mut self, private: bool) {
        let row = if self.modes.origin {
            self.cursor.row.saturating_sub(self.region_top)
        } else {
            self.cursor.row
        } + 1;
        let col = self.cursor.col + 1;
        let r = if private {
            format!("\x1b[?{};{}R", row, col)
        } else {
            format!("\x1b[{};{}R", row, col)
        };
        self.push_response(r.as_bytes());
    }

    pub(crate) fn private_mode_state(&self, m: u16) -> Option<bool> {
        let md = &self.modes;
        Some(match m {
            1 => md.app_cursor,
            5 => md.reverse_video,
            6 => md.origin,
            7 => md.wrap,
            12 => md.cursor_blink,
            25 => md.cursor_visible,
            47 | 1047 | 1049 => self.active == Screen::Alt,
            1000 => md.mouse_mode == MouseMode::Press,
            1002 => md.mouse_mode == MouseMode::Drag,
            1003 => md.mouse_mode == MouseMode::Any,
            1004 => md.focus_reporting,
            1005 => md.mouse_encoding == MouseEncoding::Utf8,
            1006 => md.mouse_encoding == MouseEncoding::Sgr,
            1015 => md.mouse_encoding == MouseEncoding::Urxvt,
            2004 => md.bracketed_paste,
            2026 => md.synchronized_output,
            _ => return None,
        })
    }

    fn set_private_mode(&mut self, m: u16, on: bool) {
        let mut mouse_changed = false;
        match m {
            1 => self.modes.app_cursor = on,
            5 => {
                self.modes.reverse_video = on;
                self.mark_all();
            }
            6 => {
                self.modes.origin = on;
                self.set_cursor_pos(0, 0);
            }
            7 => {
                self.modes.wrap = on;
                if !on {
                    self.pending_wrap = false;
                }
            }
            12 => self.modes.cursor_blink = on,
            25 => self.modes.cursor_visible = on,
            47 | 1047 => {
                if on {
                    self.enter_alt(false)
                } else {
                    self.leave_alt(false)
                }
            }
            1048 => {
                if on {
                    self.save_cursor()
                } else {
                    self.restore_cursor()
                }
            }
            1049 => {
                if on {
                    self.enter_alt(true)
                } else {
                    self.leave_alt(true)
                }
            }
            1000 | 1002 | 1003 => {
                let new = match (on, m) {
                    (false, _) => MouseMode::None,
                    (true, 1000) => MouseMode::Press,
                    (true, 1002) => MouseMode::Drag,
                    (true, _) => MouseMode::Any,
                };
                if self.modes.mouse_mode != new {
                    self.modes.mouse_mode = new;
                    mouse_changed = true;
                }
            }
            1004 => {
                if self.modes.focus_reporting != on {
                    self.modes.focus_reporting = on;
                    mouse_changed = true;
                }
            }
            1005 | 1006 | 1015 => {
                let enc = match m {
                    1005 => MouseEncoding::Utf8,
                    1006 => MouseEncoding::Sgr,
                    _ => MouseEncoding::Urxvt,
                };
                let new = if on {
                    enc
                } else if self.modes.mouse_encoding == enc {
                    MouseEncoding::Default
                } else {
                    self.modes.mouse_encoding
                };
                if self.modes.mouse_encoding != new {
                    self.modes.mouse_encoding = new;
                    mouse_changed = true;
                }
            }
            2004 => self.modes.bracketed_paste = on,
            2026 => self.set_synchronized(on),
            _ => {}
        }
        if mouse_changed {
            self.events.push(TermEvent::SetMouseMode);
        }
    }

    fn sgr(&mut self, params: &Params) {
        let groups: Vec<&[u16]> = params.iter().collect();
        let mut i = 0;
        while i < groups.len() {
            let g = groups[i];
            let p = g[0];
            i += 1;
            match p {
                0 => {
                    let link = self.pen.hyperlink_id;
                    self.pen = Style::default();
                    self.pen.hyperlink_id = link;
                }
                1 => self.pen.flags.insert(Flags::BOLD),
                2 => self.pen.flags.insert(Flags::DIM),
                3 => self.pen.flags.insert(Flags::ITALIC),
                4 => {
                    self.pen.underline = match g.get(1) {
                        None => Underline::Single,
                        Some(0) => Underline::None,
                        Some(1) => Underline::Single,
                        Some(2) => Underline::Double,
                        Some(3) => Underline::Curly,
                        Some(4) => Underline::Dotted,
                        Some(5) => Underline::Dashed,
                        Some(_) => Underline::Single,
                    }
                }
                5 | 6 => self.pen.flags.insert(Flags::BLINK),
                7 => self.pen.flags.insert(Flags::INVERSE),
                8 => self.pen.flags.insert(Flags::HIDDEN),
                9 => self.pen.flags.insert(Flags::STRIKETHROUGH),
                21 => self.pen.underline = Underline::Double,
                22 => {
                    self.pen.flags.remove(Flags::BOLD);
                    self.pen.flags.remove(Flags::DIM);
                }
                23 => self.pen.flags.remove(Flags::ITALIC),
                24 => self.pen.underline = Underline::None,
                25 => self.pen.flags.remove(Flags::BLINK),
                27 => self.pen.flags.remove(Flags::INVERSE),
                28 => self.pen.flags.remove(Flags::HIDDEN),
                29 => self.pen.flags.remove(Flags::STRIKETHROUGH),
                30..=37 => self.pen.fg = Color::Indexed((p - 30) as u8),
                38 | 48 | 58 => {
                    let (color, used) = parse_ext_color(g, &groups[i..]);
                    i += used;
                    if let Some(c) = color {
                        match p {
                            38 => self.pen.fg = c,
                            48 => self.pen.bg = c,
                            _ => self.pen.underline_color = c,
                        }
                    }
                }
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((p - 40) as u8),
                49 => self.pen.bg = Color::Default,
                59 => self.pen.underline_color = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((p - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((p - 100 + 8) as u8),
                _ => {}
            }
        }
    }
}

/// Parse the argument of SGR 38/48/58. `g` is the group holding the 38 (may
/// contain colon sub-parameters); `rest` are the following groups. Returns
/// the colour and how many following groups were consumed.
fn parse_ext_color(g: &[u16], rest: &[&[u16]]) -> (Option<Color>, usize) {
    let b = |v: u16| v.min(255) as u8;
    if g.len() > 1 {
        // Colon form.
        return (
            match g[1] {
                5 => g.get(2).map(|&n| Color::Indexed(b(n))),
                2 => {
                    // 38:2:[cs]:r:g:b  or 38:2:r:g:b
                    let v = if g.len() >= 6 {
                        &g[3..6]
                    } else if g.len() == 5 {
                        &g[2..5]
                    } else {
                        &g[0..0]
                    };
                    if v.len() == 3 {
                        Some(Color::Rgb(b(v[0]), b(v[1]), b(v[2])))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            0,
        );
    }
    let first = |i: usize| rest.get(i).and_then(|g| g.first()).copied();
    match first(0) {
        Some(5) => match first(1) {
            Some(n) => (Some(Color::Indexed(b(n))), 2),
            None => (None, rest.len().min(1)),
        },
        Some(2) => match (first(1), first(2), first(3)) {
            (Some(r), Some(g), Some(bl)) => (Some(Color::Rgb(b(r), b(g), b(bl))), 4),
            _ => (None, rest.len().min(4)),
        },
        Some(_) => (None, 1),
        None => (None, 0),
    }
}
