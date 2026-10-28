# Grimalkin

A GPU-accelerated terminal emulator written in Rust, aimed at Windows 10/11
(ConPTY) with the kitty graphics and keyboard protocols. It also builds and runs
on macOS and Linux, which is where it is mostly developed. Early software: the
core works, many conveniences are missing.

## What works today

- wgpu renderer with a glyph atlas, system font fallback (emoji, CJK, symbols),
  procedural box-drawing/block characters, wide chars and combining marks
- Terminal emulation: main/alt screen, scrollback (main screen), scroll regions,
  256/truecolor, underline styles and colors, mouse reporting (SGR), focus
  events, bracketed paste, synchronized output, OSC 0/2/4/7/8/10/11/12/52/133
- Kitty graphics protocol: direct/file/temp-file transmission, PNG and raw
  RGB(A), zlib, chunking, placement, source rects, z-index (above/below text),
  deletion, queries, images scrolling with text
- Kitty keyboard protocol (flag stack, query, key encoding)
- Selection, clipboard copy/paste, clickable OSC 8 hyperlinks, visual bell
  (optional audible), cursor shapes/blink, runtime zoom
- Themes (TOML or kitty `.conf`) with live config reload; TOML config: fonts, colors, window size/padding/opacity/title, cursor, bell, OSC 52
- Skins: undecorated, non-rectangular WinAMP-style windows (`--skin llama|grimalkin|mint`, or your own; see docs/SKINS.md)

Not done: no tabs or splits, no ligatures, no IME, no `U=1` unicode
placeholders, no animation frames (`a=f`).

## Requirements

- Rust stable 1.92+
- Windows: just `cargo`
- macOS: just `cargo`
- Linux: `sudo apt-get install libxkbcommon-dev libwayland-dev libfontconfig1-dev`

## Build and run

```
cargo run --release -p grimalkin
cargo run --release -p grimalkin -- [OPTIONS] [-e COMMAND [ARGS...]]
```

| Flag | Meaning |
| --- | --- |
| `-e`, `--command CMD [ARGS...]` | Run CMD instead of the shell (consumes the remaining arguments) |
| `--config PATH` | Use this config file |
| `--working-directory DIR` | Start in DIR |
| `--skin NAME` | Use a window skin (llama, grimalkin, mint, or a path; `none` = normal window) |
| `--theme NAME` | Use this color theme (overrides `theme` in the config) |
| `--list-themes` | List built-in and user themes, then exit |
| `--list-skins` | List built-in skins and installed WinAMP skins, then exit |
| `--install-skin PATH_OR_URL` | Install a WinAMP `.wsz` (local file or https URL) and exit; `--force` overwrites |
| `-h`, `--help` / `-V`, `--version` | Help / version |

The release binary is `target/release/grimalkin`.

## Configuration

Every key is optional. Default location:

- Windows: `%APPDATA%\grimalkin\grimalkin.toml`
- macOS/Linux: `$XDG_CONFIG_HOME/grimalkin/grimalkin.toml`, else
  `~/.config/grimalkin/grimalkin.toml`

A missing file is fine. The full commented list is in
[docs/grimalkin.example.toml](docs/grimalkin.example.toml).

```toml
scrollback_lines = 10000

[font]
family = ["JetBrains Mono", "Consolas"]
size = 13.0

[window]
cols = 120
rows = 35
opacity = 0.95
title = "grimalkin"      # shown until the shell sets one
dynamic_title = true     # false: ignore OSC 0/2 titles

theme = "catppuccin-mocha"

[colors]            # per-key overrides on top of the theme
background = "#101014"

[cursor]
shape = "bar"
blink = true

[input]
option_as_alt = true   # macOS
```

## Skins & themes

Two independent layers, like the media players of the late 90s: a **theme**
recolours the terminal, a **skin** replaces the window frame with bitmap
artwork, including non-rectangular shapes (WinAMP style).

| llama | grimalkin | mint |
|:-:|:-:|:-:|
| ![llama](crates/gui/assets/skins/llama/preview.png) | ![grimalkin](crates/gui/assets/skins/grimalkin/preview.png) | ![mint](crates/gui/assets/skins/mint/preview.png) |

```toml
skin  = "llama"        # llama | grimalkin | mint | a folder of your own | "none"
theme = "dracula"
```

`grimalkin --skin grimalkin --theme nord` does the same from the command line.
A skinned window has no OS title bar: drag the title bar or the frame to move,
the edges and the corner grip to resize, double-click the title bar for the
skin's action (usually *windowshade*: the window collapses to its title bar
and restores on the next double-click or shade button). Pixels outside the
artwork's shape are transparent and click through.

A skin is a folder with `skin.toml`, `frame.png` (9-slice), `buttons.png` and
optionally `shade.png` and `frame@2x.png`; put yours in
`<config dir>/grimalkin/skins/NAME/`. The format is documented in
[docs/SKINS.md](docs/SKINS.md), and `scripts/gen-skins.py` shows how the
built-in skins are drawn. On a skinned window the grid starts at the skin's
viewport inset plus `[window] padding`; the skin's `viewport_background` is the
terminal background unless you chose a theme or `[colors] background`.

### WinAMP skins

Classic WinAMP 2.x skins (`.wsz`) work as window frames. Grimalkin uses the
artwork of the **playlist editor** window (`pledit.bmp`), the one WinAMP window
that resizes, so the terminal sits inside a real WinAMP playlist frame (title
art, close and windowshade buttons, the bottom bar), and `pledit.txt` supplies
the colors. Browse thousands of skins at the
[Skin Museum](https://skins.webamp.org), then:

```sh
grimalkin --install-skin https://skins.webamp.org/skin/<md5>/     # a museum page, a direct .wsz URL or a local file
grimalkin --list-skins
```

and put the name it prints in `grimalkin.toml`:

```toml
skin = "winamp:base-2.91"          # <config dir>/grimalkin/skins/base-2.91.wsz
# skin = "C:/skins/Cool.wsz"       # a path to a .wsz, or to an unpacked folder with pledit.bmp, also works
# winamp_viscolor_palette = true   # optionally take the 16 ANSI colors from the skin's viscolor.txt
```

While a WinAMP skin is active its `pledit.txt` colors (text, list background,
selection, current-track color) become the theme, unless you set `theme =` (or
`--theme`) yourself; `[colors]` overrides still apply on top. A `Font=` hint is
used only if that font is installed *and* monospaced. Skins are read from the
archive on every launch and never extracted to disk. Details and deviations
from the original spec: [docs/WINAMP_SKINS.md](docs/WINAMP_SKINS.md).

### Themes and live reload

`theme = "NAME"` (or `--theme NAME`) selects a color theme: colors, optional
cursor and font settings. Built in: `default`, `catppuccin-mocha`, `dracula`,
`gruvbox-dark`, `solarized-light`, `nord`, `tokyo-night`. `grimalkin --list-themes`
lists them along with your own.

Your own themes go in `<config dir>/grimalkin/themes/` (the directory that holds
`grimalkin.toml`):

- `NAME.toml`: grimalkin format, a `name`, a `[colors]` table (`foreground`,
  `background`, `cursor`, `cursor_text`, `selection_background`,
  `selection_foreground`, `url`, `black` .. `bright_white` or `color0` ..
  `color15`) and optional `[font]` / `[cursor]` tables.
- `NAME.conf`: a kitty theme file. Drop in anything from
  [kitty-themes](https://github.com/kovidgoyal/kitty-themes).

The `[colors]` table of `grimalkin.toml` overrides the theme key by key.

The config directory (config file, `themes/`, `skins/`) is watched. Saving a
file re-applies the theme, colors, cursor, font and, between two skins, the
window skin in the running window, with no restart (switching between a skin
and a normal window needs a restart). A file with a syntax error is reported
in the log and the running settings are kept. (`--theme` and `--skin` keep
winning over the file.)

## Key bindings

"Mod" is Ctrl+Shift on Windows/Linux, and Cmd (or Ctrl+Shift) on macOS.

| Keys | Action |
| --- | --- |
| Mod+C | Copy selection |
| Mod+V, Shift+Insert | Paste |
| Mod++ / Mod+- | Zoom in / out |
| Mod+0 | Reset zoom |
| Shift+PageUp / Shift+PageDown | Scroll a page (main screen) |
| Mouse wheel | Scroll scrollback (or sent to the app in mouse mode) |

## Testing kitty graphics

The `gcat` example is a small cross-platform `icat` (works on Windows):

```
cargo run -p grimalkin-graphics --example gcat -- --demo
cargo run -p grimalkin-graphics --example gcat -- picture.png
cargo run -p grimalkin-graphics --example gcat -- --place 40x12@5x3 --z -1 picture.png
cargo run -p grimalkin-graphics --example gcat -- --delete
```

Other options: `--zlib`, `--png`, `--no-move`, `--id N`, `--no-query`.

On macOS, with kitty's `kitten` installed, `scripts/icat-test.sh [case...]`
drives `kitten icat` through several scenarios (basic, place, alpha, big,
scroll, clear, zindex, altscreen) and captures frames. `MODE=dump` (default) is
offscreen; `MODE=screen` captures the real window. `scripts/kitty-send.py` is a
raw protocol sender.

Startup font loading: the first run scans all system fonts and writes an index
(`fonts.cache` under the user cache dir, e.g. `%LOCALAPPDATA%\\grimalkin` on Windows). Later runs
load only the configured families from the index and build the full set (emoji/CJK
fallback) on a background thread. Set `GRIMALKIN_EXIT_AFTER_FRAME=1` to quit after the
first frame (for timing).

Set `GRIMALKIN_DUMP_FRAME=/path/frame.png` to render offscreen and write each
frame to that PNG (useful over ssh or with a locked screen).

## TERM

Children get `TERM=xterm-kitty` so kitty-aware tools enable graphics and
keyboard protocols. On Unix, if no kitty terminfo database can be found
(`TERMINFO` is pointed at kitty's bundled one when present), Grimalkin falls
back to `TERM=xterm-256color` so vim, tmux, less and friends keep working. See
`crates/gui/src/terminfo.rs`.

## Windows: kitty graphics need a newer ConPTY

Stock Windows 10 ConPTY (conhost 10.0.19041) strips APC sequences
(`ESC _ ... ESC \`), so the kitty graphics protocol never reaches Grimalkin, and
it swallows the kitty keyboard query (`CSI ? u`). Grimalkin talks to ConPTY
directly (`crates/pty/src/win/`) and can use Microsoft's current one:

1. Download the `Microsoft.Windows.Console.ConPTY` NuGet package (a `.nupkg` is
   a zip file) and take `runtimes/win-x64/native/conpty.dll` and
   `build/native/runtimes/x64/OpenConsole.exe` (paths vary a little by
   package version; use the x64 pair).
2. Put both files in the same directory as `grimalkin.exe`.

At startup Grimalkin loads `conpty.dll` from the executable's directory if it
exists and falls back to the in-box `kernel32.dll` ConPTY otherwise. Run with
`RUST_LOG=info` to see which one was used.

For the best chance of APC and other sequences arriving untouched, also turn on
passthrough mode (`PSEUDOCONSOLE_PASSTHROUGH_MODE`), which stops ConPTY from
re-rendering the child's output:

```toml
[pty]
passthrough = true
```

or set `GRIMALKIN_CONPTY_PASSTHROUGH=1` (`0` forces it off), which overrides
the file. Passthrough needs the sideloaded ConPTY or Windows 11; if the ConPTY
in use rejects the flag, Grimalkin logs a warning and starts without it.
Passthrough is off by default.

## Crate layout

- `crates/vt` (`grimalkin-vt`): terminal state machine, no I/O or GUI
- `crates/graphics` (`grimalkin-graphics`): kitty graphics protocol, image
  store, `gcat` example
- `crates/pty` (`grimalkin-pty`): PTY layer: native ConPTY on Windows, portable-pty (forkpty) on Unix
- `crates/gui` (`grimalkin`): the binary: winit + wgpu renderer, input, config

See [ARCHITECTURE.md](ARCHITECTURE.md) for details. Tests: `cargo test --workspace`.

## License

MIT OR Apache-2.0
