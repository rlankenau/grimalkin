# Policy tests run anywhere; optional integration tests use real Windows PE files.
param(
    [string]$UnsignedPath,
    [string[]]$SignedPath = @(),
    [string]$ExpectedSubject = $env:WINDOWS_SIGNING_SUBJECT
)

$ErrorActionPreference = 'Stop'
$verify = Join-Path $PSScriptRoot 'verify-windows-signature.ps1'
$temp = Join-Path ([IO.Path]::GetTempPath()) "grimalkin-signature-tests-$([guid]::NewGuid())"
New-Item -ItemType Directory $temp | Out-Null
if (-not $ExpectedSubject) { $ExpectedSubject = 'CN=Unsigned test fixture' }

function Assert-Rejected([string]$Name, [scriptblock]$Check) {
    $rejected = $false
    try { & $Check } catch {
        $rejected = $true
        Write-Host "${Name}: correctly rejected ($_)"
    }
    if (-not $rejected) { throw "${Name}: verification unexpectedly succeeded" }
}

try {
    # Mock only OS trust APIs, not the publisher/timestamp policy being tested.
    & {
        $fixture = @{
            ExitCode = 0
            Signature = [pscustomobject]@{
                Status = 'Valid'
                SignatureType = 'Authenticode'
                SignerCertificate = [pscustomobject]@{ Subject = $ExpectedSubject }
                TimeStamperCertificate = [pscustomobject]@{ Subject = 'CN=Test TSA' }
            }
        }
        function Test-SignTool {
            if ($args[0] -ne 'verify' -or $args -notcontains '/pa' -or $args -notcontains '/all' -or $args -notcontains '/tw') {
                throw 'Missing required SignTool verification flags'
            }
            if ($args[-1] -notlike '*.com') { throw 'Distributed launcher filename was changed' }
            $global:LASTEXITCODE = $fixture.ExitCode
        }
        function Get-AuthenticodeSignature {
            param([string]$LiteralPath)
            $fixture.Signature
        }
        $launcher = Join-Path $temp 'launcher.com'
        Set-Content $launcher 'Policy fixture (not an executable)'
        $check = { & $verify -Path $launcher -ExpectedSubject $ExpectedSubject -SignTool Test-SignTool }
        & $check
        $fixture.ExitCode = 1
        Assert-Rejected 'Invalid signature' $check
        $fixture.ExitCode = 2
        Assert-Rejected 'SignTool warning' $check
        $fixture.ExitCode = 0
        $fixture.Signature.SignerCertificate.Subject = 'CN=Wrong publisher'
        Assert-Rejected 'Unexpected publisher' $check
        $fixture.Signature.SignerCertificate.Subject = $ExpectedSubject
        $fixture.Signature.TimeStamperCertificate = $null
        Assert-Rejected 'Missing timestamp' $check
        $fixture.Signature.TimeStamperCertificate = [pscustomobject]@{ Subject = 'CN=Test TSA' }
        $fixture.Signature.Status = 'HashMismatch'
        Assert-Rejected 'Invalid Authenticode status' $check
        $fixture.Signature.Status = 'Valid'
        $fixture.Signature.SignatureType = 'Catalog'
        Assert-Rejected 'Catalog instead of embedded signature' $check
    }
    if (-not $UnsignedPath) { return }
    Assert-Rejected 'Missing file' {
        & $verify -Path (Join-Path $temp 'missing.exe') -ExpectedSubject $ExpectedSubject
    }
    Assert-Rejected 'Unsigned binary' {
        & $verify -Path $UnsignedPath -ExpectedSubject $ExpectedSubject
    }
    foreach ($file in $SignedPath) {
        & $verify -Path $file -ExpectedSubject $ExpectedSubject
        Assert-Rejected "Unexpected publisher ($file)" {
            & $verify -Path $file -ExpectedSubject 'CN=Not the Grimalkin publisher'
        }
        $bytes = [IO.File]::ReadAllBytes((Resolve-Path $file).Path)
        # Alter the hashed DOS stub, not the PE checksum or certificate table.
        $bytes[0x40] = $bytes[0x40] -bxor 1
        $tampered = Join-Path $temp ([IO.Path]::GetFileName($file))
        [IO.File]::WriteAllBytes($tampered, $bytes)
        Assert-Rejected "Tampered binary ($file)" {
            & $verify -Path $tampered -ExpectedSubject $ExpectedSubject
        }
    }
} finally {
    Remove-Item $temp -Recurse -Force
    # Expected native verification failures must not become the CI step exit code.
    $global:LASTEXITCODE = 0
}
