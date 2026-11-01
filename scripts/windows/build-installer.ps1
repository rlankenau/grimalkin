<#
.SYNOPSIS
  Builds the Inno Setup installer for one architecture from <repo>\dist\.

.DESCRIPTION
  Unsigned (default): builds installer-out\grimalkin-setup-<arch>-unsigned.exe and
  labels the installer's publisher "(unsigned build)".

  -Sign: builds installer-out\grimalkin-setup-<arch>.exe and signs it with
  scripts\windows\sign-file.ps1 through Inno's SignTool (SignedUninstaller=yes, so
  the generated uninstaller is signed before it is embedded). Needs the
  environment documented in sign-file.ps1. -Publisher is the certificate's common
  name, shown as the publisher in Apps & features.

  SignPath route (docs/SIGNING-SIGNPATH.md). SignPath cannot be called from inside
  ISCC, so Inno's SignTool is not used and the installer is built in steps:
    -SignPathGenerateUninstaller  compiles with SignedUninstaller=yes and no
        SignTool. ISCC then writes the unsigned uninstaller to
        -SignPathUninstallerDir\uninst-<inno version>-<hash>.e32 (or .e64; it is a
        PE file with an Inno-specific extension) and stops with
        "please attach your digital signature ... and compile again": that
        failure is expected. The script returns the uninstaller file, which the
        caller submits to SignPath. The directory must be empty.
    -SignPathSetup  after the uninstaller in -SignPathUninstallerDir has been
        replaced by its signed version (same name), compiles again: ISCC embeds
        the signature (and fails if the file is not signed). The result,
        <OutDir>\grimalkin-setup-<arch>.exe, is UNSIGNED so far and is submitted
        to SignPath by the caller. Use the same -Publisher in both steps.
  Both take -Publisher (the certificate's common name) like -Sign does.

  -CITest builds the CI-only twin (/DCITestUnelevated, never released) into
  installer-test\ and never signs it.

  -Dist points at the staged files (default: dist); ISCC reads ..\..\dist, so
  -Dist is copied there when it is somewhere else.
#>
param(
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$Version,
    [switch]$Sign,
    [switch]$CITest,
    [switch]$SignPathGenerateUninstaller,
    [switch]$SignPathSetup,
    [string]$SignPathUninstallerDir,
    [string]$OutDir,
    [string]$Publisher,
    [string]$Dist = 'dist'
)
$ErrorActionPreference = 'Stop'
if ($Sign -and $CITest) { throw 'the CI test twin is never signed' }
if ($Sign -and -not $Publisher) { throw '-Sign needs -Publisher' }
$signPath = $SignPathGenerateUninstaller -or $SignPathSetup
if ($SignPathGenerateUninstaller -and $SignPathSetup) { throw '-SignPathGenerateUninstaller and -SignPathSetup are separate steps' }
if ($signPath) {
    if ($Sign -or $CITest) { throw 'the SignPath steps cannot be combined with -Sign or -CITest' }
    if (-not $Publisher -or -not $SignPathUninstallerDir) { throw 'the SignPath steps need -Publisher and -SignPathUninstallerDir' }
    if ($SignPathSetup -and -not $OutDir) { throw '-SignPathSetup needs -OutDir' }
    New-Item -ItemType Directory -Force -Path $SignPathUninstallerDir | Out-Null
    $SignPathUninstallerDir = (Resolve-Path -LiteralPath $SignPathUninstallerDir).Path
    if ($SignPathGenerateUninstaller -and (Get-ChildItem -LiteralPath $SignPathUninstallerDir -Force)) {
        throw "-SignPathUninstallerDir $SignPathUninstallerDir must be empty"
    }
}

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$want = Join-Path $repo 'dist'
$have = (Resolve-Path -LiteralPath $Dist).Path
if ($have -ne $want) {
    # ISCC reads ..\..\dist; stage the architecture's files there.
    if (Test-Path $want) { Remove-Item -LiteralPath $want -Recurse -Force }
    Copy-Item -LiteralPath $have -Destination $want -Recurse
}

# Inno Setup 6 is preinstalled on the windows runner images (6.7.x on
# windows-latest and windows-11-arm; ARM64 installers need 6.3+).
$iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
if (-not (Test-Path $iscc)) {
    Write-Host 'Inno Setup 6 not preinstalled; installing with choco'
    choco install innosetup --no-progress -y
    if ($LASTEXITCODE -ne 0) { throw 'choco install innosetup failed' }
}
if (-not (Test-Path $iscc)) { throw "ISCC.exe not found at $iscc" }

if ($Version -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.+-]+)?$') { throw "unexpected version format: $Version" }
# Inno's VersionInfoVersion must be numeric x.y.z.w: drop any -suffix/+build.
$core = ($Version -split '[-+]')[0]
if ($core -notmatch '^\d+\.\d+\.\d+$') { throw "unexpected version format: $Version" }
$numeric = "$core.0"

$defs = @("/DAppVersion=$Version", "/DAppVersionNumeric=$numeric", "/DArch=$Arch")
if ($CITest) {
    # Behaves as an unelevated setup even on an administrator runner, to exercise
    # the per-user replace flow. Not uploaded to a release.
    $out = 'installer-test'
    $base = "grimalkin-setup-citest-$Arch"
    $defs += '/DCITestUnelevated'
} elseif ($signPath) {
    # The unsigned-uninstaller step produces no installer (ISCC stops); the second
    # step writes the installer, not yet signed, into -OutDir.
    $out = if ($SignPathSetup) { New-Item -ItemType Directory -Force -Path $OutDir | ForEach-Object FullName } else { Join-Path ([IO.Path]::GetTempPath()) "grimalkin-iscc-$([guid]::NewGuid())" }
    $base = "grimalkin-setup-$Arch"
    $defs += "/DAppPublisher=$Publisher", "/DPreSignedUninstaller=$SignPathUninstallerDir"
} elseif ($Sign) {
    $out = 'installer-out'
    $base = "grimalkin-setup-$Arch"
    $defs += "/DAppPublisher=$Publisher", '/DSignInstaller'
    # Inno substitutes $q with a quote and $f with the quoted file name. The
    # command must stay free of other quotes so ISCC's own argument quoting holds.
    $script = Join-Path $PSScriptRoot 'sign-file.ps1'
    $defs += '/Sgrimalkin=pwsh.exe -NoLogo -NoProfile -NonInteractive -File $q' + $script + '$q $f'
} else {
    $out = 'installer-out'
    $base = "grimalkin-setup-$Arch-unsigned"
    $defs += '/DAppPublisher=grimalkin contributors (unsigned build)'
}
Write-Host "ISCC: Arch=$Arch AppVersion=$Version -> $out\$base.exe (sign=$([bool]$Sign), signpath=$signPath)"
Push-Location $repo
try {
    if ($SignPathGenerateUninstaller) {
        # ISCC is EXPECTED to fail here, after writing the unsigned uninstaller.
        $log = & $iscc @defs "/O$out" "/F$base" installer/windows/grimalkin.iss 2>&1 | Out-String
        $code = $LASTEXITCODE
        Write-Host $log
        if ($code -eq 0) { throw 'ISCC succeeded without a signed uninstaller; the SignPath flow is not what was expected' }
        if ($log -notmatch 'Signed uninstaller mode is enabled') { throw "ISCC failed for another reason (exit code $code)" }
        $found = @(Get-ChildItem -LiteralPath $SignPathUninstallerDir -Filter 'uninst-*' -File | Where-Object Extension -In '.e32', '.e64')
        if ($found.Count -ne 1) { throw "expected exactly one uninstaller in $SignPathUninstallerDir, found $($found.Count)" }
        Remove-Item -LiteralPath $out -Recurse -Force -ErrorAction SilentlyContinue
        $global:LASTEXITCODE = 0
        return $found[0]
    }
    & $iscc @defs "/O$out" "/F$base" installer/windows/grimalkin.iss
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
} finally { Pop-Location }
# $out is relative to the repo, or absolute with -SignPathSetup (Combine keeps a rooted second path).
Get-Item ([IO.Path]::Combine($repo, "$out\$base.exe"))
