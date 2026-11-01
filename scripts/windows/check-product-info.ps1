<#
.SYNOPSIS
  Fails unless every file has ProductName "Grimalkin" and the given ProductVersion.

.DESCRIPTION
  SignPath Foundation requires product name and version to be enforced for every
  signed file; the artifact configurations in installer/windows/signpath/ do that
  on SignPath's side. This runs the same check before a request is submitted, so a
  mismatch fails in CI instead of after a manual approval. Both comparisons are
  case-sensitive.
#>
param(
    [Parameter(Mandatory)][string[]]$Path,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
foreach ($file in $Path) {
    $info = (Get-Item -LiteralPath $file -ErrorAction Stop).VersionInfo
    if ($info.ProductName -cne 'Grimalkin') { throw "${file}: ProductName is '$($info.ProductName)', expected 'Grimalkin'" }
    if ($info.ProductVersion -cne $Version) { throw "${file}: ProductVersion is '$($info.ProductVersion)', expected '$Version'" }
    Write-Host "ok: $file is Grimalkin $Version"
}
