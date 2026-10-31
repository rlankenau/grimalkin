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
    [string]$Publisher,
    [string]$Dist = 'dist'
)
$ErrorActionPreference = 'Stop'
if ($Sign -and $CITest) { throw 'the CI test twin is never signed' }
if ($Sign -and -not $Publisher) { throw '-Sign needs -Publisher' }

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
Write-Host "ISCC: Arch=$Arch AppVersion=$Version -> $out\$base.exe (sign=$([bool]$Sign))"
Push-Location $repo
try {
    & $iscc @defs "/O$out" "/F$base" installer/windows/grimalkin.iss
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
} finally { Pop-Location }
Get-Item (Join-Path $repo "$out\$base.exe")
