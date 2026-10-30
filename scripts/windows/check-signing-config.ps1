<#
.SYNOPSIS
  Pre-flight for the release signing job: proves the committed certificate chain
  and the KMS key belong together BEFORE anything is signed.

.DESCRIPTION
  Fails unless
    - installer/windows/codesign-chain.pem holds a leaf (first certificate) whose
      Subject equals -ExpectedSubject exactly, which is currently valid, has the
      Code Signing extended key usage and chains to a root the runner trusts;
    - the KMS key (-KeyAlias, in -Region) is an enabled RSA SIGN_VERIFY key
      that supports RSASSA_PKCS1_V1_5_SHA_256; and
    - the leaf certificate's public key is byte-identical to the KMS public key.
  Needs AWS credentials (configure-aws-credentials) and the preinstalled AWS CLI;
  kms:GetPublicKey is one of the three actions the role is allowed.
#>
param(
    [Parameter(Mandatory)][string]$ChainFile,
    [Parameter(Mandatory)][string]$ExpectedSubject,
    [Parameter(Mandatory)][string]$KeyAlias,
    [Parameter(Mandatory)][string]$Region
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/signing-common.ps1"

$certs = Read-PemCertificates $ChainFile
$leaf = $certs[0]
Write-Host "leaf   : $($leaf.Subject)"
Write-Host "issuer : $($leaf.Issuer)"
Write-Host "valid  : $($leaf.NotBefore.ToString('u')) .. $($leaf.NotAfter.ToString('u'))"
if ($leaf.Subject -cne $ExpectedSubject) {
    throw "the leaf certificate's Subject '$($leaf.Subject)' is not WINDOWS_SIGNING_PUBLISHER '$ExpectedSubject'"
}
$now = Get-Date
if ($now -lt $leaf.NotBefore -or $now -gt $leaf.NotAfter) { throw 'the leaf certificate is not currently valid' }
if (($leaf.NotAfter - $now).TotalDays -lt 30) { Write-Host '::warning::the code signing certificate expires in less than 30 days; renew it' }
$eku = $leaf.Extensions | Where-Object { $_ -is [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension] }
if ($null -eq $eku -or -not ($eku.EnhancedKeyUsages | Where-Object { $_.Value -eq '1.3.6.1.5.5.7.3.3' })) {
    throw 'the leaf certificate does not have the Code Signing extended key usage'
}
$x509chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
$x509chain.ChainPolicy.RevocationMode = 'NoCheck'
foreach ($c in $certs | Select-Object -Skip 1) { [void]$x509chain.ChainPolicy.ExtraStore.Add($c) }
if (-not $x509chain.Build($leaf)) {
    throw "the certificate chain does not build to a trusted root: $(($x509chain.ChainStatus | ForEach-Object { $_.StatusInformation.Trim() }) -join '; ')"
}

$json = aws kms get-public-key --key-id $KeyAlias --region $Region --output json
if ($LASTEXITCODE -ne 0) { throw "aws kms get-public-key failed for $KeyAlias in $Region" }
$key = $json | ConvertFrom-Json
Write-Host "KMS key: $($key.KeyId) spec=$($key.KeySpec) usage=$($key.KeyUsage)"
if ($key.KeyUsage -ne 'SIGN_VERIFY') { throw "the KMS key usage is $($key.KeyUsage), not SIGN_VERIFY" }
if ($key.KeySpec -notin 'RSA_3072', 'RSA_4096') { throw "the KMS key spec is $($key.KeySpec); expected RSA_3072 or RSA_4096" }
if ('RSASSA_PKCS1_V1_5_SHA_256' -notin $key.SigningAlgorithms) { throw 'the KMS key does not support RSASSA_PKCS1_V1_5_SHA_256' }
$kmsSpki = [Convert]::FromBase64String($key.PublicKey)
if (-not (Test-CertificateMatchesPublicKey $leaf $kmsSpki)) {
    throw 'the public key of the leaf certificate is not the public key of the KMS key'
}
Write-Host 'signing configuration is consistent: the certificate belongs to the KMS key'
