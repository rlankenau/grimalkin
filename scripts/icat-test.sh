#!/usr/bin/env bash
# Reproduces the manual kitty-graphics checks with `kitten icat`.
#
# usage: scripts/icat-test.sh [case...]
#   cases: basic place alpha big bigraw scroll clear zindex altscreen all (default: all)
#
# Environment:
#   MODE=dump    (default) render offscreen and write each case's last frame to
#                $OUT/<case>.png through GRIMALKIN_DUMP_FRAME. Works with a
#                locked screen or over ssh.
#   MODE=screen  open the real window, wait, and `screencapture` the display to
#                $OUT/<case>.png (needs an unlocked desktop on macOS).
#   OUT=dir      output directory (default: $TMPDIR/grimalkin-icat-test)
#   WAIT=secs    how long each case runs before it is captured/killed (default 6)
#   PROFILE=release|debug   which build to run (default debug)
#
# Requires: kitten (kitty's tool), python3 with Pillow (to generate test images).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out=${OUT:-${TMPDIR:-/tmp}/grimalkin-icat-test}
mode=${MODE:-dump}
wait_s=${WAIT:-6}
profile=${PROFILE:-debug}
mkdir -p "$out/img"

command -v kitten >/dev/null || { echo "kitten not found (install kitty)" >&2; exit 1; }

if [ "$profile" = release ]; then
  (cd "$root" && cargo build --release -p grimalkin)
else
  (cd "$root" && cargo build -p grimalkin)
fi
bin="$root/target/$profile/grimalkin"

# --- test images -------------------------------------------------------------
img="$out/img"
if [ ! -f "$img/test.png" ] || [ ! -f "$img/alpha.png" ] || [ ! -f "$img/big.png" ]; then
  python3 - "$img" <<'EOF'
import sys
from PIL import Image, ImageDraw
d = sys.argv[1]
W, H = 320, 200
im = Image.new("RGB", (W, H)); px = im.load()
for y in range(H):
    for x in range(W):
        px[x, y] = (x * 255 // W, y * 255 // H, 128)
g = ImageDraw.Draw(im)
g.rectangle([0, 0, W - 1, H - 1], outline=(255, 255, 255), width=2)
g.line([0, 0, W, H], fill=(0, 0, 0), width=2); g.line([0, H, W, 0], fill=(0, 0, 0), width=2)
g.ellipse([110, 60, 210, 140], outline=(255, 255, 0), width=4)
g.text((10, 10), "TOPLEFT", fill=(255, 255, 255)); g.text((W - 70, H - 20), "BOTRIGHT", fill=(255, 255, 255))
im.save(f"{d}/test.png")
t = Image.new("RGBA", (300, 200), (0, 0, 0, 0)); g = ImageDraw.Draw(t)
g.ellipse([20, 20, 280, 180], fill=(255, 0, 0, 255))
g.ellipse([80, 60, 220, 140], fill=(0, 0, 255, 128))
g.rectangle([0, 0, 60, 60], fill=(0, 255, 0, 90))
t.save(f"{d}/alpha.png")
B = Image.new("RGB", (4000, 3000)); pb = B.load()
for y in range(3000):
    for x in range(4000):
        pb[x, y] = ((x // 100 * 40) % 256, (y // 100 * 60) % 256, ((x + y) // 50 * 7) % 256)
g = ImageDraw.Draw(B)
g.rectangle([0, 0, 3999, 2999], outline=(255, 255, 255), width=30)
g.ellipse([1000, 750, 3000, 2250], outline=(255, 255, 255), width=40)
B.save(f"{d}/big.png")
EOF
fi

# --- cases: the shell snippet each one runs inside the terminal ---------------
case_cmd() {
  case "$1" in
    basic)  echo "echo BEFORE-IMAGE; kitten icat --transfer-mode=stream '$img/test.png'; echo AFTER-IMAGE" ;;
    place)  echo "printf '\\033[H\\033[2J'; kitten icat --transfer-mode=stream --place 40x20@5x3 '$img/test.png'; printf '\\033[25;1Hprompt after place\\n'" ;;
    alpha)  echo "for i in 1 2 3 4 5 6 7 8 9 10 11 12; do printf '\\033[44;97m%-100s\\033[0m\\n' 'text behind a transparent png ....'; done; kitten icat --transfer-mode=stream --place 30x10@3x1 '$img/alpha.png'; printf '\\033[14;1Hdone'" ;;
    big)    echo "echo BIG-4000x3000; kitten icat --transfer-mode=stream '$img/big.png'; echo AFTER-BIG" ;;
    bigraw) echo "echo RAW-4000x3000-CHUNKED; python3 '$root/scripts/kitty-send.py' '$img/big.png' 60 20; echo AFTER-BIGRAW" ;;
    scroll) echo "echo TOP; kitten icat --transfer-mode=stream '$img/test.png'; seq 1 25; echo LAST" ;;
    clear)  echo "kitten icat --transfer-mode=stream '$img/test.png'; sleep 1; printf '\\033[H\\033[2J'; echo CLEARED-NO-IMAGE-EXPECTED" ;;
    zindex) echo "for i in \$(seq 1 14); do printf '\\033[42;30m%-100s\\033[0m\\n' 'text must stay readable ON TOP of the image (z=-1)'; done; kitten icat --transfer-mode=stream --z-index=-1 --place 30x12@10x0 '$img/test.png'; printf '\\033[16;1Hdone'" ;;
    altscreen) echo "echo MAIN-SCREEN; printf '\\033[?1049h\\033[H'; kitten icat --transfer-mode=stream '$img/test.png'; sleep 2; printf '\\033[?1049l'; echo BACK-ON-MAIN-NO-IMAGE-EXPECTED; sleep 30" ;;
    *) echo "unknown case $1" >&2; return 1 ;;
  esac
}

run_case() {
  local name=$1 cmd png
  cmd=$(case_cmd "$name")
  png="$out/$name.png"
  rm -f "$png"
  echo "== $name: $cmd"
  if [ "$mode" = screen ]; then
    "$bin" -e sh -c "$cmd; sleep $((wait_s + 5))" &
    local pid=$!
    sleep "$wait_s"
    screencapture -x "$png" || echo "screencapture failed (screen locked?)" >&2
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  else
    GRIMALKIN_DUMP_FRAME="$png" "$bin" -e sh -c "$cmd; sleep $((wait_s + 5))" &
    local pid=$!
    sleep "$wait_s"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  [ -f "$png" ] && echo "   -> $png" || echo "   (no output written)" >&2
}

cases=("$@")
if [ ${#cases[@]} -eq 0 ] || [ "${cases[0]}" = all ]; then
  cases=(basic place alpha big bigraw scroll clear zindex altscreen)
fi
for c in "${cases[@]}"; do run_case "$c"; done
