# Shared helpers for the Windows Authenticode scripts. Dot-source it:
#   . "$PSScriptRoot/signing-common.ps1"
# Requires PowerShell 7 (the GitHub Windows runners ship it as pwsh).

# Files that belong to Microsoft (the ConPTY redistributable). They are shipped
# exactly as upstream built them and must never be signed as grimalkin.
$script:MicrosoftRedistributables = @('conpty.dll', 'OpenConsole.exe')

# Reads every certificate of a PEM file, in file order (leaf first, then the
# intermediates). Throws when the file holds none, so the placeholder
# installer/windows/codesign-chain.pem can never be used by accident.
function Read-PemCertificates([Parameter(Mandatory)][string]$Path) {
    $text = Get-Content -LiteralPath $Path -Raw
    $found = [regex]::Matches($text, '-----BEGIN CERTIFICATE-----\s*([A-Za-z0-9+/=\s]+?)\s*-----END CERTIFICATE-----')
    if ($found.Count -eq 0) { throw "$Path contains no PEM certificates (is it still the placeholder?)" }
    $certs = foreach ($m in $found) {
        $der = [Convert]::FromBase64String(($m.Groups[1].Value -replace '\s', ''))
        [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($der)
    }
    return , @($certs)
}

function Get-CertSha256([System.Security.Cryptography.X509Certificates.X509Certificate2]$Cert) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Cert.RawData)) -replace '-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

# Finds signtool.exe from the Windows SDK. Prefers a native one, then x64 (runs
# under emulation on Windows 11 ARM64), then x86. Returns $null when not found.
function Find-SignTool {
    if ($env:WINDOWS_SIGNTOOL -and (Test-Path -LiteralPath $env:WINDOWS_SIGNTOOL)) { return $env:WINDOWS_SIGNTOOL }
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (-not (Test-Path -LiteralPath $kits)) { return $null }
    $native = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
    $versions = Get-ChildItem -LiteralPath $kits -Directory |
        Where-Object { $_.Name -match '^\d+(\.\d+){3}$' } |
        Sort-Object { [version]$_.Name } -Descending
    foreach ($arch in @($native, 'x64', 'x86') | Select-Object -Unique) {
        foreach ($v in $versions) {
            $candidate = Join-Path $v.FullName "$arch\signtool.exe"
            if (Test-Path -LiteralPath $candidate) { return $candidate }
        }
    }
    return $null
}
