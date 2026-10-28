# WinAMP 2.x skin (.wsz) importer: spec

Contract for the importer that turns a classic WinAMP `.wsz` into a grimalkin
skin (see `SKINS.md`) plus a theme. Only the **playlist editor** artwork
(`pledit.bmp` + `pledit.txt`) is used for window chrome, because it is the one
resizable WinAMP window. Sprite coordinates come from Webamp's
`packages/webamp/js/skinSprites.ts` (`PLEDIT`, `TITLEBAR`) and its
`css/playlist-window.css`; both were read on 2026-10-28.

## 1. Format facts

* A `.wsz` is a plain ZIP. Entry names are **case-insensitive**
  (`PLEDIT.BMP`, `pledit.bmp`, `Pledit.bmp`), and may sit in a subfolder
  (`xmms-turquoise/Pledit.bmp` exists). Match `^(.*[/\\])?name\.(ext)$`
  case-insensitively on the full path. If several entries match, **use the
  last one in archive order** (Winamp extracted to a case-insensitive disk, so
  later entries overwrote earlier ones). Skip `__MACOSX/` entries.
* Images are Windows BMP: usually 8-bit paletted (base-2.91) or 24-bit; 32-bit
  also seen (no real alpha: treat A as 255). Some modern skins use PNG, and
  some BMP-named files are really PNG: **sniff magic bytes** (`BM` vs
  `\x89PNG`), never the extension. Webamp also accepts `.png` for any bitmap
  name. Bottom-up row order (positive height) is the norm. `pledit.bmp` is
  280 x 186 in the standard layout; one sample is 280 x 190 (extra rows
  ignored). Row stride is padded to 4 bytes.
* **No alpha anywhere.** Checked: none of 7 sample `pledit.bmp` files contains
  `#FF00FF`. Magenta is NOT a convention for pledit; classic Winamp shapes
  windows with `region.txt` (main/eq only), and Winamp ignores it for the
  playlist. So the playlist frame is rectangular; do not derive a mask.
* `main.bmp` (275x116 main window background) and `titlebar.bmp` (main title
  bar 275x14 at 27,0 focused / 27,15 unfocused; 9x9 buttons: close 18,0,
  shade 0,18, minimize 9,0, pressed one row lower; see `TITLEBAR` in the
  sprite table) are for a future main-window mode, unused by v1. The
  playlist's own close/shade sprites live in `pledit.bmp` (below).
* `region.txt` (INI, for the record, not used for playlist): sections
  `[Normal]`, `[WindowShade]`, `[Equalizer]`, `[EqualizerWS]`; keys
  `NumPoints=n1,n2,...` (vertex count per polygon) and `PointList=x,y x,y ...`
  (pairs separated by comma or space, polygons concatenated). Polygons with
  fewer than 3 points are dropped; extra polygons without points are ignored.
  Keys case-insensitive. The base skin ships one.
* `viscolor.txt`: 24 lines, each `R,G,B` (decimal, optional trailing
  `// comment`; tolerate spaces, missing commas). Regex
  `^\s*(\d+)\s*,?\s*(\d+)\s*,?\s*(\d+)`. Non-matching lines are skipped. Index
  0 black bg; 1 dots grey; 2..17 spectrum analyser top to bottom; 18..22
  oscilloscope; 23 peak dots. Missing entries fall back to the default skin.
* `pledit.txt` is INI, section `[Text]` (section and key names
  case-insensitive; `;` starts a comment line; ignore anything after a second
  `=`). Keys: `Normal` (track text), `Current` (now-playing track text),
  `NormalBG` (list background), `SelectedBG` (selected row background), `Font`
  (font family name, e.g. Arial, Lucida Sans; default Arial), and Winamp 2.9x
  additions `mbFG` / `mbBG` (mini-browser fg/bg; ignore). Colours are
  `#RRGGBB`, but skins have a missing `#` or extra digits: prepend `#` if
  absent and truncate to 7 chars. Missing file or section: use the base-2.91
  defaults `Normal=#00FF00 Current=#FFFFFF NormalBG=#000000
  SelectedBG=#0000C6 Font=Arial`.

## 2. pledit.bmp sprite table

All rects `x,y,w,h` in pixel coordinates of the **top-left-origin** image
(after un-flipping the BMP). "Sel" = focused window (top half of the sheet);
unselected = inactive window (same art shifted down by 21 px on the title row).
The standard playlist window is 275 x 116 minimum and grows in steps of
25 px wide, 29 px tall (`W = 275 + 25*n`, `H = 116 + 29*m`).

| Webamp name | x,y,w,h | Role |
|---|---|---|
| PLAYLIST_TOP_LEFT_SELECTED | 0,0,25,20 | corner, top-left, focused |
| PLAYLIST_TITLE_BAR_SELECTED | 26,0,100,20 | title art "WINAMP PLAYLIST EDITOR", centred, focused |
| PLAYLIST_TOP_TILE_SELECTED | 127,0,25,20 | top edge, tile period 25 px horizontally, focused |
| PLAYLIST_TOP_RIGHT_CORNER_SELECTED | 153,0,25,20 | corner, top-right (contains the close and shade button art), focused |
| PLAYLIST_TOP_LEFT_CORNER | 0,21,25,20 | unfocused variants of the four above, same x/w, y=21 |
| PLAYLIST_TITLE_BAR | 26,21,100,20 | unfocused title art |
| PLAYLIST_TOP_TILE | 127,21,25,20 | unfocused top tile |
| PLAYLIST_TOP_RIGHT_CORNER | 153,21,25,20 | unfocused top-right |
| PLAYLIST_LEFT_TILE | 0,42,12,29 | left edge, tile period 29 px vertically |
| PLAYLIST_RIGHT_TILE | 31,42,20,29 | right edge (includes scroll track), tile period 29 px |
| PLAYLIST_BOTTOM_LEFT_CORNER | 0,72,125,38 | corner, bottom-left, with menu button art (ADD REM SEL MISC) |
| PLAYLIST_BOTTOM_TILE | 179,0,25,38 | bottom edge filler, tile period 25 px |
| PLAYLIST_VISUALIZER_BACKGROUND | 205,0,75,38 | shown only when W >= 350, between left (125) and right (150) bottom pieces |
| PLAYLIST_BOTTOM_RIGHT_CORNER | 126,72,150,38 | corner, bottom-right (time display, mini transport, LIST button, resize grip) |
| PLAYLIST_CLOSE_SELECTED | 52,42,9,9 | close button, **pressed** state (normal state is baked into top-right corner) |
| PLAYLIST_COLLAPSE_SELECTED | 62,42,9,9 | shade (collapse) button, pressed state |
| PLAYLIST_EXPAND_SELECTED | 150,42,9,9 | un-shade button, pressed (shade mode) |
| PLAYLIST_SCROLL_HANDLE | 52,53,8,18 | scroll thumb, normal |
| PLAYLIST_SCROLL_HANDLE_SELECTED | 61,53,8,18 | scroll thumb, pressed |
| PLAYLIST_SHADE_BACKGROUND_LEFT | 72,42,25,14 | windowshade (14 px tall bar) left cap |
| PLAYLIST_SHADE_BACKGROUND | 72,57,25,14 | windowshade middle, tile period 25 px |
| PLAYLIST_SHADE_BACKGROUND_RIGHT | 99,57,50,14 | shade right cap, unfocused |
| PLAYLIST_SHADE_BACKGROUND_RIGHT_SELECTED | 99,42,50,14 | shade right cap, focused |
| PLAYLIST_{ADD_URL,ADD_DIR,ADD_FILE} (+_SELECTED) | 0 / 23, 111 / 130 / 149, 22,18 | ADD menu items; normal at x=0, pressed at x=23 |
| PLAYLIST_{REMOVE_ALL,CROP,REMOVE_SELECTED,REMOVE_MISC} | 54 / 77, 111 / 130 / 149 / 168, 22,18 | REM menu |
| PLAYLIST_{INVERT_SELECTION,SELECT_ZERO,SELECT_ALL} | 104 / 127, 111 / 130 / 149, 22,18 | SEL menu |
| PLAYLIST_{SORT_LIST,FILE_INFO,MISC_OPTIONS} | 154 / 177, 111 / 130 / 149, 22,18 | MISC menu |
| PLAYLIST_{NEW_LIST,SAVE_LIST,LOAD_LIST} | 204 / 227, 111 / 130 / 149, 22,18 | LIST menu |
| PLAYLIST_{ADD,REMOVE,SELECT,MISC,LIST}_MENU_BAR | 48,111,3,54 / 100,111,3,72 / 150,111,3,54 / 200,111,3,54 / 250,111,3,54 | menu popup side bars |

Layout facts from Webamp CSS (offsets in window pixels):

* Top row (20 px): TL corner (25) | fill from TOP_TILE | TITLE (100, centred)
  | fill | TR corner (25). Webamp inserts 12/13 px spacers around the title
  at odd widths; for a generic width simply tile TOP_TILE across the whole
  row and overlay the title art centred.
* Close button: 9x9 at `x = W-11, y = 3` (CSS `right:2px; top:3px`). Shade
  button: 9x9 at `x = W-21, y = 3` (`right:12px`). Both are click targets over
  art already drawn in the TR corner; on press, blit the pressed sprite.
* Middle rows: left tile at x 0..12, right tile at x W-20..W; track text area
  is x 12..W-20 with 3 px padding top and bottom (tracks start at y = 23).
  Rows are 13 px high, 9 px font.
* Scroll thumb (8x18) travels in the right tile track at x = W-15 (`right:7px`).
* Resize grip 20x20 at bottom-right. Shade mode: 14 px tall, middle tiled.

## 3. Mapping to grimalkin skin format

Frame (`[frame]`), compose in memory at 300 x 87 (top 20 + middle 29 + bottom 38;
middle width = 25 beyond the fixed 125 + 150 columns):

* `slices = { left = 125, right = 150, top = 20, bottom = 38 }`, `edge_mode = "tile"`.
  Min window width is therefore 275 (as in Winamp). Top row: tile TOP_TILE
  every 25 px, TL corner at x=0, TR corner at the last 25 px. Middle row: left
  tile at x 0..12, right tile at x 280..300, any pixels between are covered by
  the viewport. Bottom row: BOTTOM_LEFT_CORNER at 0..125, BOTTOM_TILE at
  125..150, BOTTOM_RIGHT_CORNER at 150..300. Use the focused (y=0) variants;
  optionally emit a second, unfocused frame if the engine gains a focus state.
* Engine requirement: `viewport_inset` may be smaller than the slice (the
  viewport overlaps the left/right 9-slice columns and is drawn on top).
  `viewport_inset = { left = 12, right = 20, top = 20, bottom = 38 }` (+3 top/bottom matches Winamp).
  If the engine cannot, fallback: slices `{25, 25, 20, 38}` and compose the
  bottom row from the first 25 px of BOTTOM_LEFT and last 25 px of
  BOTTOM_RIGHT, dropping the menu button art.
* The frame is **opaque**: alpha 255 everywhere, rectangular window, no mask.
  `viewport_background = NormalBG`.
* Titlebar: `rect = { x = 0, y = 0, w = -1, h = 20 }`, `double_click = "shade"`.
  The title art (TITLE_BAR, 100x20) is a picture, not text; it is not tileable
  and must be centred. Preferred: add an optional `[titlebar] image = {
  sprite = [26, 0], size = [100, 20], align = "center" }` extension to the
  skin struct (importer holds the 100x20 RGBA in memory). If not available,
  omit the art and leave the 100 px of tile visible; do not draw the window
  title as text on top of baked art (set `text` to none).
* Buttons (sheet built in memory, three states stacked vertically, 9x9 each):
  `close = rect {x=-11,y=3,w=9,h=9}`: normal/hover = crop of the TR corner at
  corner-local (14,3) (corner is 25 wide, W-11 = corner x 14), pressed =
  CLOSE_SELECTED (52,42). `shade = rect {x=-21,y=3,w=9,h=9}`: normal/hover =
  corner-local (4,3), pressed = COLLAPSE_SELECTED (62,42). Omit minimize and
  maximize. Verify the crop offsets visually against base-2.91.
* Resize: `grip = { x=-20, y=-20, w=20, h=20 }`, `border = 4`.
* `[shade]`: height 14. Compose shade.png (275 x 14): SHADE_BACKGROUND_LEFT
  (72,42,25x14) at 0, SHADE_BACKGROUND (72,57) tiled, SHADE_BACKGROUND_RIGHT
  _SELECTED (99,42,50x14) at the end. The existing engine 3-slices it using
  frame left/right slices, so emit slices of 25/50 for shade only if
  possible, else accept the stretch. Un-shade button: EXPAND_SELECTED pressed.
* Scroll bar and bottom-bar menu buttons: not mapped (decorative only,
  already baked into the corners).

Theme from `pledit.txt` (the importer returns a theme alongside the skin):

| pledit.txt | grimalkin theme |
|---|---|
| Normal | `foreground` |
| NormalBG | `background` (and `viewport_background`) |
| SelectedBG | `selection_background` |
| Normal (again) | `selection_foreground` (contrast-check vs SelectedBG; if ratio < 3 use Current) |
| Current | `cursor`; `cursor_text` = NormalBG |
| Font | `[font] family = [Font]` as a hint only; use only if installed, else keep user font |
| (derived) | `url` = Current |

Optional ANSI derivation from `viscolor.txt` (all 24 lines available): use
the analyser gradient `c[2..17]` (red at top to green at bottom). Suggested:
`red=c[2]`, `yellow=c[8]`, `green=c[12]`, `cyan=c[10]` shifted to teal via
lerp(green,c[19],0.5), `blue=` lerp(c[19],SelectedBG,0.5), `magenta=`
lerp(red,blue,0.5), `black=NormalBG`, `white=c[20]`; bright_* = lerp(x,
white, 0.25); `bright_black=c[1]`. Opt-in only (`winamp_ansi = true`), since
the result is a mood, not a calibrated palette. Default: leave ANSI colours to
the active theme.

## 4. Importer design (`crates/gui/src/skin/winamp.rs`)

Deps: `zip` (default-features off, `deflate`), `image` with only the `bmp`
and `png` features (use `image::guess_format`, not file extensions).

1. **Open**: read the file (or the cached copy); `zip::ZipArchive::new`.
   Build `HashMap<String, usize>` from lowercased path (backslashes to `/`)
   to the *last* index for that path. Helper `find(base, exts)` matches
   `^(.*/)?base\.(ext)$` on the lowercase map, preferring the shallowest path
   only when several folders exist. Limit entry reads to 8 MiB each and total
   archive to 64 MiB (zip-bomb guard); never write entries to disk by name.
2. **pledit.bmp** (required; error "not a WinAMP skin" if absent; also accept
   `pledit.png`). Decode with `image::load_from_memory`. 8-bit paletted BMP:
   the `image` BMP decoder expands the palette to RGB; palettes may be
   shorter than 256 entries or contain out-of-range indices: treat a decode
   error as fatal for that skin, with a clear message. Convert to RGBA8 with
   A=255 (ignore any 32-bit alpha, often garbage 0). If width < 280 or height
   < 110 -> error. Cropping beyond 186 rows is fine.
3. **Compose frame** (`RgbaImage` 300 x 87) by `blit(src_rect, dst_xy)` per
   the section 3 recipe; a `tile(src_rect, dst_rect)` helper repeats with
   period = src size. Also build the title image (26,0,100,20), the button
   sheet (9 px wide, 3 rows x 2 buttons), and shade.png (275 x 14).
4. **Parse text**: `pledit.txt` per section 1 (latin-1 decode, strip BOM,
   `\r\n`), `viscolor.txt` optional.
5. **Build structs** (in memory, same types the TOML loader fills, no
   temp files): `Skin { name, author: None, frame, titlebar, buttons, resize,
   shade }` and `Theme { colors, font }`. `name` = file stem. A
   `Skin::from_wsz(bytes) -> Result<(Skin, Theme)>` entry point; unit tests
   use a synthesized 280x186 BMP, plus a test over a downloaded sample if
   the env var `GRIMALKIN_WSZ_SAMPLE_DIR` is set.
6. Scale: nearest-neighbour as for other skins, no `@2x`.
7. Parsed skin is rebuilt each launch (cheap); never extract to disk.

## 5. CLI and config

* `grimalkin --install-skin <path-or-url>`:
  * URL (`http(s)://`): download with a 20 s timeout, 8 MiB cap, to a temp file
    in the skins dir; reject HTML (the first bytes must be `PK\x03\x04`);
    Webamp museum links (`https://skins.webamp.org/skin/<md5>/...`) are HTML
    pages; map them to `https://r2.webampskins.org/skins/<md5>.wsz`.
  * Validate by running the importer (`pledit.bmp` must decode) before
    installing; on failure print the reason and exit 1, nothing installed.
  * Copy to `<config_dir>/grimalkin/skins/<name>.wsz` (name = sanitized file
    stem: `[A-Za-z0-9._-]`, lowercase; refuse to overwrite without
    `--force`).
  * Print: `Installed. Add to grimalkin.toml:  skin = "winamp:<name>"`.
* Lookup: `skin = "winamp:<name>"` -> `<config_dir>/grimalkin/skins/<name>.wsz`
  (case-insensitive match on stem), also accept a path ending in `.wsz`. The
  `winamp:` skin also yields a theme; the config's `theme = ` still wins if
  set explicitly, and `[colors]` overrides apply as for any theme.
* Skins are never bundled in the repo (user-made content, licences unclear).

## 6. Sample skins (downloaded, NOT committed, used for tests only)

Directory: `/private/tmp/claude-501/-Users-rlankenau-src-grimalkin/7e1104fd-b3d4-4b91-9f6e-a4c1266c8385/scratchpad/wsz/`
(session scratch; re-download if gone). All are valid ZIPs with
`pledit.bmp`; none has magenta in it.

| File | Bytes | md5 | pledit.bmp | Notes / source |
|---|---|---|---|---|
| base-2.91.wsz | 102133 | 2f245e741b6bd4e1a122a28489f0c7b7 | `PLEDIT.BMP` 8-bit 280x186 | the classic default skin, Nullsoft; green on black. https://raw.githubusercontent.com/captbaritone/webamp/master/packages/webamp-demo/skins/base-2.91.wsz |
| Winamp3_Classified.wsz | 28323 | aec27704d023c2e21990cfc8f2dfb517 | `pledit.bmp` 24-bit 280x186 | dark grey; author unknown (Winamp3-style). https://r2.webampskins.org/skins/aec27704d023c2e21990cfc8f2dfb517.wsz (museum page https://skins.webamp.org/skin/aec27704d023c2e21990cfc8f2dfb517/) |
| MacOSXAqua1-5.wsz | 238877 | 88dbd4e043795c98625462a908a2d965 | `pledit.bmp` 24-bit | light skin, contact ak@smartftp.com per its pledit.txt; Normal black on white. webamp-demo/skins/MacOSXAqua1-5.wsz |
| Skinner_Atlas.wsz | 104992 | 185c5fe89f505e17746919888589b061 | `PLEDIT.bmp` 24-bit | light grey with cyan current; webamp-demo/skins/Skinner_Atlas.wsz |
| Green-Dimension-V2.wsz | 70273 | 4308a2fc648033bf5fe7c4d56a5c8823 | `PLEDIT.bmp` 24-bit | dark green, mixed-case names; webamp-demo/skins/Green-Dimension-V2.wsz |
| AmigaPPC-dark.wsz | 15934 | a9452d674f0be21d58e5b13f5b60ee26 | `pledit.bmp` 8-bit | red on white list (odd pledit.txt: dark colours as Normal/Current); webamp-demo/skins/AmigaPPC-dark.wsz |
| XMMS-Turquoise.wsz | 68942 | 46ab165b574d97e2e51b22cb47a7661c | `xmms-turquoise/Pledit.bmp` 32-bit 280x**190** | edge cases: **subfolder in zip, 32-bit, 190 px tall**; webamp-demo/skins/XMMS-Turquoise.wsz |

All webamp-demo URLs are under
`https://raw.githubusercontent.com/captbaritone/webamp/master/packages/webamp-demo/skins/`.
Authors are mostly not recorded in the archives; do not claim attribution.
The Skin Museum API (`POST https://api.webamp.org/graphql`,
`{search_skins(query:"...", first:3){md5 filename}}`) finds more by name;
`skins.webamp.org/skin/<md5>/<name>.wsz` returned an HTML page when fetched
with curl, so use the `r2.webampskins.org/skins/<md5>.wsz` URL for raw files.
**Do not commit** these files; tests download them on demand.

## 7. As implemented (deviations from the plan above)

Code: `crates/gui/src/skin/winamp.rs` with submodules `archive` (zip or
unpacked directory), `compose` (sprite table, frame/shade/sheet), `text`
(`pledit.txt`, `viscolor.txt`, derived theme) and `install` (CLI). All seven
sample skins load and were checked visually against their playlist windows.

* **API**: `winamp::from_wsz(bytes, name, font_ok) -> Result<Skin, String>`.
  The theme travels inside the skin (`Skin.theme: Option<SkinTheme>`), so there
  is no `(Skin, Theme)` tuple. `theme::resolve_for_skin` uses it as the base
  when `theme` is still `"default"`; `[colors]` overrides still win. The
  unit tests synthesise a 280x186 BMP/zip; real-skin tests read
  `GRIMALKIN_WSZ_SAMPLE_DIR` (default: the session scratch directory) and skip
  silently when the files are absent. `GRIMALKIN_WSZ_DUMP_DIR=dir cargo test
  dump_composed` writes the composed frame/shade/sheet PNGs for inspection.
* **Viewport inset**: the engine already draws the viewport over the 9-slice
  caps, so no engine change was needed for `viewport_inset` < slices; the only
  requirement is that the viewport covers the centre patch. Hence
  `viewport_inset.top` is 20 (not 23) and `bottom` is 38: Winamp's 3 px of
  padding above the list are simply list-background colour inside the viewport.
  The hidden part of the left and right columns is painted `NormalBG`.
* **Engine extensions** (additive, TOML-visible, see SKINS.md):
  `[titlebar] image = { sprite, size, align }` draws the 100x20 title art
  (read from the button sheet) centred on the titlebar, and
  `shaded_sprite` on a button picks another sprite while shaded (the shade
  button shows WinAMP's *expand* art on the shade bar). No title text is drawn.
* **Button sheet**: 100 x 47; columns close, shade, expand (9 px each), rows
  normal, hover, pressed; the title art sits at y = 27. Hover is the normal art
  (WinAMP has no hover art); pressed comes from the dedicated sprites. The
  normal art is cropped from the focused top-right corner (close at
  corner-x 14, shade at 4) and from the shade bar's right cap (expand at 29).
* **shade.png** is 300 px wide (not 275) so the engine's 3-slice with the
  frame's 125/150 caps keeps the 25 px left cap and 50 px right cap intact; the
  middle is the tiled `SHADE_BACKGROUND`.
* **Not used**: the unfocused variants (the engine has no focus state), the
  visualiser background, menu pop-up sprites, scroll thumb, `region.txt`,
  `main.bmp`. The frame is always opaque and rectangular.
* **Theme**: contrast rules as in section 3 (selection text falls back to
  `Current` when `Normal` has contrast < 3 on `SelectedBG`); additionally the
  cursor falls back to `Normal` when `Current` has contrast < 1.5 on `NormalBG`
  (a few skins use list-coloured `Current`). The `Font=` hint is applied only
  if the family is installed **and monospaced** (fontdb's flag; almost every
  skin says Arial, which would break the cell grid) and only while `[font]
  family` is the default. The probe (`fonts::monospace_family_installed`)
  uses the font index, so only matching font files are opened.
* **ANSI palette**: the config key is `winamp_viscolor_palette = true` (top
  level of `grimalkin.toml`), not `winamp_ansi`. `cyan` is
  `lerp(green, c[19], 0.5)` (the spec's `c[10]` alternative was dropped).
  Skins without `viscolor.txt` use the stock table.
* **Lookup**: besides `winamp:<name>` and a path to a `.wsz`, `winamp:<path>`
  and an unpacked directory (any depth up to 4) containing `pledit.bmp` (and no
  `skin.toml`) work. Names are matched against the skins directory listing,
  never joined into a path.
* **Limits**: 8 MiB per entry, 64 MiB inflated per archive (and as file size),
  20 000 entries, images at most 4096 x 4096. Entries are never written to disk.
* **CLI**: `--install-skin` accepts a local file or an `https://` URL
  (plain `http://` is refused), downloads with a 30 s timeout and a **20 MB**
  cap (not 8 MiB), requires `PK\x03\x04`, validates by running the importer,
  writes `<name>.wsz` via a temp file, refuses to overwrite without `--force`
  and prints `Installed. Add to grimalkin.toml:  skin = "winamp:<name>"`.
  Museum page links map to `r2.webampskins.org/skins/<md5>.wsz`; the name comes
  from the page URL when it has one, else the hash. `--list-skins` prints the
  built-ins, user skin folders and `winamp:<name>` for each installed `.wsz`.
* **Windows**: everything is portable code (no OS-specific parts). The release
  build uses the GUI subsystem, so like `--help` and `--list-themes` the CLI
  output of `--install-skin` is not visible from a console there; the exit
  code is correct.
