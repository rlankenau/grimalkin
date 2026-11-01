<#
.SYNOPSIS
  Tests the installer flow used by the SignPath signing route, end to end, with a
  throwaway self-signed certificate standing in for SignPath.

.DESCRIPTION
  The SignPath route cannot call SignPath from inside ISCC, so it builds in steps
  (build-installer.ps1 -SignPathGenerateUninstaller / -SignPathSetup, see
  docs/SIGNING-SIGNPATH.md). The release relies on three properties of Inno Setup
  that this script checks on every CI run, long before a real release:

    1. the unsigned uninstaller written by the first compile does not depend on
       the payload (a different grimalkin.exe gives the same file name and the
       same bytes), so it can be signed together with the program files, before
       the final compile;
    2. a final compile with an UNSIGNED uninstaller fails (fail closed);
    3. a final compile with a signed uninstaller (here: self-signed) succeeds,
       and the unins000.exe that Setup installs carries that signature.

  The self-signed certificate is created in the current user's store and removed
  again. Nothing here is released. Runs on a native runner of -Arch (it installs
  and uninstalls Setup per user).
#>
param(
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$Version,
    [string]$Dist = 'dist'
)
$ErrorActionPreference = 'Stop'
$build = Join-Path $PSScriptRoot 'build-installer.ps1'
$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $base "signpath-flow-$([guid]::NewGuid())"
New-Item -ItemType Directory $work | Out-Null
$publisher = 'Grimalkin Flow Test'
$cert = $null
try {
    # Two payloads that differ in grimalkin.exe. build-installer.ps1 copies -Dist to
    # <repo>\dist, so work from copies and leave the unmodified one for last.
    $dist1 = Join-Path $work 'dist1'
    $dist2 = Join-Path $work 'dist2'
    Copy-Item -LiteralPath $Dist -Destination $dist1 -Recurse
    Copy-Item -LiteralPath $Dist -Destination $dist2 -Recurse
    $exe2 = Join-Path $dist2 'grimalkin.exe'
    $bytes = [IO.File]::ReadAllBytes($exe2)
    [IO.File]::WriteAllBytes($exe2, [byte[]]($bytes + [byte[]](0, 1, 2, 3)))

    $u2 = Join-Path $work 'uninstaller2'
    $f2 = & $build -Arch $Arch -Version $Version -Dist $dist2 -SignPathGenerateUninstaller -SignPathUninstallerDir $u2 -Publisher $publisher
    $u1 = Join-Path $work 'uninstaller1'
    $f1 = & $build -Arch $Arch -Version $Version -Dist $dist1 -SignPathGenerateUninstaller -SignPathUninstallerDir $u1 -Publisher $publisher
    Write-Host "uninstaller 1: $($f1.Name)"
    Write-Host "uninstaller 2: $($f2.Name)"
    if ($f1.Name -cne $f2.Name) { throw 'the unsigned uninstaller depends on the payload: it cannot be signed before the final compile' }
    if ((Get-FileHash $f1.FullName).Hash -ne (Get-FileHash $f2.FullName).Hash) { throw 'the unsigned uninstaller bytes depend on the payload' }
    Write-Host 'ok: the unsigned uninstaller does not depend on the payload'
    & (Join-Path $PSScriptRoot 'check-product-info.ps1') -Path $f1.FullName -Version $Version

    $out = Join-Path $work 'out'
    $failed = $false
    try { & $build -Arch $Arch -Version $Version -Dist $dist1 -SignPathSetup -SignPathUninstallerDir $u2 -OutDir $out -Publisher $publisher | Out-Null }
    catch { $failed = $true; Write-Host "ok: an unsigned uninstaller is refused ($_)" }
    if ($failed -eq $false) { throw 'the installer was built with an unsigned uninstaller' }
    $global:LASTEXITCODE = 0

    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=$publisher" -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddDays(1)
    # Inno names the file uninst-<version>-<hash>.e32 (or .e64): not an extension a
    # signing tool knows, so sign a copy named .exe and put it back under the Inno name.
    $asExe = Join-Path $work "$($f1.BaseName).exe"
    Copy-Item $f1.FullName $asExe
    $r = Set-AuthenticodeSignature -FilePath $asExe -Certificate $cert -HashAlgorithm SHA256
    if ($null -eq $r.SignerCertificate) { throw "could not sign the test uninstaller: $($r.StatusMessage)" }
    Copy-Item $asExe $f1.FullName -Force

    $setup = & $build -Arch $Arch -Version $Version -Dist $dist1 -SignPathSetup -SignPathUninstallerDir $u1 -OutDir $out -Publisher $publisher
    if ($setup.Length -lt 1MB) { throw "suspiciously small installer: $($setup.Length) bytes" }
    Write-Host "ok: the final compile embedded the signed uninstaller ($($setup.FullName))"
    & (Join-Path $PSScriptRoot 'check-product-info.ps1') -Path $setup.FullName -Version $Version

    # The installed uninstaller must carry the signature.
    $dir = Join-Path $work 'installed'
    $p = Start-Process $setup.FullName -Wait -PassThru -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', "/DIR=$dir", "/LOG=$work\install.log"
    if ($p.ExitCode -ne 0) { Get-Content "$work\install.log"; throw "test installer exit code $($p.ExitCode)" }
    try {
        $sig = Get-AuthenticodeSignature -LiteralPath (Join-Path $dir 'unins000.exe')
        if ($null -eq $sig.SignerCertificate -or $sig.SignerCertificate.Thumbprint -ne $cert.Thumbprint) {
            throw 'the installed unins000.exe does not carry the signature of the pre-signed uninstaller'
        }
        Write-Host 'ok: the installed unins000.exe carries the signature'
    } finally {
        $u = Join-Path $dir 'unins000.exe'
        if (Test-Path $u) { Start-Process $u -Wait -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' }
    }
    Write-Host 'SignPath installer flow tests passed'
} finally {
    if ($cert) { Remove-Item "Cert:\CurrentUser\My\$($cert.Thumbprint)" -ErrorAction SilentlyContinue }
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    $global:LASTEXITCODE = 0
}
