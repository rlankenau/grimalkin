<#
.SYNOPSIS
  Tests the signature verifier (verify-signature.ps1) and, optionally, the real
  signed files. Adapted from the negative tests of the closed Azure design (#4).

.DESCRIPTION
  Policy tests (always run, any platform with PowerShell 7): the OS trust APIs
  (signtool, Get-AuthenticodeSignature) are replaced by mocks and the policy that
  verify-signature.ps1 adds on top is exercised: bad signtool exit codes and
  warnings, wrong publisher, missing timestamp, invalid status, catalog instead of
  embedded signature, signer that is not the leaf of the committed chain, issuer
  that is not the chain's CA.

  Real-file tests (Windows, run by the "windows sign" job against COPIES of the
  files, never the files being released):
    -UnsignedPath  a copy of an unsigned binary: must be rejected
    -SignedPath    signed copies: must verify; must be rejected for a wrong
                   publisher and after a one-byte tamper
    -ConptyPath    a copy of conpty.dll / OpenConsole.exe: sign-file.ps1 must
                   refuse it
  with -ExpectedSubject / -ChainFile identifying the real publisher.
#>
param(
    [string]$UnsignedPath,
    [string[]]$SignedPath = @(),
    [string]$ConptyPath,
    [string]$ExpectedSubject,
    [string]$ChainFile
)
$ErrorActionPreference = 'Stop'
$verify = Join-Path $PSScriptRoot 'verify-signature.ps1'
$signFile = Join-Path $PSScriptRoot 'sign-file.ps1'
$temp = Join-Path ([IO.Path]::GetTempPath()) "grimalkin-signature-tests-$([guid]::NewGuid())"
New-Item -ItemType Directory $temp | Out-Null

function Assert-Rejected([string]$Name, [scriptblock]$Check, [string]$Match = '.') {
    $message = $null
    try { & $Check } catch { $message = "$_" }
    if ($null -eq $message) { throw "${Name}: unexpectedly succeeded" }
    if ($message -notmatch $Match) { throw "${Name}: rejected for the wrong reason: $message" }
    Write-Host "ok: ${Name}: rejected ($message)"
}

function New-TestCertificate([string]$Subject, $Issuer) {
    $x509 = 'System.Security.Cryptography.X509Certificates'
    $hash = [System.Security.Cryptography.HashAlgorithmName]::SHA256
    $pad = [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
    $key = [System.Security.Cryptography.RSA]::Create(2048)
    $req = New-Object "$x509.CertificateRequest" -ArgumentList $Subject, $key, $hash, $pad
    $now = [DateTimeOffset]::UtcNow
    if ($null -eq $Issuer) {
        $req.CertificateExtensions.Add((New-Object "$x509.X509BasicConstraintsExtension" -ArgumentList $true, $false, 0, $true))
        return $req.CreateSelfSigned($now.AddDays(-1), $now.AddDays(30))
    }
    $serial = [byte[]]::new(8)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($serial)
    $serial[0] = $serial[0] -band 0x7F
    return $req.Create($Issuer, $now.AddDays(-1), $now.AddDays(10), $serial)
}

try {
    # ---- policy tests with mocked OS trust APIs --------------------------------
    $subject = 'CN=Grimalkin Test Publisher'
    $ca = New-TestCertificate 'CN=Grimalkin Test CA' $null
    $leaf = New-TestCertificate $subject $ca
    $otherCa = New-TestCertificate 'CN=Another Test CA' $null
    $impostor = New-TestCertificate $subject $otherCa     # same subject, other issuer
    $goodChain = Join-Path $temp 'chain.pem'
    Set-Content $goodChain ($leaf.ExportCertificatePem() + "`n" + $ca.ExportCertificatePem())
    $wrongCaChain = Join-Path $temp 'chain-wrong-ca.pem'
    Set-Content $wrongCaChain ($leaf.ExportCertificatePem() + "`n" + $otherCa.ExportCertificatePem())
    $placeholder = Join-Path $temp 'placeholder.pem'
    Set-Content $placeholder '# no certificates here yet'

    $fixture = @{
        ExitCode  = 0
        Signature = [pscustomobject]@{
            Status                 = 'Valid'
            StatusMessage          = 'fixture'
            SignatureType          = 'Authenticode'
            SignerCertificate      = $leaf
            TimeStamperCertificate = $ca   # any non-null certificate stands for the TSA
        }
    }
    function Test-SignTool {
        if ($args[0] -ne 'verify' -or $args -notcontains '/pa' -or $args -notcontains '/all' -or $args -notcontains '/tw') {
            throw 'missing required signtool flags'
        }
        if ($args[-1] -notlike '*.com') { throw 'the distributed launcher file name was changed' }
        $global:LASTEXITCODE = $fixture.ExitCode
    }
    function Get-AuthenticodeSignature { param([string]$LiteralPath) $fixture.Signature }

    $launcher = Join-Path $temp 'launcher.com'
    Set-Content $launcher 'policy fixture (not an executable)'
    $check = { & $verify -Path $launcher -ExpectedSubject $subject -ChainFile $goodChain -SignTool Test-SignTool | Out-Null }

    & $check
    Write-Host 'ok: valid signature accepted'
    $fixture.ExitCode = 1
    Assert-Rejected 'signtool failure' $check 'signtool exit 1'
    $fixture.ExitCode = 2
    Assert-Rejected 'signtool warning (exit 2)' $check 'signtool exit 2'
    $fixture.ExitCode = 0
    Assert-Rejected 'unexpected publisher' { & $verify -Path $launcher -ExpectedSubject 'CN=Wrong publisher' -SignTool Test-SignTool } 'Unexpected publisher'
    $fixture.Signature.TimeStamperCertificate = $null
    Assert-Rejected 'missing timestamp' $check 'Missing trusted timestamp'
    $fixture.Signature.TimeStamperCertificate = $ca
    $fixture.Signature.Status = 'HashMismatch'
    Assert-Rejected 'invalid Authenticode status' $check 'not valid'
    $fixture.Signature.Status = 'Valid'
    $fixture.Signature.SignatureType = 'Catalog'
    Assert-Rejected 'catalog instead of embedded signature' $check 'embedded'
    $fixture.Signature.SignatureType = 'Authenticode'
    $fixture.Signature.SignerCertificate = $impostor
    Assert-Rejected 'right name, wrong certificate (not the chain leaf)' $check 'not the leaf'
    $fixture.Signature.SignerCertificate = $leaf
    Assert-Rejected 'leaf issued by a different CA than the chain file' {
        & $verify -Path $launcher -ExpectedSubject $subject -ChainFile $wrongCaChain -SignTool Test-SignTool
    } 'not the CA'
    Assert-Rejected 'placeholder chain file' {
        & $verify -Path $launcher -ExpectedSubject $subject -ChainFile $placeholder -SignTool Test-SignTool
    } 'no PEM certificates'
    Assert-Rejected 'missing file' {
        & $verify -Path (Join-Path $temp 'missing.com') -ExpectedSubject $subject -SignTool Test-SignTool
    }
    Remove-Item Function:\Test-SignTool, Function:\Get-AuthenticodeSignature

    # ---- preflight public-key comparison (real certificates, no AWS) ---------------
    . (Join-Path $PSScriptRoot 'signing-common.ps1')
    $ownSpki = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey($leaf).ExportSubjectPublicKeyInfo()
    if (-not (Test-CertificateMatchesPublicKey $leaf $ownSpki)) { throw 'a certificate must match its own public key' }
    Write-Host 'ok: certificate matches its own public key'
    $otherSpki = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey($ca).ExportSubjectPublicKeyInfo()
    if (Test-CertificateMatchesPublicKey $leaf $otherSpki) { throw 'a certificate must not match another key' }
    Write-Host 'ok: certificate does not match a different key'

    # ---- real files (Windows) -----------------------------------------------------
    if ($UnsignedPath) {
        if (-not $ExpectedSubject) { $ExpectedSubject = $subject }
        $chainArgs = @{}
        if ($ChainFile) { $chainArgs.ChainFile = $ChainFile }
        Assert-Rejected 'unsigned binary' { & $verify -Path $UnsignedPath -ExpectedSubject $ExpectedSubject @chainArgs } 'verification failed|not valid'
        foreach ($file in $SignedPath) {
            & $verify -Path $file -ExpectedSubject $ExpectedSubject @chainArgs
            Assert-Rejected "unexpected publisher ($file)" {
                & $verify -Path $file -ExpectedSubject 'CN=Not the grimalkin publisher' @chainArgs
            } 'Unexpected publisher|verification failed'
            $bytes = [IO.File]::ReadAllBytes((Resolve-Path $file).Path)
            # Alter a byte of the hashed DOS stub, not the PE checksum or the certificate table.
            $bytes[0x40] = $bytes[0x40] -bxor 1
            $tampered = Join-Path $temp ([IO.Path]::GetFileName($file))
            [IO.File]::WriteAllBytes($tampered, $bytes)
            Assert-Rejected "tampered binary ($file)" { & $verify -Path $tampered -ExpectedSubject $ExpectedSubject @chainArgs } 'verification failed|not valid'
            # sign-file must not stack a second signature on a signed file.
            $again = Join-Path $temp "again-$([IO.Path]::GetFileName($file))"
            Copy-Item $file $again
            Assert-Rejected "re-signing a signed file ($file)" { & $signFile -Path $again } 'already carries a signature'
        }
        if ($ConptyPath) {
            $copy = Join-Path $temp ([IO.Path]::GetFileName($ConptyPath))
            Copy-Item $ConptyPath $copy
            Assert-Rejected "signing $copy" { & $signFile -Path $copy } 'Microsoft redistributable'
        }
    }
    Write-Host 'all signature verifier tests passed'
} finally {
    Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue
    # Expected native failures must not leak into the step's exit code.
    $global:LASTEXITCODE = 0
}
