# Grimalkin architecture

Grimalkin is a GPU-accelerated terminal emulator written in Rust. Primary target
is **Windows 10/11** (ConPTY); it must also build and run on macOS/Linux because
that is where it is developed and tested. Feature goal: parity with kitty for
daily use, including the **kitty graphics protocol** and **kitty keyboard
protocol**.

Rust edition 2021, toolchain 1.92 (no rustup on the dev Mac; do not rely on
nightly). Workspace with these crates:

```
grimalkin/
  Cargo.toml            # [workspace] members = crates/*
  crates/
    vt/                 # grimalkin-vt: terminal state machine (no I/O, no GUI)
    graphics/           # grimalkin-graphics: kitty graphics protocol + image store
    pty/                # grimalkin-pty: native ConPTY (Windows) / portable-pty forkpty (Unix)
    gui/                # grimalkin (binary): winit + wgpu renderer, input, config
  ARCHITECTURE.md
```

Crate names in Cargo are `grimalkin-vt`, `grimalkin-graphics`, `grimalkin-pty`,
`grimalkin` (bin). Dependency direction is strictly `gui -> {vt, graphics, pty}`.
`vt`, `graphics` and `pty` depend on nothing in-workspace, so they can be built
and tested independently. The gui crate glues vt and graphics together (see
"Graphics glue" below).

## grimalkin-vt (crates/vt)

* Parser: the `vte` crate (0.15). Implement `vte::Perform` on a `Terminal`.
* `Terminal::new(cols, rows, scrollback_limit)`; `advance(&mut self, bytes: &[u8])`
  feeds output; `drain_responses(&mut self) -> Vec<u8>` returns bytes the
  terminal wants written back to the pty (DA, DSR, kitty graphics responses,
  CPR, XTVERSION, etc).
* `Grid` of `Cell { ch: char + optional zero-width combining Vec, fg, bg,
  flags(bold, dim, italic, underline{single,double,curly,dotted,dashed},
  strikethrough, inverse, hidden, blink), underline_color, hyperlink_id:
  Option<u32>, wide/wide-spacer }`. Colors: `Color::{Default, Indexed(u8),
  Rgb(u8,u8,u8)}`.
* Primary + alternate screen, scrollback ring buffer (primary only), scroll
  region (DECSTBM), left/right margins optional (DECLRMM) - fine to omit.
* Must support: CR/LF/BS/TAB/BEL, CUU/CUD/CUF/CUB/CUP/HVP/CHA/VPA, ED/EL/ECH/
  ICH/DCH/IL/DL/SU/SD, DECSC/DECRC, DECSTBM, tab stops (HTS/TBC/CBT), SGR
  (0-9, 21-29, 30-37, 38;5, 38;2, 39, 40-49, 58/59 underline color, 90-97,
  100-107, 4:x underline styles), DECSET/DECRST modes: 1 (DECCKM), 7 (wrap),
  12 (cursor blink), 25 (cursor visible), 1000/1002/1003/1006 (mouse, SGR),
  1004 (focus), 1047/1049 (alt screen), 2004 (bracketed paste), 2026
  (synchronized output: buffer updates until end), OSC 0/2 (title), OSC 4/10/
  11/12 (palette/fg/bg/cursor query+set), OSC 7 (cwd), OSC 8 (hyperlinks),
  OSC 52 (clipboard set/query - return via an event), OSC 133 (shell
  integration marks A/B/C/D stored as line marks), DA1/DA2 responses (report
  as xterm-compatible), DSR 5/6, CSI > q (XTVERSION) answering
  `grimalkin <ver>`, DECSCUSR cursor shape, window ops 14/16/18 (pixel size
  report - needs cell pixel size set via `set_cell_pixel_size`).
* **Kitty keyboard protocol**: CSI > flags u (push), CSI < u (pop), CSI = flags;
  mode u (set), CSI ? u (query -> respond CSI ? flags u). Store a stack per
  screen (main/alt). Expose `keyboard_flags() -> u8`. Encoding of key events
  happens in the gui crate's `input.rs`, which reads these flags.
* Wide chars via `unicode-width`; graphemes via `unicode-segmentation`.
* Damage tracking: per-row dirty flags + "full redraw" flag so the renderer only
  uploads changed rows.
* Events: `Terminal::drain_events() -> Vec<TermEvent>` with variants
  `Title(String)`, `Bell`, `ClipboardSet(String)`, `ClipboardQuery`,
  `CwdChanged(String)`, `ColorChanged`, `CursorShape(..)`, `SetMouseMode`.
* APC: `\x1b_G ... \x1b\\` is NOT interpreted by vt. vt emits
  `TermEvent::GraphicsCommand(Vec<u8>)` (payload = bytes between `\x1b_G` and
  the terminator; vte's `Perform::apc_dispatch`/hook may need the `apc`
  handling - vte 0.15 delivers APC through `apc_dispatch`? check; if vte has
  no APC callback, treat `\x1b_` via `esc_dispatch` + manual accumulation
  until ST). vt exposes everything the glue needs: `cursor() -> Cursor {col,
  row}`, `move_cursor_rel(dcols: i32, drows: i32)` (with wrapping/scrolling
  like printing would), `push_response(&[u8])`, `screen() -> Screen::{Main,
  Alt}`, `cursor_abs_row() -> u64` (row index in an absolute, monotonically
  increasing line space: `scrolled_lines_total + cursor.row`; separate counter
  for alt screen that resets when alt is entered), `display_offset() -> usize`
  (scrollback view offset), `abs_row_of_visible_row(r) -> u64`, `size()`.
* **Reflow** (`reflow.rs`): a column change on the primary screen joins each
  soft-wrapped logical line (`Row::wrapped`) across scrollback + visible rows,
  trims trailing default blanks, and re-splits it at the new width (wide chars
  are never split: a `WRAP_PAD` blank is left at the row end, like the writer).
  The cursor and DECSC position follow their character; the scrollback limit is
  honoured; cost is O(cells). The alternate screen only truncates/extends.
  Absolute rows are *not* stable across a reflow (`cursor_abs_row` stays
  monotonic), so vt queues `TermEvent::Reflowed`: the gui clears the selection
  and graphics_glue drops the main screen's image placements (image data is
  kept, as kitty does). Row-only resizes keep the old top-anchored behaviour.
* Unit tests: a test helper feeds bytes and snapshot-compares the grid as text.
  Test every CSI listed above, alt screen, scrollback, wrapping of wide chars at
  the right edge, SGR 38;2, kitty keyboard stack push/pop, OSC 8.

## grimalkin-graphics (crates/graphics)

Implements https://sw.kovidgoyal.net/kitty/graphics-protocol/ (read it in full
with WebFetch before coding; it is the source of truth).

* `Protocol::handle(&mut self, payload: &[u8], ctx: &CursorCtx) -> Vec<Action>`
  where `payload` is the bytes between `\x1b_G` and `\x1b\\` (control data
  `k=v,k=v;base64payload`). `CursorCtx { col: u16, abs_row: u64, cols: u16,
  rows: u16, cell_w_px: u32, cell_h_px: u32, screen: Screen }` with
  `pub enum Screen { Main, Alt }` defined in this crate (vt has its own; gui
  maps). `pub enum Action { Respond(Vec<u8>), MoveCursor { dcols: i32, drows:
  i32 }, Dirty }`.
* Control keys to support: `a` (t transmit, T transmit+display, p put, d
  delete, q query, f frame-animate: accept but may be a no-op in v1), `f`
  (24, 32, 100=PNG), `t` (d direct, f file, t temp file, s shared memory -
  file/temp are read from disk, s may be rejected with EINVAL on Windows),
  `s`/`v` width/height, `S`/`O` size/offset, `i` image id, `I` image number,
  `p` placement id, `o=z` zlib compression (flate2), `m` more-chunks
  (accumulate chunks keyed by image id until m=0), `q` quiet levels 0/1/2,
  `x,y,w,h` source rect, `X,Y` cell offsets, `c,r` columns/rows to scale to,
  `z` z-index (negative = below text), `C` cursor movement policy,
  `U=1` unicode placeholders (virtual placements; v1 may store and expose but
  the renderer can defer), `d` delete modes: a/A, i/I, n/N, c/C, p/P, q/Q,
  x/X, y/Y, z/Z, and the uppercase variants freeing image data.
* Responses: `\x1b_Gi=<id>[,I=<num>][,p=<pid>];OK\x1b\\` or error codes
  (EINVAL, EBADF, ENOENT, ENODATA, EFBIG, ...). Respect `q`. Query (`a=q`)
  validates and responds without storing.
* Image store: `HashMap<u32, Image { id, number, width, height, rgba: Arc<[u8]>,
  texture_version }>`; placements: `Vec<Placement { image_id, placement_id,
  screen, col, abs_row, z, cols, rows (cells covered, computed from c/r or
  from pixel size / cell size), src_rect, pixel_offset(X,Y) }>`.
  Expose `placements_in(screen, abs_row_range) -> impl Iterator<&Placement>`,
  `image(id) -> Option<&Image>`, `gc_below(screen, abs_row)`,
  `clear_screen(screen)`, and `take_dirty_images() -> Vec<u32>` so the renderer
  knows which textures to (re)upload. Enforce a memory limit (default 320 MiB,
  evict oldest).
* Actions returned to vt: `Respond(Vec<u8>)`, `MoveCursor { cols, rows }`,
  `Dirty`.
* Must decode PNG via `image` crate (png feature only to keep build small).
* Unit tests: chunked transmission, zlib, PNG, put by id, delete variants,
  query, error responses, malformed input never panics (fuzz-style loop over
  random bytes).

### Graphics glue (lives in gui, `graphics_glue.rs`)

On `TermEvent::GraphicsCommand(payload)` the gui builds
`CursorCtx { col, abs_row, cols, rows, cell_w_px, cell_h_px, screen }` from vt,
calls `Protocol::handle(&payload, &ctx)`, and applies the returned actions:
`Respond(bytes)` -> `term.push_response`, `MoveCursor{dcols,drows}` ->
`term.move_cursor_rel`, `Dirty` -> request redraw. Placements are stored by
`(screen, abs_row, col)`, so they scroll with text for free; the renderer asks
`protocol.placements_in(screen, abs_row_top..=abs_row_bottom)` each frame and
converts abs rows to screen rows via `display_offset`. Placements whose
abs_row falls below `abs_row_top - scrollback_limit` are garbage-collected by
calling `protocol.gc_below(screen, abs_row)`. Entering/leaving alt screen keeps
main-screen placements intact; `protocol.clear_screen(Screen::Alt)` on alt exit.

## grimalkin-pty (crates/pty)

* `Pty::spawn(cmd: &str, args, cwd, env, cols, rows) -> Pty` using
  `portable_pty` (`native_pty_system()`). On Windows the default shell is
  `pwsh.exe` if on PATH else `powershell.exe` else `cmd.exe`; set env
  `TERM=xterm-kitty`, `COLORTERM=truecolor`, `TERM_PROGRAM=grimalkin`.
  On Unix default shell is `$SHELL` or `/bin/sh`.
* Backends: Unix uses `portable_pty` (`src/unix.rs`). Windows uses a native
  ConPTY implementation on `windows-sys` (`src/win/conpty.rs`) so we control the
  pseudoconsole flags (`SpawnConfig::passthrough` -> `PSEUDOCONSOLE_PASSTHROUGH_MODE`)
  and can sideload `conpty.dll` + `OpenConsole.exe` from the exe directory
  (stock Windows 10 ConPTY strips APC, i.e. kitty graphics). Pure command-line /
  environment-block helpers live in `src/win/cmdline.rs` and are unit-tested
  on every platform. lib.rs keeps the shared reader/writer/wait-thread logic.
* Reader thread sends `Vec<u8>` chunks over an `std::sync::mpsc` channel (or
  a callback) and the caller gets `Box<dyn Write + Send>` for input.
* `resize(cols, rows, pixel_w, pixel_h)`, `kill()`, child-exit detection.

## grimalkin (crates/gui, binary)

* `winit` 0.30 + `wgpu` (latest compatible with winit 0.30's raw-window-handle
  0.6). Single window, resizable; HiDPI aware.
* Text: `cosmic-text` for shaping + font fallback (emoji, CJK, Nerd Font
  glyphs); glyph atlas texture (R8 for mask glyphs, RGBA8 for color emoji).
  Instanced quad pipeline: pass 1 backgrounds, pass 2 images with z<0,
  pass 3 glyphs + decorations (underline styles, strikethrough, cursor),
  pass 4 images with z>=0.
* Config: `~/.config/grimalkin/grimalkin.toml` on Unix,
  `%APPDATA%\grimalkin\grimalkin.toml` on Windows: font family, size,
  line height, colors (16-color palette + fg/bg/cursor/selection), cursor
  shape/blink, scrollback lines, shell, padding, opacity. Hot-reload optional.
* Input: `input.rs` encodes winit key events to bytes, honoring kitty
  keyboard protocol flags from vt (disambiguate, report event types, report
  alternate keys, report all keys as escape codes, report associated text),
  legacy xterm encoding otherwise, DECCKM, bracketed paste, mouse modes with
  SGR encoding, scroll wheel -> scrollback when not in alt screen (and arrow
  keys when in alt screen with mouse tracking off).
* Selection: click-drag, double-click word, triple-click line; copy to
  clipboard (`arboard`); paste with Ctrl+Shift+V / Cmd+V; Ctrl+Shift+C copies.
  Ctrl+Shift+= / - / 0 zoom. Hyperlinks: Ctrl+click opens via `open` crate.
* Render loop: redraw on terminal dirty or cursor blink timer, vsync, never
  busy-loop. Honors mode 2026 synchronized output.
* Windows specifics: `#![windows_subsystem = "windows"]` on release builds,
  embed an icon via `winres`/`embed-resource` if an icon exists, DPI awareness
  handled by winit.

## Testing strategy

* `cargo test --workspace` must pass on macOS.
* Manual: run the binary on macOS, run `kitten icat some.png`, `kitten icat
  --place`, a PNG with transparency, a large image (chunked), `vim`, `htop`,
  `less`, colors test scripts. Keep a `scripts/` directory of such checks.
* CI: `.github/workflows/ci.yml` builds and tests on windows-latest,
  macos-latest, ubuntu-latest and uploads the Windows .exe as an artifact.
