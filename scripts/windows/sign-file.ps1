<#
.SYNOPSIS
  Authenticode-signs ONE file with the grimalkin AWS KMS key through jsign.

.DESCRIPTION
  Used directly by CI for grimalkin.exe / grimalkin.com and as Inno Setup's
  SignTool command (so Setup and the generated uninstaller are signed too):
  Inno runs `pwsh -File sign-file.ps1 <file>` once per file and treats a
  non-zero exit code as failure.

  Configuration comes from the environment (set by the "windows sign" job):
    JSIGN_JAR                     path of the hash-verified jsign jar
    AWS_REGION                    region of the KMS key
    AWS_KMS_KEY_ALIAS             key id or alias (alias/grimalkin-codesign)
    WINDOWS_SIGNING_TSA_URL       RFC 3161 TSA URL(s), comma separated
    AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN
                                  short-lived credentials from
                                  aws-actions/configure-aws-credentials; jsign
                                  reads them from the environment itself
  The private key never leaves KMS: jsign sends only the digest to kms:Sign.

  Safety rules: never signs the Microsoft ConPTY files, and refuses a file that
  already carries any signature (no double signing, nothing is replaced).
#>
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/signing-common.ps1"

foreach ($name in 'JSIGN_JAR', 'AWS_REGION', 'AWS_KMS_KEY_ALIAS', 'WINDOWS_SIGNING_TSA_URL') {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "sign-file: missing environment variable $name" }
}
if ($env:AWS_REGION -notmatch '^[a-z]{2}(-[a-z]+)+-\d+$') { throw "sign-file: unexpected AWS_REGION '$($env:AWS_REGION)'" }
if ($env:WINDOWS_SIGNING_TSA_URL -notmatch '^https?://[A-Za-z0-9._~:/?#@!$&*+%=-]+(,https?://[A-Za-z0-9._~:/?#@!$&*+%=-]+)*$') {
    throw 'sign-file: WINDOWS_SIGNING_TSA_URL must be one or more comma-separated http(s) URLs'
}
$jar = (Resolve-Path -LiteralPath $env:JSIGN_JAR).Path
$chain = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..\installer\windows\codesign-chain.pem')).Path
$file = (Resolve-Path -LiteralPath $Path).Path

if ([IO.Path]::GetFileName($file) -in $script:MicrosoftRedistributables) {
    throw "sign-file: refusing to sign $file (Microsoft redistributable, shipped as upstream built it)"
}
$existing = Get-AuthenticodeSignature -LiteralPath $file
if ($null -ne $existing.SignerCertificate) {
    throw "sign-file: $file already carries a signature ($($existing.SignerCertificate.Subject)); refusing to add another"
}

$jsignArgs = @(
    '-jar', $jar,
    '--storetype', 'AWS',
    '--keystore', $env:AWS_REGION,
    '--alias', $env:AWS_KMS_KEY_ALIAS,
    '--certfile', $chain,
    '--alg', 'SHA-256',
    # jsign defaults to the legacy Authenticode timestamp protocol; ask for RFC 3161.
    '--tsmode', 'RFC3161',
    '--tsaurl', $env:WINDOWS_SIGNING_TSA_URL,
    '--tsretries', '3',
    '--tsretrywait', '10',
    '--name', 'grimalkin',
    '--url', 'https://github.com/rlankenau/grimalkin',
    $file
)

# jsign already retries the timestamp request. This outer loop covers transient
# KMS / network failures. Backoff 5 s, 15 s between the three attempts.
$delays = 5, 15
for ($attempt = 1; ; $attempt++) {
    Write-Host "sign-file: signing $file (attempt $attempt of 3)"
    & java @jsignArgs
    if ($LASTEXITCODE -eq 0) { break }
    if ($attempt -ge 3) { throw "sign-file: jsign failed for $file (exit code $LASTEXITCODE)" }
    Write-Host "sign-file: jsign exit code $LASTEXITCODE; retrying in $($delays[$attempt - 1]) s"
    Start-Sleep -Seconds $delays[$attempt - 1]
}
# Callers verify the result with verify-signature.ps1 (Inno's temporary files
# may have extensions that Get-AuthenticodeSignature does not recognise, so no
# re-check here).
exit 0
