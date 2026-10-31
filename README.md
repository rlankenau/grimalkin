# Grimalkin

A GPU-accelerated terminal emulator written in Rust, aimed at Windows 10/11
(ConPTY) with the kitty graphics and keyboard protocols. It also builds and runs
on macOS and Linux, which is where it is mostly developed. Early software: the
core works, many conveniences are missing.

## What works today

- wgpu renderer with a glyph atlas, system font fallback (emoji, CJK, symbols),
  procedural box-drawing/block characters, wide chars and combining marks
- Terminal emulation: main/alt screen, scrollback (main screen), scroll regions,
  256/truecolor, underline styles and colors, mouse reporting (SGR), focus
  events, bracketed paste, synchronized output, OSC 0/2/4/7/8/10/11/12/52/133
- Kitty graphics protocol: direct/file/temp-file transmission, PNG and raw
  RGB(A), zlib, chunking, placement, source rects, z-index (above/below text),
  deletion, queries, images scrolling with text
- Kitty keyboard protocol (flag stack, query, key encoding)
- Selection, clipboard copy/paste, clickable OSC 8 hyperlinks, visual bell
  (optional audible), cursor shapes/blink, runtime zoom
- Themes (TOML or kitty `.conf`) with live config reload; TOML config: fonts, colors, window size/padding/opacity/title, cursor, bell, OSC 52
- Skins: undecorated, non-rectangular WinAMP-style windows (`--skin llama|grimalkin|mint`, or your own; see docs/SKINS.md)

Not done: no tabs or splits, no ligatures, no IME, no `U=1` unicode
placeholders, no animation frames (`a=f`).

## Installing on Windows

### Which download do I need?

Releases have one installer and one portable zip per CPU architecture:

| Your Windows | Download | Runs as |
| --- | --- | --- |
| Windows 10 or 11, Intel/AMD 64-bit (x64) | `grimalkin-setup-x64.exe` or `grimalkin-portable-x64.zip` | native x64 |
| Windows 11 on ARM64 (Snapdragon X, Windows Dev Kit, Parallels/VMware on Apple Silicon, ...) | `grimalkin-setup-arm64.exe` or `grimalkin-portable-arm64.zip` | native ARM64 (recommended) |
| Windows 11 on ARM64, x64 build | `grimalkin-setup-x64.exe` | x64 emulation (the fallback; may be slower) |
| Windows 10 on ARM64 | none | not supported (Windows 10 ARM64 has no x64 emulation, and the native build is only validated on Windows 11) |
| 32-bit x86 Windows | none | not supported |

If unsure: Settings, System, About, "System type" says `x64-based processor` or
`ARM-based processor`. The two builds have the same features. The x64 build keeps its
Windows 10 1809 minimum; the ARM64 minimum is Windows 11 until validated
otherwise. Each build bundles the ConPTY (`conpty.dll` and `OpenConsole.exe`)
of its own architecture.

> **ARM64 status:** the ARM64 build is compiled, packaged and install-tested on
> a native Windows 11 ARM64 CI runner, and the library test suite runs there.
> Interactive validation on real ARM64 hardware (GPU rendering, Preferences,
> DPI changes, clipboard, skins, kitty graphics and keyboard through the bundled
> ConPTY) is still in progress, tracked in
> [issue #1](https://github.com/rlankenau/grimalkin/issues/1). Report anything
> odd there; the x64 build under emulation is the known-good fallback.

### Installing

Download the installer for your machine from the
[Releases](https://github.com/rlankenau/grimalkin/releases) page and run it. Releases
are Authenticode-signed once release signing is set up; files named `...-unsigned` are
not signed and Windows will warn about an unknown publisher (see
[Windows release signing](#windows-release-signing)). It
installs per user by default (into `%LOCALAPPDATA%\Programs\grimalkin`, no
administrator prompt); the first page offers "install for all users" if you
want Program Files instead. The installer:

- installs `grimalkin.exe` and `grimalkin.com` (the console launcher, see
  [Windows: two files](#windows-two-files)) together, plus Microsoft's ConPTY
  redistributable (`conpty.dll` and `OpenConsole.exe`) beside them. The bundled
  ConPTY is what makes the kitty graphics protocol work on Windows 10 (see
  [Windows: kitty graphics need a newer ConPTY](#windows-kitty-graphics-need-a-newer-conpty));
  nothing extra to download
- adds a Start Menu shortcut, and optionally a desktop icon
- optionally adds grimalkin to your `PATH` so `grimalkin` works from any shell
- optionally adds an "Open grimalkin here" entry to Explorer's folder and
  folder-background right-click menus (it runs
  `grimalkin.exe --working-directory "<folder>"`)
- can launch grimalkin when it finishes
- upgrades in place: run a newer installer over an old one (a running grimalkin
  is closed first); uninstall from Settings, Apps, which also removes the PATH
  entry and context-menu keys, and the font cache (`%LOCALAPPDATA%\grimalkin\fonts.cache`). Your config and skins in `%APPDATA%\grimalkin` are kept

Silent install, for scripts: `grimalkin-setup-<arch>.exe /VERYSILENT /TASKS="addtopath,contextmenu"`
(add `/ALLUSERS` for a machine-wide install).

Prefer no installer? `grimalkin-portable-<arch>.zip` has the same files
(including the ConPTY pair); unzip it anywhere and run `grimalkin.exe`.

**Switching between x64 and ARM64 (for example from an emulated x64 install to
the native ARM64 build).** Both architectures share one installer identity,
install folder, `PATH` entry and context-menu keys, so only one can be installed
at a time and the installer enforces that. Run the new installer without
administrator rights (the default, per-user): it finds the other-architecture
install and offers to uninstall it first (Yes continues, No cancels without
changing anything). That uninstall removes the old files, `PATH` entry and
context-menu keys; the new installer then installs the new ones, so tick the same
optional tasks again. Your config and skins in `%APPDATA%\grimalkin` are kept.
For scripted installs, the installer refuses to replace an install of the other
architecture unless you pass `/REPLACEARCH=1` (exit code 1 otherwise).

For safety, an installer that is running **elevated** (as administrator, for
example `/ALLUSERS` or "install for all users") never runs the uninstaller of a
*per-user* copy, because that command comes from your own user registry and
could otherwise be used to run something as administrator. It only replaces an
*all-users* install whose uninstaller is in `Program Files`. If an elevated
installer finds a per-user install of the other architecture, it stops without
changing anything (even with `/REPLACEARCH=1`): uninstall that copy from
Settings, Apps first, or run the installer again without administrator rights.
Limitation: if a standard user elevates with a *different* administrator's
credentials, the installer cannot see the standard user's per-user install, so
remove a per-user install of the other architecture yourself before installing
for all users that way.

Installs made before ARM64
existed are x64 by definition. Portable zips never interfere with the installer
or each other; keep them in different folders. To go back, run the x64
installer the same way.

**Releases.** Pushing a tag matching `v*` (for example `v0.2.0`) runs CI, which
publishes everything to the GitHub release: per architecture the installer
(`grimalkin-setup-x64.exe`, `grimalkin-setup-arm64.exe`, versioned from the tag)
and the portable zip (`grimalkin-portable-x64.zip`,
`grimalkin-portable-arm64.zip`). These names are for signed releases; while signing is
not enabled the assets are published as `grimalkin-setup-<arch>-unsigned.exe` and
`grimalkin-portable-<arch>-unsigned.zip` (see
[Windows release signing](#windows-release-signing)). The bare `grimalkin.exe` /
`grimalkin.com` are no longer separate release assets (the names would collide between
architectures); they are inside the zips. Every run also uploads the installers and zips as
workflow artifacts (same names). x64 is built on
`windows-latest`, ARM64 natively on the `windows-11-arm` runner (target
`aarch64-pc-windows-msvc`), where the tests and the installer smoke test also
run. The installers are built from `installer/windows/grimalkin.iss`
(`/DArch=x64|arm64`) with Inno Setup 6.3 or newer; the ConPTY package version
and SHA256 are pinned in `.github/workflows/ci.yml`, which packages the
architecture-matching DLL/OpenConsole pair, fails if it is missing, and checks
the PE machine type of every shipped binary
(`scripts/windows/check-pe-machine.ps1`).

## Requirements

- Rust stable 1.92+
- Windows: just `cargo`
- macOS: just `cargo`
- Linux: `sudo apt-get install libxkbcommon-dev libwayland-dev libfontconfig1-dev`

## Build and run

```
cargo run --release -p grimalkin
cargo run --release -p grimalkin -- [OPTIONS] [-e COMMAND [ARGS...]]
```

| Flag | Meaning |
| --- | --- |
| `-e`, `--command CMD [ARGS...]` | Run CMD instead of the shell (consumes the remaining arguments) |
| `--config PATH` | Use this config file |
| `--working-directory DIR` | Start in DIR |
| `--skin NAME` | Use a window skin (llama, grimalkin, mint, or a path; `none` = normal window) |
| `--theme NAME` | Use this color theme (overrides `theme` in the config) |
| `--list-themes` | List built-in and user themes, then exit |
| `--list-skins` | List built-in skins and installed WinAMP skins, then exit |
| `--install-skin PATH_OR_URL` | Install a WinAMP `.wsz` (local file or https URL) and exit; `--force` overwrites |
| `-h`, `--help` / `-V`, `--version` | Help / version |

The release binary is `target/release/grimalkin`.

### Windows: two files

The Windows release is two files that belong in the same folder:

- `grimalkin.exe` is the GUI-subsystem terminal. Double-click it (or a
  shortcut to it) to open a window; no console window appears.
- `grimalkin.com` is a tiny console-subsystem launcher (built from
  `crates/launcher` as `grimalkin-launcher.exe` and renamed). cmd and
  PowerShell resolve a bare `grimalkin` to the `.com` before the `.exe`, so
  from a shell `grimalkin ...` runs the launcher, which starts `grimalkin.exe`
  from its own folder with the identical command line, waits for it and exits
  with its exit code. Output (`--help`, `--list-themes`, `--list-skins`,
  `--install-skin`, errors) therefore prints before the next prompt, and
  `$LASTEXITCODE` / `%ERRORLEVEL%` are set. The old workarounds
  (`grimalkin --list-skins | Out-Host`, `start /wait grimalkin ...`) are no
  longer needed with the `.com` present. Ctrl+C is left to the child.

Without `grimalkin.com`, `grimalkin.exe` still attaches to the parent console
to print, but the shell does not wait for it (the prompt redraws over the
output and exit codes are not set).

The optional `conpty.dll` and `OpenConsole.exe` (newer ConPTY) go in this same
folder, next to both files, as before.

When building yourself: `cargo build --release -p grimalkin -p grimalkin-launcher`,
then copy `target/release/grimalkin-launcher.exe` to `grimalkin.com` beside
`grimalkin.exe`.

### Windows release signing

Windows releases are meant to be Authenticode-signed: `grimalkin.exe`, `grimalkin.com`,
the installer and its uninstaller carry a signature with a trusted timestamp. The private key
lives in AWS KMS and cannot be exported; GitHub Actions signs with it through short-lived
OIDC credentials, only for `v*` tags on this repository. Microsoft's `conpty.dll` and
`OpenConsole.exe` are never re-signed: CI checks that they are byte-identical to the pinned
NuGet package and still carry Microsoft's own signature.

**Unsigned builds and the caveat.** Signing is switched on by the repository variable
`WINDOWS_SIGNING_ENABLED`. Until it is `true` (the state of this repository before the AWS
side is provisioned), and for every pull request, fork, trunk push and non-release run,
nothing is signed. Such files are always labelled: the installer and zip are named
`grimalkin-setup-<arch>-unsigned.exe` and `grimalkin-portable-<arch>-unsigned.zip`, the zip
contains `UNSIGNED.txt`, and the installer's publisher reads "grimalkin contributors
(unsigned build)". If a tag is pushed while signing is off, the release gets those
`-unsigned` assets. Signed releases have the plain names. Windows will show "Unknown
publisher" for unsigned files. Releases made before signing was set up stay unsigned.

**Publisher and SmartScreen.** Windows shows the certificate holder's name (the
validated identity on the certificate) as the publisher. A valid signature proves who
published the file and that it was not modified; it does **not** by itself give SmartScreen
reputation. A new certificate may still trigger "Windows protected your PC" until enough
users have downloaded the files, and Smart App Control, WDAC, antivirus or enterprise
policy can still object. Extended Validation no longer bypasses SmartScreen (Microsoft),
so use an OV or IV certificate and keep the same signing identity across renewals.

#### How the pipeline works

```
windows gate -> windows build (x64, arm64)  -> windows sign -> windows smoke (x64, arm64) -> release assets
                (no cloud permissions)          (one job, the only id-token: write)
```

- **gate**: signs only if `WINDOWS_SIGNING_ENABLED` is `true` **and** the event is a push of a
  `v*` tag on `rlankenau/grimalkin` **and** the tagged commit is an ancestor of `trunk`. When
  signing is enabled and the run is a release tag, a missing variable, the placeholder
  certificate chain or a tag off trunk fails the run: **there is no silent unsigned
  fallback and nothing is uploaded**.
- **build**: compiles, stages the architecture's files and the ConPTY pair, checks machine
  types and the ConPTY bytes/signature, tests the signature verifier. It runs build scripts
  and has no cloud credentials. With signing off it also builds the unsigned installer and zip.
- **sign**: downloads both architectures' files (it never runs cargo), verifies the pinned
  [jsign](https://ebourg.github.io/jsign/) jar by SHA-256, assumes the AWS role, proves that
  the committed certificate belongs to the KMS key, signs `grimalkin.exe` and `grimalkin.com`
  of both architectures and verifies each right away (`signtool verify /pa /all /tw /v` plus
  `Get-AuthenticodeSignature`: valid, embedded, exact publisher, timestamp present, leaf and
  issuer equal to `installer/windows/codesign-chain.pem`). Then it runs negative tests (unsigned,
  tampered, wrong publisher, ConPTY refusal, no double signing) on copies, builds both installers
  with Inno Setup (`SignTool=` calls `scripts/windows/sign-file.ps1`; `SignedUninstaller=yes`
  signs the uninstaller too), builds the zips, verifies what is inside them, and only then
  uploads.
- **smoke**: on a native x64 and a native ARM64 runner, re-runs the whole installer smoke test
  (PE machine types, `Arch` marker, refuse and `/REPLACEARCH=1` flows, elevated refusal, unelevated
  twin) on the installer that will be released, and for signed builds verifies Setup, the installed
  files and the installed uninstaller.
- **release assets**: attached to the GitHub release only after the smoke tests of both
  architectures pass. Actions in this path are pinned by commit SHA.

Local `cargo build` and ordinary Inno builds are unsigned and need no signing tools.
`scripts/windows/` holds the scripts and `installer/windows/codesign-chain.pem` the public
certificate chain (a placeholder until a certificate is issued).

#### Setup

Order of operations, with commands, is in [scripts/aws/README.md](scripts/aws/README.md); the
research behind the choices (CA options, costs, key protection evidence, jsign) is in
[docs/SIGNING-AWS.md](docs/SIGNING-AWS.md).

1. Deploy `infra/aws/codesign.yaml` (CloudFormation): GitHub OIDC provider, IAM role
   (assumable only by `v*` tag builds of this repository, with GitHub's immutable subject),
   RSA_4096 `SIGN_VERIFY` KMS key `alias/grimalkin-codesign` whose key policy lets only that
   role `Sign`. Turn CloudTrail on (CAs require access logging) and restrict `v*` tag
   creation with a repository ruleset.
2. `scripts/aws/make-csr.py` builds the certificate request signed by the KMS key.
3. Get an OV or IV code signing certificate from a CA that accepts KMS keys (your choice).
   Certificates last at most 460 days, so renew yearly.
4. Commit leaf + intermediates to `installer/windows/codesign-chain.pem`.
5. Set the repository **variables** (Settings, Secrets and variables, Actions, Variables;
   none is a secret):

   | Variable | Value |
   | --- | --- |
   | `WINDOWS_SIGNING_ENABLED` | `true` to sign release tags; anything else builds unsigned |
   | `AWS_SIGNING_ROLE_ARN` | the stack's `SignerRoleArn` |
   | `AWS_REGION` | region of the key |
   | `AWS_KMS_KEY_ALIAS` | key alias or id; default `alias/grimalkin-codesign` |
   | `WINDOWS_SIGNING_PUBLISHER` | exact, case-sensitive Subject of the leaf certificate |
   | `WINDOWS_SIGNING_TSA_URL` | RFC 3161 timestamp server(s), comma separated; default `http://timestamp.digicert.com` |

6. Push a `v*` tag on a trunk commit. Renewal means replacing the chain file (and the publisher
   variable if the subject changed); no workflow edits.

jsign reads the credentials that `aws-actions/configure-aws-credentials` exports
(`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`); it needs
`kms:Sign`, `kms:GetPublicKey`, `kms:DescribeKey` and `kms:ListKeys`. Timestamps are RFC 3161
with retries (3 attempts, 10 s wait, then the whole signature call retries with backoff).

#### Windows 10/11 acceptance checklist

CI runs on Windows Server images (`windows-latest`) and Windows 11 ARM64, which cannot stand in
for a real Windows 10 or 11 desktop. Before announcing the first signed release, on a clean
x64 Windows 10, an x64 Windows 11 and a Windows 11 ARM64 machine, with the files **downloaded
through a browser** (so Mark of the Web is set):

- [ ] File Properties, Digital Signatures: signer is the expected publisher, a counter-signature
      (timestamp) is present, "This digital signature is OK", on `grimalkin-setup-<arch>.exe`,
      and after install on `grimalkin.exe`, `grimalkin.com` and `unins000.exe`.
- [ ] `Get-AuthenticodeSignature` reports `Valid` for each of those files, and
      `conpty.dll` / `OpenConsole.exe` show Microsoft Corporation.
- [ ] Running the installer shows the verified publisher in the UAC / SmartScreen dialog (record
      any "Windows protected your PC" warning and the browser used; a warning is reputation, not
      a failed signature). Run it again with the ZIP and `grimalkin.exe` downloaded directly.
- [ ] Silent install, launch from the Start menu and `grimalkin.com`, add-to-PATH, context menu,
      uninstall (Settings, Apps) and upgrade over the previous version.
- [ ] Smart App Control or an enterprise WDAC policy, if available, runs the signed build.
- [ ] Record release tag, SHA-256 hashes, certificate subject and expiry, date and the results.

### Install with winget

> **Pending first submission.** The package is not in the winget repository yet;
> until it is, the command below fails with "No package found". Use the
> installer from [Releases](https://github.com/rlankenau/grimalkin/releases).

```
winget install rlankenau.grimalkin
```

winget runs the installer silently (`/VERYSILENT /NORESTART /SUPPRESSMSGBOXES`),
per user by default, and picks the x64 or the native ARM64 build for your
machine. The optional PATH and Explorer context-menu tasks are off in a silent
install; pass them with `winget install rlankenau.grimalkin --custom '/TASKS="addtopath,contextmenu"'`.
`winget upgrade rlankenau.grimalkin` upgrades in place. The manifests are
generated from the release assets, see [Packaging](#packaging).

## Installing on macOS

Download `grimalkin-<version>-macos-arm64.dmg` (Apple Silicon) or
`grimalkin-<version>-macos-x86_64.dmg` (Intel) from the latest release or the
CI artifacts, open it and drag `grimalkin.app` onto the Applications shortcut.

Release builds are signed with a Developer ID and notarized by Apple, so the
app opens normally with no Gatekeeper warning.

A build you make from source (or a CI artifact from a fork or pull request,
which has no signing secrets) is only ad-hoc signed and not notarized, so
Gatekeeper blocks the first launch. Either right-click the app in
Applications, choose Open, then Open again in the dialog (once is enough), or
clear the quarantine flag from a terminal:

```
xattr -d com.apple.quarantine /Applications/grimalkin.app
```

When started from Finder the shell opens in your home directory. To build the
app and dmg yourself run `scripts/make-app-bundle.sh` (output in `dist/`;
options `--target`, `--binary`, `--no-dmg`, `--no-notarize`; `--help` lists the
signing variables). It signs with the first "Developer ID Application"
identity in your keychain (or `GRIMALKIN_SIGN_IDENTITY`; `-` forces ad-hoc) and,
if `GRIMALKIN_NOTARY_PROFILE` names a `notarytool` keychain profile, notarizes
and staples both the app and the dmg:

```
xcrun notarytool store-credentials grimalkin --apple-id you@example.com \
    --team-id TEAMID --password <app-specific-password>
GRIMALKIN_NOTARY_PROFILE=grimalkin scripts/make-app-bundle.sh
```

### Install with Homebrew

> **Pending first submission.** The tap `rlankenau/homebrew-grimalkin` has to be
> created and filled once by the maintainer (see [Packaging](#packaging)); until
> then use the dmg above.

```
brew tap rlankenau/grimalkin
brew install --cask grimalkin
```

This installs the same signed and notarized `grimalkin.app` as the dmg into
`/Applications` and links the command-line entry point as `grimalkin` into
Homebrew's `bin`. `brew upgrade --cask grimalkin` updates it and
`brew uninstall --cask --zap grimalkin` also removes its config, cache and data.

### Install with MacPorts

> **Pending first submission, and the Portfile is unverified.** It has not been
> built with MacPorts yet and is not in macports-ports; see
> [Packaging](#packaging).

```
sudo port install grimalkin
```

This builds from source with the `cargo` port group, installs
`grimalkin.app` into `/Applications/MacPorts` and a `grimalkin` command into
`${prefix}/bin`. The app is ad-hoc signed (a MacPorts build is not Developer ID
signed or notarized, and is not quarantined).

### Release signing setup

The `macos bundle` CI job signs and notarizes when these repository secrets
exist (Settings > Secrets and variables > Actions). With any of them missing it
builds ad-hoc signed as before.

| Secret | What it is |
| --- | --- |
| `MACOS_CERT_P12_BASE64` | Developer ID Application certificate and private key as base64 `.p12`. In Keychain Access export it (File > Export Items, format .p12), then `base64 -i cert.p12 \| pbcopy`. |
| `MACOS_CERT_PASSWORD` | The password you chose when exporting the `.p12`. |
| `NOTARY_KEY_P8_BASE64` | App Store Connect API key (Users and Access > Integrations > App Store Connect API, a team key), the downloaded `AuthKey_XXXX.p8` as base64: `base64 -i AuthKey_XXXX.p8 \| pbcopy`. |
| `NOTARY_KEY_ID` | That key's Key ID. |
| `NOTARY_ISSUER_ID` | The Issuer ID shown above the key list. |

The certificate secrets alone give a signed (not notarized) build; notarization
needs all three `NOTARY_*` secrets.

## Configuration

Every key is optional. Default location:

- Windows: `%APPDATA%\grimalkin\grimalkin.toml`
- macOS/Linux: `$XDG_CONFIG_HOME/grimalkin/grimalkin.toml`, else
  `~/.config/grimalkin/grimalkin.toml`

A missing file is fine. The full commented list is in
[docs/grimalkin.example.toml](docs/grimalkin.example.toml).

```toml
scrollback_lines = 10000

[font]
family = ["JetBrains Mono", "Consolas"]
size = 13.0

[window]
cols = 120
rows = 35
opacity = 0.95
title = "grimalkin"      # shown until the shell sets one
dynamic_title = true     # false: ignore OSC 0/2 titles

theme = "catppuccin-mocha"

[colors]            # per-key overrides on top of the theme
background = "#101014"

[cursor]
shape = "bar"
blink = true

[input]
option_as_alt = true   # macOS
ctrl_v_paste = true    # plain Ctrl+V pastes via grimalkin (default on Windows)
```

## Skins & themes

Two independent layers, like the media players of the late 90s: a **theme**
recolours the terminal, a **skin** replaces the window frame with bitmap
artwork, including non-rectangular shapes (WinAMP style).

| llama | grimalkin | mint |
|:-:|:-:|:-:|
| ![llama](crates/gui/assets/skins/llama/preview.png) | ![grimalkin](crates/gui/assets/skins/grimalkin/preview.png) | ![mint](crates/gui/assets/skins/mint/preview.png) |

```toml
skin  = "llama"        # llama | grimalkin | mint | a folder of your own | "none"
theme = "dracula"
```

`grimalkin --skin grimalkin --theme nord` does the same from the command line.
A skinned window has no OS title bar: drag the title bar or the frame to move,
the edges and the corner grip to resize, double-click the title bar for the
skin's action (usually *windowshade*: the window collapses to its title bar
and restores on the next double-click or shade button). Pixels outside the
artwork's shape are transparent and click through.

A skin is a folder with `skin.toml`, `frame.png` (9-slice), `buttons.png` and
optionally `shade.png` and `frame@2x.png`; put yours in
`<config dir>/grimalkin/skins/NAME/`. The format is documented in
[docs/SKINS.md](docs/SKINS.md), and `scripts/gen-skins.py` shows how the
built-in skins are drawn. On a skinned window the grid starts at the skin's
viewport inset plus `[window] padding`; the skin's `viewport_background` is the
terminal background unless you chose a theme or `[colors] background`.

### WinAMP skins

Classic WinAMP 2.x skins (`.wsz`) work as window frames. Grimalkin uses the
artwork of the **playlist editor** window (`pledit.bmp`), the one WinAMP window
that resizes, so the terminal sits inside a real WinAMP playlist frame (title
art, close and windowshade buttons, the bottom bar), and `pledit.txt` supplies
the colors. Browse thousands of skins at the
[Skin Museum](https://skins.webamp.org), then:

```sh
grimalkin --install-skin https://skins.webamp.org/skin/<md5>/     # a museum page, a direct .wsz URL or a local file
grimalkin --list-skins
```

and put the name it prints in `grimalkin.toml`:

```toml
skin = "winamp:base-2.91"          # <config dir>/grimalkin/skins/base-2.91.wsz
# skin = "C:/skins/Cool.wsz"       # a path to a .wsz, or to an unpacked folder with pledit.bmp, also works
# winamp_viscolor_palette = true   # optionally take the 16 ANSI colors from the skin's viscolor.txt
```

While a WinAMP skin is active its `pledit.txt` colors (text, list background,
selection, current-track color) become the theme, unless you set `theme =` (or
`--theme`) yourself; `[colors]` overrides still apply on top. A `Font=` hint is
used only if that font is installed *and* monospaced. Skins are read from the
archive on every launch and never extracted to disk. Details and deviations
from the original spec: [docs/WINAMP_SKINS.md](docs/WINAMP_SKINS.md).

### Themes and live reload

`theme = "NAME"` (or `--theme NAME`) selects a color theme: colors, optional
cursor and font settings. Built in: `default`, `catppuccin-mocha`, `dracula`,
`gruvbox-dark`, `solarized-light`, `nord`, `tokyo-night`. `grimalkin --list-themes`
lists them along with your own.

Your own themes go in `<config dir>/grimalkin/themes/` (the directory that holds
`grimalkin.toml`):

- `NAME.toml`: grimalkin format, a `name`, a `[colors]` table (`foreground`,
  `background`, `cursor`, `cursor_text`, `selection_background`,
  `selection_foreground`, `url`, `black` .. `bright_white` or `color0` ..
  `color15`) and optional `[font]` / `[cursor]` tables.
- `NAME.conf`: a kitty theme file. Drop in anything from
  [kitty-themes](https://github.com/kovidgoyal/kitty-themes).

The `[colors]` table of `grimalkin.toml` overrides the theme key by key.

The config directory (config file, `themes/`, `skins/`) is watched. Saving a
file re-applies the theme, colors, cursor, font and, between two skins, the
window skin in the running window, with no restart (switching between a skin
and a normal window needs a restart). A file with a syntax error is reported
in the log and the running settings are kept. (`--theme` and `--skin` keep
winning over the file.)

## Menus

macOS has a menu bar: Settings... (Cmd+,) and Quit (Cmd+Q) in the app menu, Edit (Copy, Paste), View (zoom,
and Themes / Skins submenus that switch live by editing `grimalkin.toml` in
place, keeping its comments), Window and Help (open the config file, reveal
the themes folder). On macOS and Windows, right-click opens the same items as a
context menu (when the program is not using the mouse; Shift+right-click
always). Windows keeps the skinned window chrome-free, so it has no menu bar.
Linux has no menus yet.

## Preferences

Settings... (Cmd+, on macOS; Ctrl+, elsewhere; also in the right-click menu)
opens a Preferences window with Appearance (theme with palette previews, skin,
opacity, padding), Font (family, installed monospace fonts, size, line height),
Cursor, Terminal (scrollback, shell, title, bell), and Advanced (graphics
cursor, clipboard, install a WinAMP skin from a file or URL, open the config
file and the themes/skins folders). Every control writes `grimalkin.toml`
straight away, keeping your comments, and the running terminal applies it live
(slider drags are written ten times a second at most). "Reset ... to defaults"
removes that section's keys from the file. Escape or Cmd/Ctrl+W closes the
window. The window is drawn with egui on its own GPU device (egui-wgpu 0.35
needs wgpu 29; the terminal uses wgpu 30). Shell, graphics cursor and the
Windows ConPTY mode only apply to a new launch, and so does switching
between a plain window and a skin (the window says so when you try; switching
between two skins is live).

## Key bindings

"Mod" is Ctrl+Shift on Windows/Linux, and Cmd (or Ctrl+Shift) on macOS.

| Keys | Action |
| --- | --- |
| Mod+C | Copy selection |
| Mod+V, Shift+Insert | Paste |
| Ctrl+V (Windows; `input.ctrl_v_paste`, default on) | Paste, sanitised, instead of sending Ctrl+V to the shell |
| Mod++ / Mod+- | Zoom in / out |
| Mod+0 | Reset zoom |
| Cmd+, (macOS) / Ctrl+, | Preferences |
| Menu key, Shift+F10 (macOS and Windows) | Open the context menu at the text cursor |
| Enter / Esc (while a paste prompt is shown) | Paste / cancel (see below) |
| Shift+PageUp / Shift+PageDown | Scroll a page (main screen) |
| Mouse wheel | Scroll scrollback (or sent to the app in mouse mode) |

Pasting text with a newline (when the program has not turned on bracketed paste) or more than
1 MiB shows "Paste N lines? [Enter] paste  [Esc] cancel" over the window. Turn it off, or change the
size limit, in the `[paste]` section (`confirm_multiline`, `confirm_large_bytes`; 0 disables the size check).
The menus show these shortcuts next to Copy, Paste, Zoom and Settings. On Windows they are labels
only; the key handling is the one in the table.

## Testing kitty graphics

The `gcat` example is a small cross-platform `icat` (works on Windows):

```
cargo run -p grimalkin-graphics --example gcat -- --demo
cargo run -p grimalkin-graphics --example gcat -- picture.png
cargo run -p grimalkin-graphics --example gcat -- --place 40x12@5x3 --z -1 picture.png
cargo run -p grimalkin-graphics --example gcat -- --delete
```

Other options: `--zlib`, `--png`, `--no-move`, `--id N`, `--no-query`.

On macOS, with kitty's `kitten` installed, `scripts/icat-test.sh [case...]`
drives `kitten icat` through several scenarios (basic, place, alpha, big,
scroll, clear, zindex, altscreen) and captures frames. `MODE=dump` (default) is
offscreen; `MODE=screen` captures the real window. `scripts/kitty-send.py` is a
raw protocol sender.

Startup font loading: the first run scans all system fonts and writes an index
(`fonts.cache` under the user cache dir, e.g. `%LOCALAPPDATA%\\grimalkin` on Windows). Later runs
load only the configured families from the index and build the full set (emoji/CJK
fallback) on a background thread. Set `GRIMALKIN_EXIT_AFTER_FRAME=1` to quit after the
first frame (for timing).

Set `GRIMALKIN_DUMP_FRAME=/path/frame.png` to render offscreen and write each
frame to that PNG (useful over ssh or with a locked screen).

## TERM

Children get `TERM=xterm-kitty` so kitty-aware tools enable graphics and
keyboard protocols. On Unix, if no kitty terminfo database can be found
(`TERMINFO` is pointed at kitty's bundled one when present), Grimalkin falls
back to `TERM=xterm-256color` so vim, tmux, less and friends keep working. See
`crates/gui/src/terminfo.rs`.

## Windows: kitty graphics need a newer ConPTY

Stock Windows 10 ConPTY (conhost 10.0.19041) strips APC sequences
(`ESC _ ... ESC \`), so the kitty graphics protocol never reaches Grimalkin, and
it swallows the kitty keyboard query (`CSI ? u`). Grimalkin talks to ConPTY
directly (`crates/pty/src/win/`) and can use Microsoft's current one:

1. Download the `Microsoft.Windows.Console.ConPTY` NuGet package (a `.nupkg` is
   a zip file) and take `runtimes/win-x64/native/conpty.dll` and
   `build/native/runtimes/x64/OpenConsole.exe` (paths vary a little by
   package version; use the x64 pair).
2. Put both files in the same directory as `grimalkin.exe`.

At startup Grimalkin loads `conpty.dll` from the executable's directory if it
exists and falls back to the in-box `kernel32.dll` ConPTY otherwise. Run with
`RUST_LOG=info` to see which one was used.

For the best chance of APC and other sequences arriving untouched, also turn on
passthrough mode (`PSEUDOCONSOLE_PASSTHROUGH_MODE`), which stops ConPTY from
re-rendering the child's output:

```toml
[pty]
passthrough = true
```

or set `GRIMALKIN_CONPTY_PASSTHROUGH=1` (`0` forces it off), which overrides
the file. Passthrough only applies to the in-box ConPTY of Windows 11 24H2 or newer;
if it rejects the flag, Grimalkin logs a warning and starts without it. It is
never sent to a sideloaded `conpty.dll` (see below).
Passthrough is off by default.

Tested facts (Windows 10, ConPTY package 1.25):

- Stock Windows 10 ConPTY strips APC, so kitty graphics do not work at all.
- Sideloading `Microsoft.Windows.Console.ConPTY` 1.25 (`conpty.dll` +
  `OpenConsole.exe` next to `grimalkin.exe`) makes kitty graphics work.
- In the redistributable ConPTY (1.24+), bit 0x8 is not passthrough: it is
  `PSEUDOCONSOLE_GLYPH_WIDTH_GRAPHEMES` (the glyph-width mask is 0x18:
  GRAPHEMES 0x08, WCSWIDTH 0x10, CONSOLE 0x18; `AMBIGUOUS_IS_WIDE` is 0x20),
  and 0x2/0x4 are unused. That is why a passthrough flag appeared to do
  nothing there. Grimalkin therefore sets 0x8 only for the in-box ConPTY; the
  sideloaded one already forwards unknown sequences.
- The APC *reply* to a query (`a=q`) is stripped on the input side, so tools
  must detect support through `TERM_PROGRAM=grimalkin` / `TERM=xterm-kitty`
  rather than by querying.
- Cursor position reports (CPR) are swallowed.
- OpenConsole does not learn that a placement moved the cursor, so Grimalkin
  does not move it after placements on Windows (`[pty] graphics_cursor = "auto"`;
  set `"protocol"` to override). Tools then print their own newlines, e.g.
  `gcat --conpty`.
- `viu`'s kitty auto-detection hangs on Windows in any terminal (a bug in
  viuer's input parsing); `viu -b` (block mode) works.

## App icon

The pixel-art cat lives in `assets/icon/`: `grimalkin-<size>.png` (16 to 1024),
`grimalkin.ico` (16 to 256, PNG entries, Windows Vista+) and `grimalkin.icns`
(macOS, for the `.app` bundle). They are generated, byte-for-byte reproducibly,
by `python3 scripts/gen-icon.py` (pure Python; the `.icns` step needs macOS
`iconutil`, pass `--no-icns` to skip it).

On Windows, `crates/gui/build.rs` and `crates/launcher/build.rs` embed the `.ico`
plus version info (ProductName, FileVersion from the crate version, ...) into
`grimalkin.exe` and `grimalkin.com` through the `winresource` crate (needs
`rc.exe`, which the MSVC toolchain provides); CI checks the result. On Windows
and Linux the window icon is also set at runtime from the 64 px PNG
(`crates/gui/src/icon.rs`).

## Crate layout

- `crates/vt` (`grimalkin-vt`): terminal state machine, no I/O or GUI
- `crates/graphics` (`grimalkin-graphics`): kitty graphics protocol, image
  store, `gcat` example
- `crates/pty` (`grimalkin-pty`): PTY layer: native ConPTY on Windows, portable-pty (forkpty) on Unix
- `crates/gui` (`grimalkin`): the binary: winit + wgpu renderer, input, config

See [ARCHITECTURE.md](ARCHITECTURE.md) for details. Tests: `cargo test --workspace`.

## Packaging

For maintainers. Package-manager metadata lives in `packaging/`; the
`Packaging` workflow (`.github/workflows/packaging.yml`) regenerates it for every
release. It runs after the CI workflow finishes for a pushed `v*` tag (not on
`release: published`: the release is created by CI's own token, which cannot
trigger other workflows, and it would fire before the installers are attached),
on demand (Actions, Packaging, Run workflow, with a tag), and as a dry run on pull
requests that touch `packaging/`. Pre-release tags (`v1.0.0-rc3`) are built and
validated but never submitted automatically: winget and Homebrew have no
pre-release channel.

| Channel | Files | Automated per release | First time, manual |
| --- | --- | --- | --- |
| winget | `packaging/winget/` (templates, `update-manifest.py`) | manifests from the release installers, `winget validate`, artifact, `wingetcreate submit` if `WINGET_GITHUB_TOKEN` is set | submit the package to `microsoft/winget-pkgs` |
| Homebrew tap | `packaging/homebrew/` (`grimalkin.rb`, `update-cask.sh`) | cask version and sha256 from the release dmgs, pushed to the tap if `HOMEBREW_TAP_TOKEN` is set, else artifact | create the tap repository |
| MacPorts | `packaging/macports/` (`Portfile`, `gen-crates.py`) | checks the crate list against `Cargo.lock`, Portfile artifact with the version bumped | PR to `macports/macports-ports` |

Repository secrets (Settings, Secrets and variables, Actions), all optional:

| Secret | What it is |
| --- | --- |
| `WINGET_GITHUB_TOKEN` | A personal access token (classic, `public_repo`) of the account that owns your fork of `microsoft/winget-pkgs`. `wingetcreate` pushes a branch to that fork and opens the pull request. |
| `HOMEBREW_TAP_TOKEN` | A token with write access to `rlankenau/homebrew-grimalkin` (fine-grained PAT, Contents: read and write on that one repository). |

Without a secret the job still builds and validates the files, uploads them as an
artifact and prints the manual command.

**winget, first submission (manual).** The workflow only adds versions to a
package that already exists in `microsoft/winget-pkgs`, so the first version is
submitted by hand from a Windows machine, with a stable (non pre-release) tag:

```
wingetcreate new https://github.com/rlankenau/grimalkin/releases/download/vX.Y.Z/grimalkin-setup-x64.exe `
                 https://github.com/rlankenau/grimalkin/releases/download/vX.Y.Z/grimalkin-setup-arm64.exe
```

It prompts for the missing fields and offers to open the pull request. You can
instead generate the manifests with
`python3 packaging/winget/update-manifest.py vX.Y.Z`, check them with
`winget validate --manifest packaging/winget/manifests/X.Y.Z`, and copy them into
a fork of `microsoft/winget-pkgs` under `manifests/r/rlankenau/grimalkin/X.Y.Z/`.
`winget install --manifest <dir>` tests them locally (once, in an administrator
shell: `winget settings --enable LocalManifestFiles`). Unsigned installers are
accepted, but they get more scrutiny in review (SmartScreen and antivirus checks
on the installer URL) and users get SmartScreen warnings, so get the
[Windows release signing](#windows-release-signing) pipeline working first and
submit a signed release. The package identifier is `rlankenau.grimalkin`; the
`ProductCode` in the installer manifest is the Inno Setup `AppId` plus `_is1`
and must not change. After the package is merged, set `WINGET_GITHUB_TOKEN` and
later releases are submitted by the workflow (`wingetcreate submit` of the generated manifests). `update-manifest.py` leaves an
architecture out of the manifest when the release has no installer for it
(`v1.0.0-rc3` predates the ARM64 installer).

**Homebrew tap.** Create the public repository `rlankenau/homebrew-grimalkin`
(the `homebrew-` prefix is what makes `brew tap rlankenau/grimalkin` work), put
`packaging/homebrew/grimalkin.rb` in it as `Casks/grimalkin.rb`, and add
`HOMEBREW_TAP_TOKEN`. Releases then update the cask by themselves; by hand, run
`packaging/homebrew/update-cask.sh vX.Y.Z path/to/Casks/grimalkin.rb`. The
cask is checked with `brew style --cask` (clean) from a temporary tap; run
`brew audit --cask --new rlankenau/grimalkin/grimalkin` from the real tap too.
The main `homebrew-cask` repository is not a realistic target yet: it only
accepts notable software (about 75 GitHub stars, or 30 forks or 30 watchers, for
self-submitted apps; check <https://docs.brew.sh/Acceptable-Casks>) and
requires notarized apps. Revisit when the project is better known.

**MacPorts.** `packaging/macports/Portfile` is a draft in the `aqua shells`
categories (as `alacritty` and `kitty` have) using the `cargo` 1.0 port group. It
builds only the `grimalkin` crate, wraps the binary with
`scripts/make-app-bundle.sh --binary ... --no-dmg --no-notarize` (ad-hoc
signing) and installs `grimalkin.app`, a `${prefix}/bin/grimalkin` symlink and
the licenses. Its `test.run` executes `grimalkin --list-themes`. **It has not been
run through `port lint` or `port install` yet** (the author's machine has no
MacPorts); treat it as unverified until someone has done
`sudo port lint --nitpick` and `sudo port -vst install` on a Mac with a
checkout of macports-ports. For a release: set the version in `github.setup`,
fix the `checksums` (`sudo port checksum grimalkin` prints the expected values),
and regenerate the crate list when `Cargo.lock` changes (CI checks it):

```
packaging/macports/gen-crates.py --update packaging/macports/Portfile
```

Submit it as a pull request to <https://github.com/macports/macports-ports>
adding `aqua/grimalkin/Portfile`; the reviewers will want a stable (non rc)
version.

## License

MIT OR Apache-2.0: see [LICENSE](LICENSE), [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE). The bundled ConPTY redistributable is MIT
licensed by Microsoft: see
[installer/windows/LICENSE-ConPTY.txt](installer/windows/LICENSE-ConPTY.txt).
