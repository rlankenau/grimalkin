#!/usr/bin/env bash
# Rewrite packaging/homebrew/grimalkin.rb (or CASK) for a release tag.
#
#   packaging/homebrew/update-cask.sh v1.0.0 [CASK]
#   DMG_DIR=/path/with/dmgs packaging/homebrew/update-cask.sh v1.0.0 [CASK]
#
# Downloads grimalkin-<version>-macos-{arm64,x86_64}.dmg from the GitHub release
# (or takes them from $DMG_DIR), computes their sha256 and updates `version` and
# both `sha256` values (and the repository in the download URL when REPO is set). Needs curl (or DMG_DIR), shasum/sha256sum, sed.
# REPO overrides the GitHub repository (default rlankenau/grimalkin).
set -euo pipefail

tag="${1:?usage: update-cask.sh vX.Y.Z [cask-file]}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cask="${2:-$here/grimalkin.rb}"
repo="${REPO:-rlankenau/grimalkin}"

[[ "$tag" == v* ]] || { echo "tag must start with v (got $tag)" >&2; exit 2; }
version="${tag#v}"
[[ "$version" =~ ^[0-9]+(\.[0-9]+)*(-[0-9A-Za-z.-]+)?$ ]] || { echo "unexpected version: $version" >&2; exit 2; }
[[ -f "$cask" ]] || { echo "no such cask file: $cask" >&2; exit 2; }

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

work=""
if [[ -z "${DMG_DIR:-}" ]]; then
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    DMG_DIR="$work"
    for a in arm64 x86_64; do
        f="grimalkin-${version}-macos-${a}.dmg"
        echo "downloading $f" >&2
        curl -fsSL --retry 3 -o "$DMG_DIR/$f" "https://github.com/$repo/releases/download/$tag/$f"
    done
fi

hashes=()
for a in arm64 x86_64; do
    f="$DMG_DIR/grimalkin-${version}-macos-${a}.dmg"
    [[ -s "$f" ]] || { echo "missing $f" >&2; exit 1; }
    h="$(sha256 "$f")"
    [[ "$h" =~ ^[0-9a-f]{64}$ ]] || { echo "bad hash for $f" >&2; exit 1; }
    hashes+=("$h")
done
arm="${hashes[0]}"
intel="${hashes[1]}"

tmp="$(mktemp)"
sed -E \
    -e "s/^(  version \")[^\"]*(\")/\1${version}\2/" \
    -e "s/^(  sha256 arm: +\")[0-9a-f]*(\",)/\1${arm}\2/" \
    -e "s/^(         intel: \")[0-9a-f]*(\")/\1${intel}\2/" \
    -e "s#github\.com/[^/]+/[^/]+/releases/download/#github.com/${repo}/releases/download/#" \
    "$cask" > "$tmp"
cat "$tmp" > "$cask"
rm -f "$tmp"

grep -q "version \"${version}\"" "$cask" && grep -q "$arm" "$cask" && grep -q "$intel" "$cask" \
    || { echo "failed to update $cask" >&2; exit 1; }
echo "updated $cask to $version (arm64 $arm, x86_64 $intel)" >&2
