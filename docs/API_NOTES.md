# Library crate API notes (generated after phase 1)

## vt

```
crate grimalkin-vt (no I/O, no GUI). Re-exports: cell::{default_palette_rgb, Cell, CellWidth, Color, Flags, LineMark, MarkKind, Row, Style, Underline}; grid::Grid; term::{ColorTable, Cursor, CursorShape, Damage, Direction, Hyperlink, Modes, MouseEncoding, MouseMode, Rgb, Screen, TermEvent, Terminal}. util (base64/color-spec helpers) is private.

== cell ==
enum Color { Default, Indexed(u8), Rgb(u8,u8,u8) }  [Copy,Eq,Hash,Default]
enum Underline { None, Single, Double, Curly, Dotted, Dashed }
struct Flags(pub u16)  consts BOLD DIM ITALIC STRIKETHROUGH INVERSE HIDDEN BLINK; const fn empty()->Flags; const fn contains(self,Flags)->bool; insert(&mut self,Flags); remove(&mut self,Flags); set(&mut self,Flags,bool); const fn is_empty(self)->bool; impl BitOr
enum CellWidth { Single, Wide, WideSpacer }
struct Style { pub fg:Color, pub bg:Color, pub flags:Flags, pub underline:Underline, pub underline_color:Color, pub hyperlink_id:Option<u32> }  [Copy,Default]
struct Cell { pub ch:char, pub combining:Vec<char>, pub fg:Color, pub bg:Color, pub flags:Flags, pub underline:Underline, pub underline_color:Color, pub hyperlink_id:Option<u32>, pub width:CellWidth }  [Default]
  impl Cell: const fn blank()->Cell; erased(bg:Color)->Cell; with_style(ch:char,&Style,CellWidth)->Cell; style(&self)->Style; is_wide(&self)->bool; is_spacer(&self)->bool; is_empty(&self)->bool; push_text(&self,&mut String); bold(&self)->bool; inverse(&self)->bool
enum MarkKind { PromptStart, PromptEnd, OutputStart, CommandEnd }  (OSC 133 A/B/C/D)
struct LineMark { pub kind:MarkKind, pub exit_code:Option<i32> }
struct Row { pub cells:Vec<Cell>, pub wrapped:bool, pub marks:Vec<LineMark> }
  impl Row: new(cols:usize,bg:Color)->Row; len(&self)->usize; is_empty(&self)->bool; clear(&mut self,bg:Color); text(&self)->String; resize(&mut self,cols:usize); fix_wide(&mut self)
fn default_palette_rgb(idx:u8)->(u8,u8,u8)

== grid ==
struct Grid
  new(cols:usize,rows:usize,scrollback_limit:usize)->Grid; cols()->usize; rows()->usize; row(r)->&Row; row_mut(r)->&mut Row; cell(r,c)->&Cell; cell_mut(r,c)->&mut Cell; lines()->&[Row]; scrollback_len()->usize; scrollback_limit()->usize; scrollback_row(i:usize)->Option<&Row>; scrolled_total()->u64; clear(&mut self,bg:Color); scroll_up(&mut self,top,bottom,n,bg)->usize; scroll_down(&mut self,top,bottom,n,bg); insert_lines(at,bottom,n,bg); delete_lines(at,bottom,n,bg); resize(&mut self,cols,rows,cursor_row)->isize

== term ==
enum Screen { Main, Alt }
struct Cursor { pub col:usize, pub row:usize }  [Copy,Default]
enum CursorShape { Block, Underline, Bar }
enum MouseMode { None, Press(1000), Drag(1002), Any(1003) }
enum MouseEncoding { Default, Utf8, Sgr, Urxvt }
struct Modes { pub app_cursor, reverse_video, origin, wrap, cursor_blink, cursor_visible, insert: bool; pub mouse_mode:MouseMode; pub mouse_encoding:MouseEncoding; pub focus_reporting, bracketed_paste, synchronized_output, app_keypad: bool }
enum TermEvent { Title(String), Bell, ClipboardSet(String), ClipboardQuery, CwdChanged(String), ColorChanged, CursorShape(CursorShape), SetMouseMode, GraphicsCommand(Vec<u8>) }
type Rgb = (u8,u8,u8)
struct ColorTable { pub palette:[Rgb;256], pub foreground:Rgb, pub background:Rgb, pub cursor:Option<Rgb>, ..}  fn resolve(&self,c:Color,default:Rgb)->Rgb
enum Damage { None, Full, Rows(Vec<usize>) }
struct Hyperlink { pub id:Option<String>, pub uri:String }
enum Direction { Backward, Forward }
struct Terminal  (impl Display, Debug)
  new<C,R,S: TryInto<usize>>(cols:C,rows:R,scrollback_limit:S)->Terminal   (sizes clamped >=1)
  advance(&mut self,bytes:&[u8])
  drain_responses(&mut self)->Vec<u8>        // bytes to write back to the pty
  push_response(&mut self,&[u8])
  drain_events(&mut self)->Vec<TermEvent>
  respond_clipboard(&mut self,text:&str)
  size()->(usize,usize) [cols,rows]; cols(); rows(); cursor()->Cursor; screen()->Screen; grid()->&Grid; modes()->&Modes; colors()->&ColorTable; title()->&str; cwd()->Option<&str>
  cursor_shape()->CursorShape; cursor_visible(); cursor_blink(); mouse_mode()->MouseMode; mouse_encoding()->MouseEncoding; bracketed_paste(); focus_reporting(); app_cursor_keys(); synchronized_output()  (all bool)
  keyboard_flags()->u8                        // kitty keyboard protocol flags currently active
  cell_pixel_size()->(u32,u32); set_cell_pixel_size<W,H: TryInto<u32>>(&mut self,w,h)
  alt_enter_count()->u64
  hyperlink(id:u32)->Option<&Hyperlink>; hyperlink_uri(id:u32)->Option<&str>
  set_default_colors(&mut self,fg:Rgb,bg:Rgb,cursor:Option<Rgb>); set_palette_color(&mut self,idx:u8,rgb:Rgb)
  display_offset()->usize; scrollback_len()->usize; scroll_display(&mut self,delta:i32)->bool; scroll_display_to_bottom()->bool; scroll_display_to_top()->bool
  visible_row(r:usize)->&Row; visible_rows()->impl Iterator<Item=&Row>
  abs_row_of_visible_row(r)->u64; cursor_abs_row()->u64; oldest_abs_row()->u64; row_at_abs(abs:u64)->Option<&Row>
  marks_at(abs:u64)->&[LineMark]; line_marks()->Vec<(u64,LineMark)>; find_mark(kind:MarkKind,from_abs:u64,dir:Direction)->Option<u64>
  mark_all(&mut self)  // force full redraw
  is_dirty()->bool; dirty_rows()->Vec<usize>; needs_full_redraw()->bool; take_damage(&mut self)->Damage; clear_damage(&mut self)
  sync_elapsed()->Option<Duration>; check_sync_timeout(&mut self,Duration)->bool
  render_snapshot()->String; snapshot_lines()->Vec<String>
  resize<C,R: TryInto<usize>>(&mut self,cols:C,rows:R)
  move_cursor_rel(&mut self,dcols:i32,drows:i32)
```

Notes:
- Threading: Terminal is a plain owned struct with no internal locking or I/O. Keep it on one thread (or behind a Mutex) shared by the pty reader and the render thread. Do not call advance concurrently with rendering.
- Per-frame loop: after each pty read call advance(bytes); then write drain_responses() back to the pty (DA, DSR, CPR, kitty keyboard query replies and so on). Each frame, call drain_events() and handle each event, then take_damage(). Redraw per the result: Full means everything, Rows(v) means only those visible rows, None means skip.
- take_damage() returns None while synchronized output (mode 2026) is active, and it force-ends a stuck sync after 1s itself. Keep polling it on a timer even when no bytes arrive, otherwise a missing end-sync freezes the display. When the view is scrolled back (display_offset()>0) it always reports Full.
- Render from visible_rows() or visible_row(r), not grid().row(r). The visible_* calls honor the scrollback view offset. Skip cells where cell.is_spacer(). A Wide cell spans 2 columns. Draw combining chars from cell.combining. Resolve Color to RGB with colors().resolve(color, default_fg_or_bg). Handle the INVERSE, DIM, HIDDEN and BLINK flags yourself. Hyperlinks come from cell.hyperlink_id via hyperlink_uri(id).
- Kitty graphics: the VT crate only forwards raw APC G payloads as TermEvent::GraphicsCommand(Vec<u8>) and does not parse them or place images. The graphics crate must decode them. Placement needs set_cell_pixel_size(w,h) to be kept current, plus cursor() and cursor_abs_row(). Use the absolute row numbers (u64, stable across scrolling) to anchor images, and compare them against oldest_abs_row(). Replies to graphics commands go back through push_response(). Cursor movement after an image should use move_cursor_rel().
- Input: pick key encoding from keyboard_flags() (kitty keyboard protocol bits), app_cursor_keys(), modes().app_keypad and bracketed_paste(). Mouse reporting follows mouse_mode() and mouse_encoding(), and focus events follow focus_reporting(). On ClipboardQuery, answer with respond_clipboard(text).
- Resize: call resize(cols, rows) and tell the pty of the new size. Also call set_cell_pixel_size whenever the font metrics change. Resize does not reflow, and shrinking rows drops content below the cursor. The default colors and palette are set by the GUI via set_default_colors and set_palette_color (the theme), and ColorChanged events signal app-driven changes.
- Known gaps: no DECLRMM, DCS is ignored (no sixel, XTGETTCAP), U+009B is not treated as CSI, and BEL-terminated APC is not handled. A Cell is about 64 bytes, so large scrollback at wide columns is memory-heavy (10k lines at 200 columns is roughly 128 MB). No Windows build was attempted, but the crate has no OS-specific code.

## graphics

```
crate grimalkin_graphics (src/lib.rs; modules b64, cmd, loader are private)

const DEFAULT_MEMORY_LIMIT: usize = 320*1024*1024;

enum Screen { Main, Alt }  // Debug, Clone, Copy, PartialEq, Eq, Hash

struct CursorCtx { pub col: u16, pub abs_row: u64, pub cols: u16, pub rows: u16, pub cell_w_px: u32, pub cell_h_px: u32, pub screen: Screen }  // Debug, Clone, Copy

enum Action { Respond(Vec<u8>), MoveCursor { dcols: i32, drows: i32 }, Dirty }  // Debug, Clone, PartialEq, Eq

struct Image { pub id: u32, pub number: u32, pub width: u32, pub height: u32, pub rgba: Arc<[u8]>, pub texture_version: u64 }  // Debug, Clone; RGBA8

struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }  // source rect in pixels

struct Placement { pub image_id: u32, pub placement_id: u32, pub screen: Screen, pub col: u16, pub abs_row: u64, pub z: i32, pub cols: u32, pub rows: u32, pub src_rect: Rect, pub pixel_offset: (u32,u32), pub is_virtual: bool }
  impl Placement { pub fn last_row(&self) -> u64 }

struct Protocol  // impl Default
  impl Protocol {
    pub fn new() -> Self
    pub fn with_memory_limit(bytes: usize) -> Self
    pub fn set_memory_limit(&mut self, bytes: usize)
    pub fn memory_limit(&self) -> usize
    pub fn memory_used(&self) -> usize
    pub fn set_viewport_top(&mut self, screen: Screen, abs_row_top: u64)
    pub fn image(&self, id: u32) -> Option<&Image>
    pub fn image_count(&self) -> usize
    pub fn placement_count(&self) -> usize
    pub fn placements_in<R: RangeBounds<u64>>(&self, screen: Screen, range: R) -> impl Iterator<Item=&Placement> + '_
    pub fn placements_in_draw_order<R: RangeBounds<u64>>(&self, screen: Screen, range: R) -> Vec<&Placement>
    pub fn virtual_placements(&self, screen: Screen) -> impl Iterator<Item=&Placement> + '_
    pub fn take_dirty_images(&mut self) -> Vec<u32>
    pub fn take_removed_images(&mut self) -> Vec<u32>
    pub fn gc_below(&mut self, screen: Screen, abs_row: u64)
    pub fn clear_screen(&mut self, screen: Screen)
    pub fn handle(&mut self, payload: &[u8], ctx: &CursorCtx) -> Vec<Action>
  }
```

Notes:
- Protocol is a plain owned struct with &mut methods and no internal locking. Keep it on the terminal/parser thread, or wrap it in a Mutex if the renderer is on another thread. Image.rgba is Arc<[u8]>, so cloning it to hand to the renderer or GPU upload thread is cheap.
- handle() takes the bytes between ESC _ G and ESC \, without the G's delimiters. It returns Actions in order. Write Respond bytes back to the pty, apply MoveCursor relative to the cursor in the CursorCtx you passed, and redraw on Dirty. Build a fresh CursorCtx for every call.
- Call set_viewport_top(screen, abs_top_row) before EVERY handle call. It is persistent state, not part of CursorCtx. If you skip it or let it go stale after scrolling, visible-screen and cell-coordinate deletes (d=a/p/q/x/y) use the wrong viewport.
- Per frame: call take_removed_images() and drop those textures. Then call take_dirty_images(), look up each id with image(id), and upload or replace the texture, using texture_version to detect replacement. Then call placements_in_draw_order(screen, visible_abs_rows) and draw. Draw z<0 under the text and the rest over it. Removals and re-uploads are already deduplicated, so a re-uploaded id is not also in the removed list.
- placements_in does NOT return virtual (U=1) placements. Unicode-placeholder resolution is left to the renderer. Use virtual_placements() and match them against the placeholder cells yourself.
- Call gc_below(screen, oldest_retained_abs_row) when scrollback trims lines. Call clear_screen(screen) on the equivalent of RIS or alt-screen exit, so memory is freed.
- If both cols and rows are set on a Placement, the renderer decides between letterboxing and stretching. src_rect is in image pixels, and pixel_offset is the offset within the first cell.
- Not implemented, and they return errors to the client: relative placements (P/Q/H/V), animation (a=f/a/c), indexed color (f=8/16) and shared memory (t=s). Not compiled or tested on Windows. Memory is capped by DEFAULT_MEMORY_LIMIT (320 MiB) with eviction, and peak memory for a huge image is about 2x its size.

## pty

```
crate grimalkin_pty (src/lib.rs only; tests in tests/unix_spawn.rs)

pub const MAX_CHUNK: usize = 64 * 1024;
pub type WakeFn = Box<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent { Output(Vec<u8>) /* <= MAX_CHUNK */, Exited(Option<u32>) /* exactly once, last */ }

pub struct SpawnConfig {
  pub cmd: String,            // "" = default_shell()
  pub args: Vec<String>,      // excludes argv[0]
  pub cwd: Option<PathBuf>,
  pub env: Vec<(String,String)>, // overrides defaults
  pub cols: u16, pub rows: u16,
  pub pixel_width: u16, pub pixel_height: u16, // 0 = unknown
  pub login_shell: bool,      // Unix only
  pub tx: Sender<PtyEvent>,
  pub on_wake: Option<WakeFn>,
}

#[derive(Clone)] pub struct PtyWriter  // Debug, io::Write
impl PtyWriter {
  pub fn write_all(&self, buf: &[u8]) -> io::Result<()>;  // queued, never blocks; BrokenPipe after I/O failure
  pub fn flush(&self) -> io::Result<()>;
}
impl Write for PtyWriter

pub struct Pty  // Drop kills child
impl Pty {
  pub fn spawn(cmd: &str, args: &[String], cwd: Option<&Path>, env: &[(String,String)], cols: u16, rows: u16, tx: Sender<PtyEvent>) -> io::Result<Pty>;  // no on_wake; login_shell auto if args empty and cmd is ""/known shell
  pub fn spawn_with(cfg: SpawnConfig) -> io::Result<Pty>;
  pub fn writer(&self) -> PtyWriter;
  pub fn write_all(&self, buf: &[u8]) -> io::Result<()>;
  pub fn flush(&self) -> io::Result<()>;
  pub fn resize(&self, cols: u16, rows: u16, pixel_w: u16, pixel_h: u16) -> io::Result<()>;  // dims clamped to >=1
  pub fn kill(&mut self) -> io::Result<()>;   // SIGHUP, SIGKILL after 2s (Unix); TerminateProcess (Win); ok if already exited
  pub fn try_wait(&self) -> Option<Option<u32>>; // None=running; Some(code)
  pub fn is_alive(&self) -> bool;
  pub fn pid(&self) -> Option<u32>;
}

pub fn resolve_unix_shell(shell_env: Option<&OsStr>, usable: impl Fn(&Path)->bool) -> PathBuf; // $SHELL or /bin/sh
pub fn resolve_windows_shell(path_var: &OsStr, exists: impl Fn(&Path)->bool) -> PathBuf;      // pwsh > powershell > cmd
pub fn default_shell() -> PathBuf;
pub fn default_env() -> Vec<(OsString, OsString)>;  // COLORTERM=truecolor, TERM=xterm-kitty, TERM_PROGRAM=grimalkin, TERM_PROGRAM_VERSION, macOS LANG fallback
(also public-per-grep: resolve_in_path helper may exist; not confirmed as pub)
```

Notes:
- Threading: Pty spawns background reader, wait, and writer threads. Events arrive on the mpsc Receiver you supply; drain it with try_recv() on the GUI thread each frame/wake. Keep one Receiver per tab.
- Use Pty::spawn_with and set on_wake (e.g. winit EventLoopProxy::send_event) so the loop sleeps until output arrives. on_wake runs on a background thread after each event is sent, so it must be cheap and Send+Sync. Pty::spawn has no on_wake.
- Per frame: drain try_recv() until empty (cap bytes or time per frame to avoid stalls), feed Output bytes to the VT parser, handle Exited by closing or marking the tab. Output is never delivered after Exited.
- Input: clone PtyWriter per consumer and call write_all with key and paste bytes. It never blocks, but an error (BrokenPipe) means the child is gone. Do not rely on backpressure.
- Resize: call resize(cols, rows, pixel_w, pixel_h) when the grid changes. Pass real pixel sizes, since kitty graphics and ioctl TIOCGWINSZ users need cell pixel size. Initial pixel size via SpawnConfig.pixel_width/height.
- The output channel is unbounded; a flood like `yes` grows memory if the GUI lags. Cap per-frame processing but keep draining.
- TERM=xterm-kitty is set on all platforms, so the emulator must really implement kitty graphics and keyboard protocol or tools will misbehave. Override via env if needed.
- Dropping Pty kills the child. Keep Pty alive as long as the tab. Windows/ConPTY paths are unbuilt and untested; expect surprises there. Invalid cwd silently falls back to home. Spawning is serialized by a global lock on macOS (openpty is not thread-safe), so concurrent spawns are safe but brief.
