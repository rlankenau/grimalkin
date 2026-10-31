#!/usr/bin/env python3
"""Generate the MacPorts `cargo.crates` block from Cargo.lock (stdlib only).

    gen-crates.py [--lock Cargo.lock]                 # print the block
    gen-crates.py --update Portfile [--lock Cargo.lock]
                                                       # rewrite the block in the Portfile

The block sits between the marker lines "# BEGIN cargo.crates" and
"# END cargo.crates" in the Portfile. Every crate from the crates.io registry in
the lock file is listed (name, version, sha256 checksum), the format the MacPorts
`cargo` 1.0 PortGroup expects; workspace members and git/path dependencies have no
checksum and are not listed (this workspace has none of the latter).
"""
import argparse
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
BEGIN = "# BEGIN cargo.crates"
END = "# END cargo.crates"


def crates(lock_path):
    with open(lock_path, encoding="utf-8") as fh:
        text = fh.read()
    out = []
    for block in text.split("[[package]]")[1:]:
        fields = dict(re.findall(r'(?m)^(name|version|source|checksum) = "([^"]*)"', block))
        src = fields.get("source", "")
        if not src:
            continue  # workspace member
        if not src.startswith("registry+https://github.com/rust-lang/crates.io-index"):
            sys.exit("error: %s comes from %s; the cargo PortGroup only fetches crates.io crates"
                     % (fields.get("name"), src))
        out.append((fields["name"], fields["version"], fields["checksum"]))
    return sorted(out)


def block(entries):
    lines = ["cargo.crates \\"]
    for i, (name, ver, sha) in enumerate(entries):
        cont = " \\" if i + 1 < len(entries) else ""
        lines.append("    %-32s %9s  %s%s" % (name, ver, sha, cont))
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--lock", default=os.path.join(HERE, "..", "..", "Cargo.lock"))
    ap.add_argument("--update", metavar="PORTFILE")
    args = ap.parse_args()
    text = block(crates(args.lock))
    if not args.update:
        print(text)
        return
    with open(args.update, encoding="utf-8") as fh:
        port = fh.read()
    m = re.search(r"(?ms)^%s\n.*?^%s$" % (re.escape(BEGIN), re.escape(END)), port)
    if not m:
        sys.exit("error: marker lines not found in " + args.update)
    port = port[:m.start()] + BEGIN + "\n" + text + "\n" + END + port[m.end():]
    with open(args.update, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(port)
    print("updated %s (%d crates)" % (args.update, text.count("\n")), file=sys.stderr)


if __name__ == "__main__":
    main()
