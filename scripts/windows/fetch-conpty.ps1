<#
.SYNOPSIS
  Downloads the pinned Microsoft.Windows.Console.ConPTY NuGet package, checks
  its SHA-256, and extracts the conpty.dll / OpenConsole.exe pair of ONE
  architecture into a directory.

.DESCRIPTION
  A missing or empty entry fails: never fall back to another architecture's
  files. Used by the build job (extracts into dist) and by the sign job (extracts
  into a scratch directory to compare against what the build job staged).
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Sha256,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$Dest
)
$ErrorActionPreference = 'Stop'

# Paths inside the .nupkg for each architecture.
$entries = @{
    'x64'   = @{ 'runtimes/win-x64/native/conpty.dll'       = 'conpty.dll'; 'build/native/runtimes/x64/OpenConsole.exe'   = 'OpenConsole.exe' }
    'arm64' = @{ 'runtimes/win-arm64/native/conpty.dll'     = 'conpty.dll'; 'build/native/runtimes/arm64/OpenConsole.exe' = 'OpenConsole.exe' }
}[$Arch]

$pkg = Join-Path ([IO.Path]::GetTempPath()) "microsoft.windows.console.conpty.$Version.nupkg"
$url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$Version/microsoft.windows.console.conpty.$Version.nupkg"
Invoke-WebRequest -Uri $url -OutFile $pkg
$actual = (Get-FileHash -Algorithm SHA256 $pkg).Hash.ToLowerInvariant()
if ($actual -ne $Sha256.ToLowerInvariant()) {
    throw "SHA256 mismatch for ${pkg}: expected $Sha256, got $actual"
}

New-Item -ItemType Directory -Force -Path $Dest | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($pkg)
try {
    foreach ($name in $entries.Keys) {
        $entry = $zip.GetEntry($name)
        if ($null -eq $entry) { throw "$Arch asset $name not found in $pkg; refusing to ship another architecture under the $Arch label" }
        if ($entry.Length -eq 0) { throw "$name is empty in $pkg" }
        $target = Join-Path (Resolve-Path $Dest).Path $entries[$name]
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
    }
} finally { $zip.Dispose() }
