#!/usr/bin/env bash
# Build dist/grimalkin.app and dist/grimalkin-<version>-macos-<arch>.dmg (macOS only).
#
#   scripts/make-app-bundle.sh                    # cargo build --release for the host, then bundle
#   scripts/make-app-bundle.sh --target x86_64-apple-darwin
#                                                 # cross-build for another arch (rustup target add first)
#   scripts/make-app-bundle.sh --binary PATH      # bundle a prebuilt binary (use --arch with it
#                                                 # if it is not for the host arch)
#   scripts/make-app-bundle.sh --no-dmg           # stop after the .app
#
# The app is ad-hoc signed (no Developer ID, not notarized), which is enough to
# avoid the "damaged" error but still needs right-click > Open on first launch.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "make-app-bundle.sh only runs on macOS" >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

binary=""
target=""
arch=""
make_dmg=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) binary="${2:?--binary needs a path}"; shift 2 ;;
        --target) target="${2:?--target needs a triple}"; shift 2 ;;
        --arch) arch="${2:?--arch needs arm64 or x86_64}"; shift 2 ;;
        --no-dmg) make_dmg=0; shift ;;
        -h|--help) sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

# Version: [workspace.package] version in the root Cargo.toml.
version="$(awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]") }
    in_pkg && /^version[[:space:]]*=/ { gsub(/.*=[[:space:]]*"|".*/, ""); print; exit }
' Cargo.toml)"
if [[ -z "$version" ]]; then
    echo "could not read the workspace version from Cargo.toml" >&2
    exit 1
fi

if [[ -z "$arch" ]]; then
    case "$target" in
        aarch64-*) arch=arm64 ;;
        x86_64-*) arch=x86_64 ;;
        "") arch="$(uname -m)" ;;
        *) echo "unsupported --target $target" >&2; exit 2 ;;
    esac
fi
[[ "$arch" == "aarch64" ]] && arch=arm64

if [[ -z "$binary" ]]; then
    if [[ -n "$target" ]]; then
        cargo build --release --locked -p grimalkin --target "$target"
        binary="target/$target/release/grimalkin"
    else
        cargo build --release --locked -p grimalkin
        binary="target/release/grimalkin"
    fi
fi
if [[ ! -x "$binary" ]]; then
    echo "binary not found or not executable: $binary" >&2
    exit 1
fi

app="dist/grimalkin.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/grimalkin"
chmod 755 "$app/Contents/MacOS/grimalkin"
printf 'APPL????' > "$app/Contents/PkgInfo"

icon="assets/icon/grimalkin.icns"
icon_entry=""
if [[ -f "$icon" ]]; then
    cp "$icon" "$app/Contents/Resources/grimalkin.icns"
    icon_entry="
    <key>CFBundleIconFile</key>
    <string>grimalkin.icns</string>"
else
    echo "warning: $icon not found; building the bundle without an icon" >&2
fi

year="$(date +%Y)"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleName</key>
    <string>grimalkin</string>
    <key>CFBundleDisplayName</key>
    <string>grimalkin</string>
    <key>CFBundleIdentifier</key>
    <string>dev.grimalkin.terminal</string>
    <key>CFBundleExecutable</key>
    <string>grimalkin</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>${icon_entry}
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright (c) ${year} the grimalkin authors. MIT OR Apache-2.0.</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

# Ad-hoc signature: the arm64 kernel refuses unsigned code, and it keeps
# Gatekeeper's message at "unidentified developer" instead of "damaged".
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
echo "built $app (version $version, $arch)"

if [[ "$make_dmg" -eq 1 ]]; then
    dmg="dist/grimalkin-${version}-macos-${arch}.dmg"
    stage="$(mktemp -d)"
    trap 'rm -rf "$stage"' EXIT
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    rm -f "$dmg"
    hdiutil create -volname "grimalkin" -srcfolder "$stage" -format UDZO -ov "$dmg" >/dev/null
    echo "built $dmg"
fi
