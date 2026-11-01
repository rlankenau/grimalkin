<#
.SYNOPSIS
  Fails unless every file has ProductName "Grimalkin" and the given ProductVersion.

.DESCRIPTION
  SignPath Foundation requires product name and version to be enforced for every
  signed file; the artifact configurations in installer/windows/signpath/ do that
  on SignPath's side. This runs the same check before a request is submitted, so a
  mismatch fails in CI instead of after a manual approval. Both comparisons are
  case-sensitive. Trailing spaces are ignored on purpose: Inno Setup writes the
  strings into fixed-size slots of its stub, so Setup and the uninstaller carry
  "Grimalkin" followed by padding (observed in CI); a warning says when that happens.
#>
param(
    [Parameter(Mandatory)][string[]]$Path,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
foreach ($file in $Path) {
    $info = (Get-Item -LiteralPath $file -ErrorAction Stop).VersionInfo
    $name = "$($info.ProductName)"
    $ver = "$($info.ProductVersion)"
    if ($name.TrimEnd() -cne 'Grimalkin') { throw "${file}: ProductName is '$name', expected 'Grimalkin'" }
    if ($ver.TrimEnd() -cne $Version) { throw "${file}: ProductVersion is '$ver', expected '$Version'" }
    if ($name -cne $name.TrimEnd() -or $ver -cne $ver.TrimEnd()) {
        Write-Host "::warning::${file}: Inno Setup padded ProductName/ProductVersion with trailing spaces ($($name.Length)/$($ver.Length) characters); SignPath's metadata restriction must accept that"
    }
    Write-Host "ok: $file is Grimalkin $Version"
}
