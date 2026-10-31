<#
.SYNOPSIS
  Fails unless every file carries a valid embedded grimalkin Authenticode
  signature with a trusted timestamp.

.DESCRIPTION
  For each file:
    1. signtool verify /pa /all /tw /v must exit 0. Exit 2 (warnings, for
       example a missing timestamp) is a failure too. /a is NOT used: shipped
       files must carry embedded signatures, not catalog signatures.
    2. Get-AuthenticodeSignature must report Status Valid, SignatureType
       Authenticode (embedded), a signer certificate whose Subject is exactly
       -ExpectedSubject (case-sensitive), and a timestamp countersignature.
    3. With -ChainFile: the signer certificate must be byte-identical to the
       leaf certificate in that PEM file, and its Issuer must equal the Subject
       of the next certificate in the file. The chain file is committed to the
       repository and reviewed, so this pins the issuing CA, not just a name.

  -SignTool may be a path or the name of a function (the self-test mocks it).
  If signtool.exe cannot be found the script fails, unless -AllowMissingSignTool
  is given (used on runners where the SDK location is unproven; the
  Get-AuthenticodeSignature check still runs and uses the same WinVerifyTrust
  engine).
#>
param(
    [Parameter(Mandatory)][string[]]$Path,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ExpectedSubject,
    [string]$ChainFile,
    [string]$SignTool,
    [switch]$AllowMissingSignTool
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/signing-common.ps1"

if (-not $SignTool) { $SignTool = Find-SignTool }
if (-not $SignTool) {
    if (-not $AllowMissingSignTool) { throw 'signtool.exe not found (install the Windows SDK, or set WINDOWS_SIGNTOOL)' }
    Write-Host '::warning::signtool.exe not found; relying on Get-AuthenticodeSignature only'
}

$chain = $null
if ($ChainFile) { $chain = Read-PemCertificates $ChainFile }

foreach ($file in $Path) {
    $file = (Resolve-Path -LiteralPath $file -ErrorAction Stop).Path

    if ($SignTool) {
        & $SignTool verify /pa /all /tw /v $file | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "Authenticode verification failed: $file (signtool exit $LASTEXITCODE)" }
    }

    $sig = Get-AuthenticodeSignature -LiteralPath $file
    if ($sig.Status -ne 'Valid') { throw "Signature is not valid: $file ($($sig.Status): $($sig.StatusMessage))" }
    if ($sig.SignatureType -ne 'Authenticode') { throw "Not an embedded Authenticode signature: $file ($($sig.SignatureType))" }
    if ($null -eq $sig.SignerCertificate) { throw "No signer certificate: $file" }
    if ($sig.SignerCertificate.Subject -cne $ExpectedSubject) {
        throw "Unexpected publisher for ${file}: '$($sig.SignerCertificate.Subject)', expected '$ExpectedSubject'"
    }
    if ($null -eq $sig.TimeStamperCertificate) { throw "Missing trusted timestamp: $file" }

    if ($chain) {
        if ((Get-CertSha256 $sig.SignerCertificate) -ne (Get-CertSha256 $chain[0])) {
            throw "Signer certificate of $file is not the leaf of $ChainFile"
        }
        if ($chain.Count -gt 1 -and $sig.SignerCertificate.Issuer -cne $chain[1].Subject) {
            throw "Issuer of the signer certificate of $file is not the CA in $ChainFile"
        }
    }
    Write-Host "Verified $file"
    Write-Host "  publisher : $($sig.SignerCertificate.Subject)"
    Write-Host "  timestamp : $($sig.TimeStamperCertificate.Subject)"
}
