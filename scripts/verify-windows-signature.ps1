# Verify the distributed filename with WinTrust, then enforce publisher identity.
param(
    [Parameter(Mandatory)][string[]]$Path,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ExpectedSubject,
    [string]$SignTool = $env:WINDOWS_SIGNTOOL
)

$ErrorActionPreference = 'Stop'
if (-not $SignTool) {
    $SignTool = (Get-Command signtool.exe -ErrorAction Stop).Source
}

foreach ($file in $Path) {
    $file = (Resolve-Path -LiteralPath $file -ErrorAction Stop).Path
    # /pa uses Authenticode policy, /all checks every signature, /tw warns on
    # missing timestamps. Warnings (exit 2) are failures too. Do not use /a:
    # first-party files must have embedded signatures, not catalog signatures.
    & $SignTool verify /pa /all /tw /v $file | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode verification failed: $file (exit $LASTEXITCODE)"
    }

    $signature = Get-AuthenticodeSignature -LiteralPath $file
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode' -or $null -eq $signature.SignerCertificate) {
        throw "Invalid embedded signature: $file ($($signature.Status))"
    }
    if ($signature.SignerCertificate.Subject -cne $ExpectedSubject) {
        throw "Unexpected publisher for ${file}: $($signature.SignerCertificate.Subject)"
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Missing trusted timestamp: $file"
    }
    Write-Host "Verified $file; publisher=$ExpectedSubject; timestamp issuer=$($signature.TimeStamperCertificate.Subject)"
}
