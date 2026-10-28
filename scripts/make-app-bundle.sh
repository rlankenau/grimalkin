#!/usr/bin/env bash
# Build, sign and notarize dist/grimalkin.app and
# dist/grimalkin-<version>-macos-<arch>.dmg (macOS only).
#
#   scripts/make-app-bundle.sh                    # cargo build --release for the host, then bundle
#   scripts/make-app-bundle.sh --target x86_64-apple-darwin
#                                                 # cross-build for another arch (rustup target add first)
#   scripts/make-app-bundle.sh --binary PATH      # bundle a prebuilt binary (use --arch with it
#                                                 # if it is not for the host arch)
#   scripts/make-app-bundle.sh --no-dmg           # stop after the .app
#   scripts/make-app-bundle.sh --no-notarize      # sign but do not notarize
#
# Signing (environment variables):
#   GRIMALKIN_SIGN_IDENTITY   codesign identity. Default: the first "Developer ID
#                             Application" identity in the keychain search list.
#                             With no such identity, or the value "-", the bundle
#                             is ad-hoc signed (not notarized; needs right-click >
#                             Open on first launch).
#
# Notarization (only with a Developer ID identity; otherwise skipped with a message):
#   GRIMALKIN_NOTARY_PROFILE  notarytool keychain profile name
#                             (xcrun notarytool store-credentials <name> ...)
#   GRIMALKIN_NOTARY_KEY_PATH + GRIMALKIN_NOTARY_KEY_ID + GRIMALKIN_NOTARY_ISSUER
#                             App Store Connect API key (.p8 path, key id, issuer
#                             uuid) for CI; takes precedence over the profile.
#
# With notarization the app is notarized and stapled first, then packed into the
# dmg, and the dmg is signed, notarized and stapled too (two submissions), so
# both work offline.
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
notarize=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) binary="${2:?--binary needs a path}"; shift 2 ;;
        --target) target="${2:?--target needs a triple}"; shift 2 ;;
        --arch) arch="${2:?--arch needs arm64 or x86_64}"; shift 2 ;;
        --no-dmg) make_dmg=0; shift ;;
        --no-notarize) notarize=0; shift ;;
        -h|--help) sed -n '2,28p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
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

# --- signing configuration --------------------------------------------------

identity="${GRIMALKIN_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
    # Hash of the first valid Developer ID Application identity, if any.
    identity="$(security find-identity -v -p codesigning 2>/dev/null \
        | awk '/"Developer ID Application:/ { print $2; exit }')" || true
    identity="${identity:--}"
fi
if [[ "$identity" == "-" ]]; then
    developer_id=0
    echo "signing: ad-hoc (no Developer ID identity; set GRIMALKIN_SIGN_IDENTITY to sign)"
else
    developer_id=1
    echo "signing: identity $identity"
fi

notary_args=()
notary_desc=""
if [[ -n "${GRIMALKIN_NOTARY_KEY_PATH:-}" && -n "${GRIMALKIN_NOTARY_KEY_ID:-}" && -n "${GRIMALKIN_NOTARY_ISSUER:-}" ]]; then
    notary_args=(--key "$GRIMALKIN_NOTARY_KEY_PATH" --key-id "$GRIMALKIN_NOTARY_KEY_ID" --issuer "$GRIMALKIN_NOTARY_ISSUER")
    notary_desc="App Store Connect API key $GRIMALKIN_NOTARY_KEY_ID"
elif [[ -n "${GRIMALKIN_NOTARY_PROFILE:-}" ]]; then
    notary_args=(--keychain-profile "$GRIMALKIN_NOTARY_PROFILE")
    notary_desc="keychain profile $GRIMALKIN_NOTARY_PROFILE"
fi

do_notarize=0
if [[ "$notarize" -eq 0 ]]; then
    echo "notarization: skipped (--no-notarize)"
elif [[ "$developer_id" -eq 0 ]]; then
    echo "notarization: skipped (needs a Developer ID signature)"
elif [[ ${#notary_args[@]} -eq 0 ]]; then
    echo "notarization: skipped (set GRIMALKIN_NOTARY_PROFILE, or GRIMALKIN_NOTARY_KEY_PATH/_KEY_ID/_ISSUER)"
else
    do_notarize=1
    echo "notarization: enabled ($notary_desc)"
fi

# Submit a file to the notary service and wait. Prints the submission id and
# status; on anything but Accepted prints the notary log and exits.
notarize_file() {
    local file="$1" out id status started
    started=$SECONDS
    echo "notarizing $file ..."
    out="$(xcrun notarytool submit "$file" "${notary_args[@]}" --wait --output-format json)" || true
    id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
    status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
    echo "notarization submission id: ${id:-unknown}"
    echo "notarization status: ${status:-unknown} ($((SECONDS - started))s)"
    if [[ "$status" != "Accepted" ]]; then
        echo "notarization failed; raw response:" >&2
        echo "$out" >&2
        if [[ -n "$id" ]]; then
            xcrun notarytool log "$id" "${notary_args[@]}" >&2 || true
        fi
        exit 1
    fi
}

# --- sign the app -----------------------------------------------------------

if [[ "$developer_id" -eq 1 ]]; then
    # Inside-out, no --deep: the executable first, then the bundle. Hardened
    # runtime needs no entitlements for a terminal (no sandbox, no JIT).
    sign_args=(--force --options runtime --timestamp --sign "$identity")
    codesign "${sign_args[@]}" "$app/Contents/MacOS/grimalkin"
    codesign "${sign_args[@]}" "$app"
    codesign --verify --deep --strict --verbose=2 "$app"
    if ! spctl --assess --type execute --verbose=2 "$app"; then
        # Expected before notarization; only noteworthy when we will not notarize.
        [[ "$do_notarize" -eq 1 ]] || echo "warning: Gatekeeper rejects $app (not notarized)" >&2
    fi
else
    # Ad-hoc signature: the arm64 kernel refuses unsigned code, and it keeps
    # Gatekeeper's message at "unidentified developer" instead of "damaged".
    codesign --force --deep --sign - "$app"
    codesign --verify --deep --strict "$app"
fi

if [[ "$do_notarize" -eq 1 ]]; then
    zipdir="$(mktemp -d)"
    ditto -c -k --keepParent "$app" "$zipdir/grimalkin.zip"
    notarize_file "$zipdir/grimalkin.zip"
    rm -rf "$zipdir"
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
    spctl --assess --type execute --verbose=2 "$app"
fi
echo "built $app (version $version, $arch)"

if [[ "$make_dmg" -eq 1 ]]; then
    dmg="dist/grimalkin-${version}-macos-${arch}.dmg"
    stage="$(mktemp -d)"
    trap 'rm -rf "$stage"' EXIT
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    rm -f "$dmg"
    hdiutil create -volname "grimalkin" -srcfolder "$stage" -format UDZO -ov "$dmg" >/dev/null
    if [[ "$developer_id" -eq 1 ]]; then
        codesign --force --timestamp --sign "$identity" "$dmg"
        codesign --verify --verbose=2 "$dmg"
    fi
    if [[ "$do_notarize" -eq 1 ]]; then
        notarize_file "$dmg"
        xcrun stapler staple "$dmg"
        xcrun stapler validate "$dmg"
        spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
    fi
    echo "built $dmg"
fi
