# Also used as Inno Setup's synchronous SignTool callback ($f).
param([Parameter(Mandatory)][string]$Path)

$ErrorActionPreference = 'Stop'
foreach ($name in 'WINDOWS_SIGNING_ENDPOINT','WINDOWS_SIGNING_ACCOUNT','WINDOWS_SIGNING_PROFILE','WINDOWS_SIGNING_SUBJECT') {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Missing signing configuration: $name" }
}
$file = (Resolve-Path -LiteralPath $Path).Path
if ([IO.Path]::GetFileName($file) -in @('conpty.dll', 'OpenConsole.exe')) {
    throw 'Never re-sign Microsoft redistributables'
}

Import-Module ArtifactSigning -RequiredVersion 0.1.20 -ErrorAction Stop
$options = @{
    Endpoint = $env:WINDOWS_SIGNING_ENDPOINT
    CodeSigningAccountName = $env:WINDOWS_SIGNING_ACCOUNT
    CertificateProfileName = $env:WINDOWS_SIGNING_PROFILE
    Files = $file
    FileDigest = 'SHA256'
    TimestampRfc3161 = 'http://timestamp.acs.microsoft.com'
    TimestampDigest = 'SHA256'
    # Only the Azure CLI session established with release OIDC is eligible.
    ExcludeEnvironmentCredential = $true
    ExcludeWorkloadIdentityCredential = $true
    ExcludeManagedIdentityCredential = $true
    ExcludeSharedTokenCacheCredential = $true
    ExcludeVisualStudioCredential = $true
    ExcludeVisualStudioCodeCredential = $true
    ExcludeAzureCliCredential = $false
    ExcludeAzurePowerShellCredential = $true
    ExcludeAzureDeveloperCliCredential = $true
    ExcludeInteractiveBrowserCredential = $true
}
Invoke-ArtifactSigning @options -ErrorAction Stop
& (Join-Path $PSScriptRoot 'verify-windows-signature.ps1') -Path $file -ExpectedSubject $env:WINDOWS_SIGNING_SUBJECT
