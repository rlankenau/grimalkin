# Themes and skins

Two independent layers, both selectable in `grimalkin.toml` and switchable at
runtime via config reload:

* **Theme** = colors + cursor + font settings. `theme = "name"`.
* **Skin** = window chrome: a non-rectangular, undecorated window drawn from
  bitmaps, 1990s WinAMP style. `skin = "name"` (or `"none"` for the normal OS
  decorated window, the default).

Lookup order for both: `<config_dir>/grimalkin/themes/<name>.toml` and
`<config_dir>/grimalkin/skins/<name>/`, then the built-in set embedded in the
binary with `include_bytes!`/`include_str!` from `crates/gui/assets/`.

## Themes (`crates/gui/src/theme.rs`)

File `themes/<name>.toml`:

```toml
name = "Catppuccin Mocha"
[colors]
foreground = "#cdd6f4"
background = "#1e1e2e"
cursor = "#f5e0dc"
cursor_text = "#1e1e2e"
selection_background = "#585b70"
selection_foreground = "#cdd6f4"
url = "#89b4fa"
# ansi 0-15; also accepted as color0..color15 keys
black = "#45475a"
red = "#f38ba8"
# ...
bright_black = "#585b70"
# ...
[font]            # optional overrides of the [font] section of grimalkin.toml
family = ["JetBrains Mono"]
[cursor]          # optional
shape = "block"   # block | underline | bar
blink = false
```

Also accept kitty theme files (`themes/<name>.conf`, lines `color0 #rrggbb`,
`foreground`, `background`, `cursor`, `selection_background`, ...) so users
can drop in any theme from kitty-themes. Built-ins: `default` (current
defaults), `catppuccin-mocha`, `dracula`, `gruvbox-dark`, `solarized-light`,
`nord`, `tokyo-night`. The config values in `[colors]` of grimalkin.toml
override the theme (theme first, then per-key overrides). `set_default_colors`
/ `set_palette_color` on the vt Terminal apply the theme; changing theme at
runtime must repaint everything.

## Skins (`crates/gui/src/skin.rs`)

A skin is a directory:

```
skins/<name>/
  skin.toml
  frame.png       # RGBA, the chrome at 1x pixel scale
  frame@2x.png    # optional HiDPI version; otherwise frame.png is scaled with
                  # nearest-neighbour by the integer DPI factor (pixel-art look)
  buttons.png     # optional sprite sheet
```

`skin.toml`:

```toml
name = "Llama"
author = "..."
# The window is sized so that `viewport` fits the terminal grid; the frame is
# stretched with 9-slice scaling: the four corner regions are fixed, the edges
# tile or stretch (per `edge_mode`), the center is the terminal viewport.
[frame]
image = "frame.png"
slices = { left = 24, right = 24, top = 36, bottom = 28 }   # 9-slice insets (px)
edge_mode = "tile"      # tile | stretch
viewport_inset = { left = 24, right = 24, top = 36, bottom = 28 }  # where the grid draws
viewport_background = "#000000"   # drawn under the grid (theme background wins if set)
viewport_opacity = 1.0
# Non-rectangular shape: pixels with alpha < 128 in the (scaled) frame are
# outside the window: not drawn, not clickable. Corners stay as drawn; edge
# tiles repeat the mask too.

[titlebar]
rect = { x = 0, y = 0, w = -1, h = 36 }    # w=-1 means "full width"; drag to move
double_click = "shade"   # shade | maximize | none
text = { x = 40, y = 10, w = -120, font_px = 11, color = "#9bd6a1", align = "center" }   # window title in an LCD-ish inset

[buttons]                # each: rect in frame space (negative x = from right edge),
                         # sprite = [x, y] in buttons.png for normal/hover/pressed
                         # states stacked vertically, size = rect size
sheet = "buttons.png"
close    = { rect = { x = -22, y = 8, w = 14, h = 14 }, sprite = [0, 0] }
minimize = { rect = { x = -40, y = 8, w = 14, h = 14 }, sprite = [14, 0] }
shade    = { rect = { x = -58, y = 8, w = 14, h = 14 }, sprite = [28, 0] }   # WinAMP windowshade: collapse to the title bar
maximize = { rect = { x = -76, y = 8, w = 14, h = 14 }, sprite = [42, 0] }

[resize]
grip = { x = -20, y = -20, w = 20, h = 20 }   # drag to resize; plus edges of
border = 6                                    # `border` px around the frame

[shade]                                       # optional windowshade layout
image = "shade.png"                           # a thin bar; same titlebar/buttons rules
height = 14
```

### Runtime behaviour

* With a skin active the winit window is created `with_decorations(false)`,
  `with_transparent(true)` (macOS/Linux), and sized to grid + insets. Window
  min size respects the fixed corners.
* Rendering: a new "chrome" pass draws the 9-sliced frame (nearest-neighbour
  sampling, integer scale) around the viewport, and the shader multiplies the
  final alpha by the frame mask so pixels outside the shape are transparent.
  Terminal content is clipped to the viewport rect (scissor). Images and
  selection already use the cell grid, so they only need the viewport offset.
* **Windows**: per-pixel alpha swapchains are unreliable across wgpu backends,
  so instead compute an `HRGN` from the scaled mask (run-length rows ->
  `CreateRectRgn` + `CombineRgn`, or `ExtCreateRegion` with RGNDATA) and call
  `SetWindowRgn` whenever the window size or DPI changes. The swapchain stays
  opaque; the OS clips drawing and hit-testing to the region. Use
  `raw-window-handle` from winit to get the HWND; `windows-sys` features
  Win32_Graphics_Gdi + Win32_UI_WindowsAndMessaging. Keep this in
  `skin/win.rs` behind `cfg(windows)`.
* **macOS**: the transparent window already passes clicks through alpha-0
  pixels; set the surface alpha mode to the one wgpu offers (PostMultiplied
  on Metal) and output straight (non-premultiplied) alpha accordingly.
* **Linux**: alpha via the compositor; click-through outside the shape is best
  effort (ignore).
* Hit testing (in `skin.rs`, pure logic, unit-tested): mouse position ->
  `Hit::{Viewport, Titlebar, Button(kind), ResizeEdge(dir), Outside, Chrome}`.
  Titlebar press -> `window.drag_window()`; edge/grip press ->
  `window.drag_resize_window(dir)`; buttons: hover/pressed sprite states,
  click actions (close, minimize, maximize/restore, shade toggle).
  Double-click on the title bar does the configured action.
* Windowshade: the window shrinks to the shade bar height and shows only the
  title; terminal keeps running; clicking shade again restores the previous
  size.
* Pixel scale: `scale = max(1, round(dpi_scale_factor))`; the frame is drawn
  at that integer scale; `frame@2x.png` is used when scale == 2 if present.
* Config: `skin = "llama"`, `theme = "dracula"`, both hot-reloadable; also
  `--skin`/`--theme` CLI flags. `skin = "none"` restores normal decorations
  (requires window recreation; acceptable to apply on next launch, but try
  `set_decorations`).

### Built-in skins (generated by a script in `scripts/gen-skins.py`, PNGs
committed under `crates/gui/assets/skins/`)

1. `llama`: classic late-90s media-player look: dark brushed steel gradient
   frame, 2px bevelled edges (light top-left, dark bottom-right), a green
   LCD-style title inset with a scanline texture, chunky bevelled buttons,
   notched (non-rectangular!) corners: a 45-degree cut on the top corners and a
   stepped "wing" on the bottom-right with the resize grip. Window shade bar
   included.
2. `grimalkin`: a cat-shaped silhouette: two triangular ears rising from the
   title bar (non-rectangular top edge), rounded body corners, dark purple/
   charcoal palette, amber eyes as the minimize/close buttons (they blink on
   hover), a tail curling at the bottom-right as the resize grip.
3. `mint`: a light pastel 1998 "skin" with rounded 12px corners, drop-shadow
   baked into the alpha, and a thin chrome title bar; the conservative option.

The generator must produce pixel-art-quality images (no blur, crisp 1px
lines, limited palette), at 1x; the engine scales. Each skin ships a 160x60
`preview.png` for docs.

## Engine notes (as implemented in `crates/gui/src/skin/`)

Details the format above leaves open, for skin authors:

* **Rect syntax** (frame space, 1x pixels): `x < 0` / `y < 0` count from the
  right / bottom edge; `w = -1` / `h = -1` extend to the right / bottom edge;
  `w < -1` / `h < -1` leave a margin of `-w` / `-h` pixels at that edge.
* **Lookup**: `skin = "name"` checks `<config_dir>/grimalkin/skins/<name>/`,
  then the built-ins (every directory under `crates/gui/assets/skins/` with a
  `skin.toml` is embedded by `build.rs`; no Rust edit needed). A value with a
  path separator is read as a skin directory. `--skin NAME` overrides the
  config; an unknown skin logs an error and falls back to a normal window.
* **Scale**: `scale = max(1, round(dpi))`. `frame@2x.png` (exactly twice the
  size of `frame.png`) is used only when `scale == 2`; otherwise `frame.png`
  is replicated by the integer scale. `buttons.png`, `shade.png` are 1x only.
* **Hit testing priority**: buttons, viewport, outside-the-mask (alpha < 128),
  grip, window-edge resize zone (`border` px; corner zones reach `2*border`),
  titlebar, other chrome (dragging the plain frame moves the window). The
  viewport and buttons always count as inside the shape.
* **Buttons**: normal / hover / pressed sprites are stacked vertically in
  `buttons.png`, `rect.h` apart, starting at `sprite`. A press only fires on
  release over the same button. A sheet that is too short for the hover and
  pressed rows falls back to the normal sprite.
* **Windowshade**: the shaded window is the full width and `[shade] height`
  pixels tall. `shade.png` is 3-sliced horizontally using the frame's left
  and right `slices`; without it the top `height` rows of `frame.png` are
  used. Without a `[shade]` section the bar is as tall as the titlebar rect.
  Titlebar, title text and button rects that do not fit the thin bar (taller
  than it) are shrunk and vertically centred on it. The grid, and the size the
  child sees, are unchanged while shaded.
* **Viewport background**: `viewport_background` applies when the user has not
  set `[colors] background`; otherwise the configured background is used.
  `viewport_opacity` (times `[window] opacity`) is the alpha of that fill.
* **Surface alpha**: the chrome pass renders premultiplied alpha (what
  CAMetalLayer and compositors expect, including for wgpu's `PostMultiplied`
  mode on Metal). Pixels outside the shape are alpha 0 because the frame is
  the only thing drawn there; terminal content is confined to the viewport.
* **macOS**: winit has no `drag_resize_window`, so edge/grip resizing is done
  by hand from pointer motion there (`skin/drag.rs`). The direction, pointer,
  window origin, size and minimum size are latched on press and kept until
  release; each step is computed from the system's global pointer position
  (window-local coordinates are unreliable while the window resizes), so the
  opposite edge stays put and all eight directions work, including left/top
  (which also move the window origin). A button state that reads "up" or a
  focus loss ends the drag, and the pointer icon is re-derived from the hit
  test afterwards. Title-bar moves use winit's `drag_window`, which is native.

## Testing hooks

Environment variables for frame dumps and automation (`crates/gui/src/debug_script.rs`):

* `GRIMALKIN_SCALE_FACTOR=2` pretends the display scale is 2 (HiDPI chrome checks on a 1x screen).
* `GRIMALKIN_DEBUG_SCRIPT="1500:hover:close;800:shade;1000:size:900x500"`
  runs `delay-ms:command` steps through the real input handlers: `move:X,Y`,
  `hover:BUTTON`, `press:BUTTON`, `down`, `up`, `click`, `dblclick`, `shade`,
  `size:WxH`, `exit`. After each step the log (`RUST_LOG=grimalkin=info`) has a
  `debug script: inner size WxH` line, e.g. a grip drag is
  `move:1006,587;down;move:1046,617;move:1086,647;up`. Combine with `GRIMALKIN_DUMP_FRAME` to inspect each state.
* Hot reload: between two skins the window is re-skinned live (a skin read from
  a user directory is re-read on every change under `skins/`); going between
  "no skin" and a skin applies on the next launch.
