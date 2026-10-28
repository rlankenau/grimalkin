//! gcat: a tiny cross-platform "icat" for the kitty graphics protocol.
//! Run with `cargo run -p grimalkin-graphics --example gcat -- --help`.
//!
//! Support query: gcat sends an `a=q` query unless told otherwise. When the
//! environment has `TERM_PROGRAM=grimalkin` the terminal is assumed to support
//! the protocol and the query is skipped (`--query` forces it, `--no-query`
//! always skips it).
//!
//! ConPTY mode (`--conpty`, the default on Windows, `--no-conpty` to disable):
//! Windows ConPTY (OpenConsole) passes APC sequences through but does not learn
//! that an image placement moved the cursor, so output following an image is
//! drawn too high. In this mode every placement is sent with `C=1` (cursor does
//! not move) and gcat prints N newlines itself, where N is the number of
//! terminal rows the image covers: the given rows for `--place`, otherwise
//! ceil(image height / cell height), capped at terminal rows - 1. The cell
//! height comes from the window pixel size or a `CSI 16 t` query (reply
//! `CSI 6 ; height ; width t`), falling back to 20 px. `--demo` additionally
//! clears the screen and uses absolute `CSI H` positioning to put the cursor
//! below the first image after the placed copy.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal;
use flate2::{write::ZlibEncoder, Compression};

const HELP: &str = "\
gcat - display images with the kitty graphics protocol

USAGE:
  gcat [OPTIONS] IMAGE [IMAGE...]
  gcat --demo        terminal self-test (colours, text styles, images)
  gcat --delete      delete all images (a=d,d=A)

OPTIONS:
  --place COLSxROWS@XxY  scale to COLS x ROWS cells at column X, row Y (0-based)
  --zlib                 compress payload with zlib (o=z)
  --png                  send the file's PNG bytes as-is (f=100)
  --z N                  z-index
  --no-move              do not move the cursor after display (C=1)
  --id N                 base image id (default 100; image i gets N+i)
  --no-query             skip the support query
  --query                force the support query (it is skipped by default
                         when TERM_PROGRAM=grimalkin)
  --conpty               Windows ConPTY workaround (default on Windows): send
                         C=1 and print the newlines for the image's rows
                         ourselves; cell height is queried with CSI 16 t
                         (fallback 20 px). Demo restores the cursor with CSI H.
  --no-conpty            disable the ConPTY workaround
  -h, --help             show this help
";

#[derive(Clone, Default)]
struct Opts {
    place: Option<(u32, u32, u32, u32)>,
    zlib: bool,
    png: bool,
    z: Option<i32>,
    no_move: bool,
    id: u32,
    conpty: bool,
    cell_h: u32,
    term_rows: u32,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(data: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        o.push(B64[(n >> 18) as usize & 63]);
        o.push(B64[(n >> 12) as usize & 63]);
        o.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] } else { b'=' });
        o.push(if c.len() > 2 { B64[n as usize & 63] } else { b'=' });
    }
    o
}

fn query_support() -> bool {
    let mut out = std::io::stdout();
    if terminal::enable_raw_mode().is_err() {
        return false;
    }
    let _ = write!(out, "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    let _ = out.flush();
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut got = String::new();
    let mut ok = false;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match event::poll(left) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if let KeyCode::Char(c) = k.code {
                        got.push(c);
                        if got.contains("i=31;OK") {
                            ok = true;
                            break;
                        }
                        if got.contains("i=31;E") {
                            break;
                        }
                    }
                }
            }
            _ => break,
        }
    }
    let _ = terminal::disable_raw_mode();
    ok
}

/// Cell height in pixels: window pixel size if available, else `CSI 16 t`, else 20.
fn cell_height() -> u32 {
    if let Ok(ws) = terminal::window_size() {
        if ws.rows > 0 && ws.height > 0 {
            return (ws.height as u32 / ws.rows as u32).max(1);
        }
    }
    let mut out = std::io::stdout();
    if terminal::enable_raw_mode().is_err() {
        return 20;
    }
    let _ = write!(out, "\x1b[16t");
    let _ = out.flush();
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut got = String::new();
    let mut h = 20;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match event::poll(left) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    match k.code {
                        KeyCode::Char(c) => got.push(c),
                        KeyCode::Esc => got.push('\x1b'),
                        _ => {}
                    }
                    if got.ends_with('t') {
                        // ESC [ 6 ; height ; width t
                        let body = got.rsplit("[6;").next().unwrap_or("");
                        if let Some(v) = body.split(';').next().and_then(|v| v.parse::<u32>().ok()) {
                            if v > 0 {
                                h = v;
                            }
                        }
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    let _ = terminal::disable_raw_mode();
    h
}

/// Terminal rows covered by an image of `height_px` pixels.
fn image_rows(o: &Opts, height_px: u32) -> u32 {
    if let Some((_, r, _, _)) = o.place {
        return r;
    }
    height_px.div_ceil(o.cell_h.max(1)).min(o.term_rows.saturating_sub(1)).max(1)
}

/// Transmit and display one image. `payload` is raw pixels (f=32) or PNG bytes (f=100).
fn send(out: &mut impl Write, payload: &[u8], dims: Option<(u32, u32)>, o: &Opts, id: u32) {
    let mut data = payload.to_vec();
    if o.zlib {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(&data).unwrap();
        data = e.finish().unwrap();
    }
    let enc = b64(&data);
    let mut keys = format!("a=T,q=2,i={id}");
    match dims {
        Some((w, h)) => keys += &format!(",f=32,s={w},v={h}"),
        None => keys += ",f=100",
    }
    if o.zlib {
        keys += ",o=z";
    }
    if let Some((c, r, x, y)) = o.place {
        keys += &format!(",c={c},r={r}");
        let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
    }
    if let Some(z) = o.z {
        keys += &format!(",z={z}");
    }
    if o.no_move || o.conpty {
        keys += ",C=1";
    }
    let chunks: Vec<&[u8]> = enc.chunks(4096).collect();
    for (i, ch) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            let _ = write!(out, "\x1b_G{keys},m={more};");
        } else {
            let _ = write!(out, "\x1b_Gq=2,m={more};");
        }
        let _ = out.write_all(ch);
        let _ = out.write_all(b"\x1b\\");
    }
    if o.conpty {
        let height_px = match dims {
            Some((_, h)) => h,
            // PNG IHDR: signature(8) + len/type(8) + width(4) + height(4)
            None => payload.get(20..24).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        };
        let n = image_rows(o, height_px);
        let _ = out.write_all("\n".repeat(n as usize).as_bytes());
    } else {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

fn demo_image() -> (Vec<u8>, u32, u32) {
    let (w, h) = (256u32, 128u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let border = x < 2 || y < 2 || x >= w - 2 || y >= h - 2;
            if border {
                px.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                let a = if ((x / 16) + (y / 16)) % 2 == 0 { 255 } else { 90 };
                px.extend_from_slice(&[x as u8, (y * 2) as u8, 255 - x as u8, a]);
            }
        }
    }
    (px, w, h)
}

fn demo(out: &mut impl Write, o: &Opts) {
    if o.conpty {
        // Known starting point so the row math below is exact.
        let _ = write!(out, "\x1b[2J\x1b[H");
    }
    let _ = writeln!(out, "\x1b[1mgcat demo\x1b[0m\n16 colours:");
    for i in 0..16 {
        let _ = write!(out, "\x1b[48;5;{i}m  ");
    }
    let _ = writeln!(out, "\x1b[0m\n256 colours:");
    for i in 16..256 {
        let _ = write!(out, "\x1b[48;5;{i}m ");
        if (i - 15) % 72 == 0 {
            let _ = writeln!(out, "\x1b[0m");
        }
    }
    let _ = writeln!(out, "\x1b[0m\ntruecolor gradient:");
    for i in 0..64 {
        let r = (i * 4) as u8;
        let _ = write!(out, "\x1b[48;2;{r};{};{}m ", 255 - r, 128);
    }
    let _ = writeln!(out, "\x1b[0m");
    let _ = writeln!(
        out,
        "\x1b[1mbold\x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \
         \x1b[4:3mcurly\x1b[0m \x1b[9mstrike\x1b[0m \x1b[7mreverse\x1b[0m"
    );
    let _ = writeln!(out, "wide: 漢字かな  emoji: 🐱🚀");
    let _ = writeln!(out, "┌──────────┐\n│ box draw │\n└──────────┘");
    let (px, w, h) = demo_image();
    let _ = writeln!(out, "image {w}x{h}:");
    // Lines printed so far (assuming a terminal at least 72 columns wide): the
    // image starts on the next row.
    let image_row = 17;
    send(out, &px, Some((w, h)), &Opts { id: o.id, ..o.clone() }, o.id);
    let _ = writeln!(out, "scaled copy at 20x5@40x2:");
    let placed = Opts { place: Some((20, 5, 40, 2)), ..o.clone() };
    send(out, &px, Some((w, h)), &placed, o.id + 1);
    if o.conpty {
        let below = image_row + image_rows(o, h) + 1;
        let _ = write!(out, "\x1b[{below};1H");
    } else {
        let _ = writeln!(out, "\x1b[12;1H");
    }
}

fn die(msg: &str) -> ! {
    eprintln!("gcat: {msg}\n\n{HELP}");
    std::process::exit(2)
}

fn main() {
    let mut o = Opts { id: 100, conpty: cfg!(windows), ..Default::default() };
    let (mut files, mut do_demo, mut del, mut query) = (Vec::new(), false, false, None::<bool>);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().unwrap_or_else(|| die(&format!("{n} needs a value")));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            "--demo" => do_demo = true,
            "--delete" => del = true,
            "--zlib" => o.zlib = true,
            "--png" => o.png = true,
            "--no-move" => o.no_move = true,
            "--no-query" => query = Some(false),
            "--query" => query = Some(true),
            "--conpty" => o.conpty = true,
            "--no-conpty" => o.conpty = false,
            "--z" => o.z = Some(val("--z").parse().unwrap_or_else(|_| die("bad --z"))),
            "--id" => o.id = val("--id").parse().unwrap_or_else(|_| die("bad --id")),
            "--place" => {
                let v = val("--place");
                let p = (|| {
                    let (size, pos) = v.split_once('@')?;
                    let (c, r) = size.split_once('x')?;
                    let (x, y) = pos.split_once('x')?;
                    Some((c.parse().ok()?, r.parse().ok()?, x.parse().ok()?, y.parse().ok()?))
                })();
                o.place = Some(p.unwrap_or_else(|| die("--place expects COLSxROWS@XxY")));
            }
            s if s.starts_with("--") => die(&format!("unknown option {s}")),
            _ => files.push(a),
        }
    }
    let mut out = std::io::stdout();
    if del {
        let _ = write!(out, "\x1b_Ga=d,d=A\x1b\\");
        let _ = out.flush();
        return;
    }
    if !do_demo && files.is_empty() {
        die("no images given");
    }
    let trusted = std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "grimalkin");
    if query.unwrap_or(!trusted) && !query_support() {
        println!("no kitty graphics support");
        std::process::exit(1);
    }
    if o.conpty {
        o.term_rows = terminal::size().map_or(24, |(_, r)| r as u32);
        o.cell_h = cell_height();
    }
    if do_demo {
        demo(&mut out, &o);
        return;
    }
    for (i, f) in files.iter().enumerate() {
        let id = o.id + i as u32;
        let mut bytes = Vec::new();
        if let Err(e) = std::fs::File::open(f).and_then(|mut h| h.read_to_end(&mut bytes)) {
            eprintln!("gcat: {f}: {e}");
            std::process::exit(1);
        }
        if o.png {
            send(&mut out, &bytes, None, &o, id);
        } else {
            match image::load_from_memory(&bytes) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    send(&mut out, rgba.as_raw(), Some((w, h)), &o, id);
                }
                Err(e) => {
                    eprintln!("gcat: {f}: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}
