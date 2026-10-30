<#
.SYNOPSIS
  Fails unless every named PE file in a directory targets the expected CPU.

.DESCRIPTION
  Reads IMAGE_FILE_HEADER.Machine straight from the file (no tools needed) and
  compares it with the architecture the build is labelled with. Used by CI so
  an ARM64 package can never contain an x64 binary (or the other way around),
  e.g. a ConPTY DLL of the wrong architecture, which would silently fall back
  to the in-box ConPTY at run time.

.EXAMPLE
  scripts/windows/check-pe-machine.ps1 -Dir dist -Arch arm64
#>
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [string[]]$Files = @('grimalkin.exe', 'grimalkin.com', 'conpty.dll', 'OpenConsole.exe')
)
$ErrorActionPreference = 'Stop'

$machines = @{ 0x8664 = 'x64'; 0xAA64 = 'arm64'; 0x014C = 'x86'; 0xA641 = 'arm64ec' }
$expected = @{ 'x64' = 0x8664; 'arm64' = 0xAA64 }[$Arch]

function Get-PeMachine([string]$Path) {
    $fs = [System.IO.File]::OpenRead($Path)
    try {
        $br = New-Object System.IO.BinaryReader($fs)
        if ($fs.Length -lt 0x40 -or $br.ReadUInt16() -ne 0x5A4D) { throw "$Path is not a PE file (no MZ header)" }
        $fs.Seek(0x3C, 'Begin') | Out-Null
        $peOffset = $br.ReadUInt32()
        $fs.Seek($peOffset, 'Begin') | Out-Null
        if ($br.ReadUInt32() -ne 0x00004550) { throw "$Path is not a PE file (no PE signature)" }
        return [int]$br.ReadUInt16()
    } finally { $fs.Dispose() }
}

$bad = @()
foreach ($name in $Files) {
    $path = Join-Path $Dir $name
    if (-not (Test-Path -LiteralPath $path)) { $bad += "${name}: missing in $Dir"; continue }
    $m = Get-PeMachine (Resolve-Path -LiteralPath $path).Path
    $label = if ($machines.ContainsKey($m)) { $machines[$m] } else { 'unknown' }
    '{0,-18} machine=0x{1:X4} ({2})' -f $name, $m, $label | Write-Host
    if ($m -ne $expected) { $bad += ("{0}: machine 0x{1:X4} ({2}), expected 0x{3:X4} ({4})" -f $name, $m, $label, $expected, $Arch) }
}
if ($bad.Count -gt 0) {
    $bad | ForEach-Object { Write-Host "::error::$_" }
    throw "machine type check failed for the $Arch package:`n$($bad -join "`n")"
}
Write-Host "all $($Files.Count) binaries are $Arch"
