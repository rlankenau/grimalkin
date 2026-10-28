# Built-in skins

Generated, do not edit by hand: `python3 scripts/gen-skins.py` (pure Python 3,
deterministic) rewrites everything here, `python3 scripts/check-skins.py
[--montage DIR]` validates it and renders contact sheets. See `docs/SKINS.md`
for the skin format.

| skin | frame | slices (l/r/t/b) | shape |
|------|-------|------------------|-------|
| `llama` | 320x240 | 24/24/32/32 | 45 degree cut top corners and bottom-left, raised bottom edge with a 3-step wing (the resize grip) bottom-right |
| `grimalkin` | 320x254 | 28/28/54/40 | cat ears (rows 0..22 are transparent between them), rounded bottom-left, curled tail hanging below the body bottom-right (grip) |
| `mint` | 320x244 | 16/24/28/24 | 12px rounded corners, 8px dithered drop shadow baked into the right/bottom (alpha 180-200, the rest 0) |

Conventions the artwork relies on (and the engine should honour):

* `slices` equal `viewport_inset`; everything inside the viewport rectangle is
  opaque (hidden under the grid). Mask rule: alpha < 128 is outside the window.
  Only mint has partial alpha (shadow pixels, all >= 128 so they stay inside
  the shape and on Windows render as opaque dark pixels).
* `edge_mode = "tile"`: the tileable spans (top/bottom strip between the left and
  right slices, left/right walls between the top and bottom slices) repeat
  every 8 px exactly, alpha included, so tiling or stretching never shows a seam.
  All features that are not 8 px periodic (title trough ends, notches, ears,
  tail, rounded corners, grips) live inside the corner slices.
* `buttons.png`: one column per button in the order close, minimize, shade,
  maximize; each cell is exactly the `rect` size; rows are normal / hover /
  pressed, top to bottom (llama 14x14, grimalkin 16x16, mint 12x12). For
  grimalkin the close and minimize buttons are amber eyes: hover = blink.
* `shade.png`: 320 px wide (same as the frame), height = the top slice. It is
  the top rows of the frame art closed off along the bottom with a bevel and
  cut corners. Because it is as tall as the top slice, the `[titlebar]` and
  `[buttons]` rects apply to it unchanged. The left/right slice-wide caps are
  fixed and the middle repeats every 8 px. For `grimalkin` the top 22 rows are
  transparent except for the ears.
* `[titlebar].text`: `x` is the left edge, a negative `w` means the right edge of
  the text box is `frame_width + w` (the box stops where the buttons start);
  `y` is the top of the text line inside the trough/plate.
* `preview.png` (160x60): top-right crop of a mock-up window on a checkerboard.
