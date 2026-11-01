<#
.SYNOPSIS
  Installer smoke test: silent install, check, other-architecture refusal and
  replacement flows, uninstall. Runs on a native runner of the installer's
  architecture (x64 on windows-latest, arm64 on windows-11-arm).

.DESCRIPTION
  This is the smoke test that used to be inline in ci.yml (PE machine check, Arch
  registry marker, refuse / replace of the other architecture, elevated refusal,
  the CITestUnelevated twin). New: with -Signed it also verifies the signature
  of Setup, of the installed first-party files and of the installed uninstaller.

  -SetupPath      the shipped installer for this architecture
  -CITestSetupPath the CI-only unelevated twin (never released)
  -Signed         the installer is a signed release build; -ExpectedSubject and
                  -ChainFile (AWS KMS route: installer/windows/codesign-chain.pem)
                  or -ExpectedIssuer (SignPath route: the issuing CA, because
                  SignPath owns the leaf certificate) are then required
#>
param(
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$SetupPath,
    [Parameter(Mandatory)][string]$CITestSetupPath,
    [switch]$Signed,
    [string]$ExpectedSubject,
    [string]$ChainFile,
    [string]$ExpectedIssuer
)
$ErrorActionPreference = 'Stop'
if ($Signed -and (-not $ExpectedSubject -or (-not $ChainFile -and -not $ExpectedIssuer))) {
  throw '-Signed needs -ExpectedSubject and -ChainFile or -ExpectedIssuer'
}
# How the signer is pinned: the committed chain (KMS) and/or the issuing CA (SignPath).
$pin = @{}
if ($ChainFile) { $pin.ChainFile = $ChainFile }
if ($ExpectedIssuer) { $pin.ExpectedIssuer = $ExpectedIssuer }
if ($Signed) {
  & "$PSScriptRoot/verify-signature.ps1" -Path $SetupPath -ExpectedSubject $ExpectedSubject @pin -AllowMissingSignTool
}
$setup = (Resolve-Path -LiteralPath $SetupPath).Path
# Per-user silent install with the optional tasks turned on.
$p = Start-Process $setup -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/TASKS=addtopath,contextmenu','/LOG=install.log'
if ($p.ExitCode -ne 0) { Get-Content install.log; throw "installer exit code $($p.ExitCode)" }
$dir = Join-Path $env:LOCALAPPDATA 'Programs\grimalkin'
foreach ($f in 'grimalkin.exe','grimalkin.com','conpty.dll','OpenConsole.exe','LICENSE-ConPTY.txt','README.md','unins000.exe') {
  if (-not (Test-Path (Join-Path $dir $f))) { throw "missing after install: $f" }
}
if ($Signed) {
  # Everything the signed Setup installed must carry the right signatures: our
  # signature on the first-party files and on the uninstaller Inno generated
  # (SignedUninstaller=yes), Microsoft's own on the ConPTY pair.
  & "$PSScriptRoot/verify-signature.ps1" -Path (Join-Path $dir 'grimalkin.exe'),(Join-Path $dir 'grimalkin.com'),(Join-Path $dir 'unins000.exe') `
    -ExpectedSubject $ExpectedSubject @pin -AllowMissingSignTool
  foreach ($f in 'conpty.dll','OpenConsole.exe') {
    $s = Get-AuthenticodeSignature -LiteralPath (Join-Path $dir $f)
    if ($s.Status -ne 'Valid' -or $s.SignerCertificate.GetNameInfo('SimpleName', $false) -ne 'Microsoft Corporation') {
      throw "installed $f is not validly signed by Microsoft Corporation ($($s.Status))"
    }
  }
} else {
  Write-Host 'UNSIGNED build: signature checks skipped'
}
# The installed binaries are the right architecture too.
& "$PSScriptRoot/check-pe-machine.ps1" -Dir $dir -Arch $arch
$recorded = (Get-ItemProperty 'HKCU:\Software\grimalkin\Install').Arch
if ($recorded -ne $arch) { throw "installed Arch value is '$recorded', expected '$arch'" }
$userPath = [Environment]::GetEnvironmentVariable('Path','User')
if (-not (($userPath -split ';') -contains $dir)) { throw "install dir not on user PATH: $userPath" }
$cmd = (Get-ItemProperty 'HKCU:\Software\Classes\Directory\Background\shell\grimalkin\command').'(default)'
Write-Host "context menu command: $cmd"
if ($cmd -notlike '*--working-directory*') { throw 'context menu command missing' }
& (Join-Path $dir 'grimalkin.com') --version
if ($LASTEXITCODE -ne 0) { throw "grimalkin.com --version failed: $LASTEXITCODE" }

# Coexistence: pretend the install on disk is the OTHER architecture.
# Re-running this installer must then refuse (silent mode, no
# /REPLACEARCH) and leave the install untouched.
$other = if ($arch -eq 'x64') { 'arm64' } else { 'x64' }
Set-ItemProperty 'HKCU:\Software\grimalkin\Install' -Name Arch -Value $other
$p = Start-Process $setup -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/TASKS=addtopath,contextmenu','/LOG=refuse.log'
Write-Host "installer exit code with $other install present and no /REPLACEARCH: $($p.ExitCode)"
if ($p.ExitCode -eq 0) { throw "installer must refuse to install over a $other install without /REPLACEARCH=1" }
if (-not (Test-Path (Join-Path $dir 'grimalkin.exe'))) { throw 'refused install must leave the existing install untouched' }
# Security rule (review finding): an ELEVATED setup must never run a
# per-user (HKCU) uninstaller, even with /REPLACEARCH=1. This runner is
# an administrator, so the shipped installer is elevated here.
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Write-Host "runner is elevated: $isAdmin"
if ($isAdmin) {
  $p = Start-Process $setup -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/REPLACEARCH=1','/TASKS=addtopath,contextmenu','/LOG=elevated-refuse.log'
  Write-Host "elevated installer exit code with a per-user $other install and /REPLACEARCH=1: $($p.ExitCode)"
  if ($p.ExitCode -eq 0) { throw 'an elevated installer must not replace a per-user install of the other architecture' }
  if ((Get-Content elevated-refuse.log -Raw) -notmatch 'Refusing to run a per-user uninstaller') { Get-Content elevated-refuse.log; throw 'elevated refusal not logged' }
  if ((Get-Content elevated-refuse.log -Raw) -match 'Removing other-architecture install') { throw 'elevated installer ran the per-user uninstaller' }
  $recorded = (Get-ItemProperty 'HKCU:\Software\grimalkin\Install').Arch
  if ($recorded -ne $other) { throw "elevated refusal changed the Arch value to '$recorded'" }
  if (-not (Test-Path (Join-Path $dir 'grimalkin.exe'))) { throw 'elevated refusal must leave the existing install untouched' }
}
# Unelevated per-user flow (CI-only twin build): with /REPLACEARCH=1 it
# uninstalls the old install first and ends with one consistent install.
$setup = (Resolve-Path -LiteralPath $CITestSetupPath).Path
$p = Start-Process $setup -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/REPLACEARCH=1','/TASKS=addtopath,contextmenu','/LOG=replace.log'
if ($p.ExitCode -ne 0) { Get-Content replace.log; throw "installer with /REPLACEARCH=1 exit code $($p.ExitCode)" }
$recorded = (Get-ItemProperty 'HKCU:\Software\grimalkin\Install').Arch
if ($recorded -ne $arch) { throw "after replacing, Arch is '$recorded', expected '$arch'" }
if ((Get-Content replace.log -Raw) -notmatch 'other-architecture install') { Get-Content replace.log; throw 'replace log does not show the old install being removed' }
& "$PSScriptRoot/check-pe-machine.ps1" -Dir $dir -Arch $arch
$userPath = [Environment]::GetEnvironmentVariable('Path','User')
if ((($userPath -split ';') | Where-Object { $_ -eq $dir }).Count -ne 1) { throw "install dir must be on user PATH exactly once: $userPath" }

# Uninstall via the registered uninstaller (after a replace its file
# name need not be unins000.exe). _?= runs it in place so -Wait really waits.
Get-ChildItem $dir -Filter 'unins*' | Format-Table Name, Length
$uninst = ((Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{5E56C746-3735-4FF7-96B9-E90F7D4C5AA7}_is1').UninstallString).Trim('"')
Write-Host "registered uninstaller: $uninst"
if (-not (Test-Path $uninst)) { throw "registered uninstaller missing: $uninst" }
$u = Start-Process $uninst -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"_?=$dir"
if ($u.ExitCode -ne 0) { throw "uninstaller exit code $($u.ExitCode)" }
$userPath = [Environment]::GetEnvironmentVariable('Path','User')
if (($userPath -split ';') -contains $dir) { throw 'install dir still on user PATH after uninstall' }
if (Test-Path 'HKCU:\Software\Classes\Directory\shell\grimalkin') { throw 'context menu key left behind' }
if (Test-Path 'HKCU:\Software\Classes\Directory\Background\shell\grimalkin') { throw 'background context menu key left behind' }
if (Test-Path 'HKCU:\Software\grimalkin\Install') { throw 'Arch marker key left behind' }
