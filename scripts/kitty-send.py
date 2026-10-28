#!/usr/bin/env python3
"""Send a PNG to the terminal with the kitty graphics protocol, unscaled, in
chunked direct transmission (4096-byte base64 chunks) like a real large upload.

usage: kitty-send.py FILE.png [COLS [ROWS]]

Unlike `kitten icat`, nothing is resized on the client side, so a 4000x3000
PNG really travels to the terminal at full size. COLS/ROWS (kitty `c`/`r`) let
the terminal scale it down to a number of cells.
"""
import base64
import sys

path = sys.argv[1]
extra = ""
if len(sys.argv) > 2:
    extra += f",c={sys.argv[2]}"
if len(sys.argv) > 3:
    extra += f",r={sys.argv[3]}"
data = base64.standard_b64encode(open(path, "rb").read()).decode("ascii")
out = sys.stdout
chunks = [data[i : i + 4096] for i in range(0, len(data), 4096)]
for i, chunk in enumerate(chunks):
    more = 1 if i + 1 < len(chunks) else 0
    ctrl = f"a=T,f=100,q=2{extra}," if i == 0 else ""
    out.write(f"\x1b_G{ctrl}m={more};{chunk}\x1b\\")
out.write("\n")
out.flush()
