#!/usr/bin/env python3
"""Write the winget manifests for a grimalkin release (Python 3 standard library only).

    update-manifest.py v1.0.0 [--repo OWNER/REPO] [--out-dir DIR] [--urls-file FILE]
    update-manifest.py v1.0.0 --x64 PATH [--arm64 PATH]   # hash local installers

Default mode asks the GitHub API for the release (GITHUB_TOKEN is used if set, which
only raises the rate limit; the release is public), downloads the Windows installer
assets and writes the three manifest files to
packaging/winget/manifests/<version>/ (or --out-dir). Asset names tried per
architecture: grimalkin-setup-<arch>.exe (signed releases), then
grimalkin-setup-<arch>-unsigned.exe. An architecture whose installer is not on
the release is left out of the manifest with a warning (at least one is needed).

With --x64/--arm64 the installers are read from disk instead of downloaded; the
URLs written to the manifest are still the release download URLs (built from the
tag and the file's basename), so run it against the exact files you uploaded.
--release-date YYYY-MM-DD sets ReleaseDate in that mode (the API supplies it otherwise).

--urls-file FILE writes one "<url>|<arch>" line per installer, in the form
`wingetcreate update --urls` takes.
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
TEMPLATES = os.path.join(HERE, "templates")
ARCHES = ("x64", "arm64")
FILES = (
    "rlankenau.grimalkin.yaml",
    "rlankenau.grimalkin.installer.yaml",
    "rlankenau.grimalkin.locale.en-US.yaml",
)
# winget package versions are free-form strings, but keep to what release tags use.
VERSION_RE = re.compile(r"^[0-9]+(\.[0-9]+)*(-[0-9A-Za-z.-]+)?$")
SHA_RE = re.compile(r"^[0-9a-f]{64}$")


def fail(msg):
    print("error: " + msg, file=sys.stderr)
    sys.exit(1)


def http(url, token=None, accept=None):
    req = urllib.request.Request(url)
    req.add_header("User-Agent", "grimalkin-update-manifest")
    if accept:
        req.add_header("Accept", accept)
    if token and url.startswith("https://api.github.com/"):
        req.add_header("Authorization", "Bearer " + token)
    return urllib.request.urlopen(req, timeout=120)


def sha256_stream(fh):
    h = hashlib.sha256()
    for chunk in iter(lambda: fh.read(1 << 20), b""):
        h.update(chunk)
    return h.hexdigest()


def sha256_file(path):
    with open(path, "rb") as fh:
        return sha256_stream(fh)


def sha256_url(url):
    with http(url) as resp:
        return sha256_stream(resp)


def asset_url(repo, tag, name):
    return "https://github.com/%s/releases/download/%s/%s" % (repo, tag, name)


def from_release(repo, tag, token):
    api = "https://api.github.com/repos/%s/releases/tags/%s" % (repo, tag)
    try:
        with http(api, token, "application/vnd.github+json") as resp:
            rel = json.load(resp)
    except urllib.error.HTTPError as e:
        fail("cannot read release %s of %s: HTTP %d" % (tag, repo, e.code))
    if rel.get("draft"):
        fail("release %s is a draft; its assets are not publicly downloadable" % tag)
    assets = {a["name"]: a for a in rel.get("assets", [])}
    found = {}
    for arch in ARCHES:
        for name in ("grimalkin-setup-%s.exe" % arch, "grimalkin-setup-%s-unsigned.exe" % arch):
            if name in assets:
                a = assets[name]
                url = a["browser_download_url"]
                print("hashing %s ..." % name, file=sys.stderr)
                digest = sha256_url(url)
                api_digest = (a.get("digest") or "")
                if api_digest.startswith("sha256:") and api_digest[7:] != digest:
                    fail("%s: downloaded sha256 %s differs from the release API digest %s"
                         % (name, digest, api_digest[7:]))
                found[arch] = (url, digest)
                break
        else:
            print("warning: no %s installer on release %s; leaving %s out" % (arch, tag, arch), file=sys.stderr)
    date = (rel.get("published_at") or "")[:10]
    return found, date


def from_local(repo, tag, paths):
    found = {}
    for arch, path in paths.items():
        if not os.path.isfile(path):
            fail("%s is not a file" % path)
        found[arch] = (asset_url(repo, tag, os.path.basename(path)), sha256_file(path))
    return found


def keep_architectures(installer_text, wanted):
    """Drop the installer entries of architectures that are not in `wanted`."""
    head, _, rest = installer_text.partition("Installers:\n")
    body, tail_marker, tail = rest.partition("ManifestType: installer")
    entries = re.split(r"(?m)^(?=  - Architecture: )", body)
    kept = [e for e in entries if e.strip() == "" or re.match(r"  - Architecture: (\S+)", e).group(1) in wanted]
    return head + "Installers:\n" + "".join(kept) + tail_marker + tail


def render(version, found, date):
    values = {"VERSION": version}
    for arch in ARCHES:
        url, digest = found.get(arch, ("", ""))
        values[arch.upper() + "_URL"] = url
        values[arch.upper() + "_SHA256"] = digest.upper()  # winget-pkgs convention
    values["RELEASEDATE_LINE"] = ("ReleaseDate: %s" % date) if date else "# ReleaseDate: unknown"
    out = {}
    for fname in FILES:
        with open(os.path.join(TEMPLATES, fname + ".in"), encoding="utf-8") as fh:
            text = fh.read()
        if fname.endswith(".installer.yaml"):
            text = keep_architectures(text, set(found))
        for key, val in values.items():
            text = text.replace("@%s@" % key, val)
        left = re.findall(r"@[A-Z0-9_]+@", text)
        if left:
            fail("unreplaced placeholders in %s: %s" % (fname, ", ".join(sorted(set(left)))))
        out[fname] = text
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tag", help="release tag, e.g. v1.0.0 or v1.0.0-rc3")
    ap.add_argument("--repo", default="rlankenau/grimalkin")
    ap.add_argument("--out-dir", help="default: packaging/winget/manifests/<version>")
    ap.add_argument("--x64", metavar="PATH", help="local x64 installer instead of downloading")
    ap.add_argument("--arm64", metavar="PATH", help="local arm64 installer instead of downloading")
    ap.add_argument("--release-date", metavar="YYYY-MM-DD")
    ap.add_argument("--urls-file", metavar="FILE")
    args = ap.parse_args()

    if not args.tag.startswith("v"):
        fail("tag must start with 'v' (got %r)" % args.tag)
    version = args.tag[1:]
    if not VERSION_RE.match(version):
        fail("unexpected version %r" % version)

    local = {a: p for a, p in (("x64", args.x64), ("arm64", args.arm64)) if p}
    if local:
        found = from_local(args.repo, args.tag, local)
        date = args.release_date or ""
        if date:
            datetime.date.fromisoformat(date)
    else:
        found, date = from_release(args.repo, args.tag, os.environ.get("GITHUB_TOKEN"))
    if not found:
        fail("no installers found")
    for arch, (_, digest) in found.items():
        if not SHA_RE.match(digest):
            fail("bad sha256 for %s" % arch)

    out_dir = args.out_dir or os.path.join(HERE, "manifests", version)
    os.makedirs(out_dir, exist_ok=True)
    for fname, text in render(version, found, date).items():
        with open(os.path.join(out_dir, fname), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        print("wrote " + os.path.join(out_dir, fname), file=sys.stderr)
    if args.urls_file:
        with open(args.urls_file, "w", encoding="utf-8", newline="\n") as fh:
            for arch in ARCHES:
                if arch in found:
                    fh.write("%s|%s\n" % (found[arch][0], arch))


if __name__ == "__main__":
    main()
