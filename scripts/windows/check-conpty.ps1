<#
.SYNOPSIS
  Asserts that the ConPTY pair in a staged directory is exactly what Microsoft
  published, and that it still carries Microsoft's own Authenticode signature.

.DESCRIPTION
  1. Re-fetches the pinned NuGet package (SHA-256 verified) and compares
     the SHA-256 of conpty.dll and OpenConsole.exe with the staged files, so
     nothing the build job produced is trusted.
  2. Requires Get-AuthenticodeSignature to report Status Valid, an embedded
     Authenticode signature, a signer whose simple name is "Microsoft Corporation"
     and an issuer "Microsoft (ID Verified) Code Signing PCA ...".
  These files are NEVER signed with the grimalkin key: grimalkin.exe loads them
  by Microsoft's signature, and re-signing would change the upstream bytes.
#>
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Sha256
)
$ErrorActionPreference = 'Stop'

$ref = Join-Path ([IO.Path]::GetTempPath()) "conpty-ref-$Arch-$([guid]::NewGuid())"
try {
    & "$PSScriptRoot/fetch-conpty.ps1" -Version $Version -Sha256 $Sha256 -Arch $Arch -Dest $ref
    foreach ($name in 'conpty.dll', 'OpenConsole.exe') {
        $staged = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $staged)) { throw "$name missing in $Dir" }
        $want = (Get-FileHash -Algorithm SHA256 (Join-Path $ref $name)).Hash
        $have = (Get-FileHash -Algorithm SHA256 $staged).Hash
        if ($want -ne $have) { throw "$Arch ${name}: staged SHA-256 $have differs from the NuGet package ($want); it was modified" }

        $sig = Get-AuthenticodeSignature -LiteralPath $staged
        if ($sig.Status -ne 'Valid' -or $sig.SignatureType -ne 'Authenticode' -or $null -eq $sig.SignerCertificate) {
            throw "$Arch ${name}: expected Microsoft's valid embedded signature, got $($sig.Status)/$($sig.SignatureType)"
        }
        $simple = $sig.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
        if ($simple -cne 'Microsoft Corporation') { throw "$Arch ${name}: signed by '$simple', not Microsoft Corporation" }
        if ($sig.SignerCertificate.Issuer -notmatch 'CN=Microsoft (ID Verified )?Code Signing PCA') {
            throw "$Arch ${name}: unexpected issuer '$($sig.SignerCertificate.Issuer)'"
        }
        Write-Host "$Arch ${name}: matches NuGet SHA-256 $have; valid Microsoft signature ($($sig.SignerCertificate.Issuer))"
    }
} finally {
    Remove-Item -LiteralPath $ref -Recurse -Force -ErrorAction SilentlyContinue
}
