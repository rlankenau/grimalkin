# Run on a disposable Windows machine: installs per-user and exercises integration.
param(
    [string]$Dist = 'dist',
    [string]$Archive = 'grimalkin-portable-x64.zip',
    [string]$Installer = 'installer-out/grimalkin-setup-x64.exe',
    [switch]$Signed,
    [string]$ExpectedSubject = $env:WINDOWS_SIGNING_SUBJECT
)

$ErrorActionPreference = 'Stop'
if ($Signed -and -not $ExpectedSubject) { throw 'ExpectedSubject is required for signed distributions' }
$verify = Join-Path $PSScriptRoot 'verify-windows-signature.ps1'
$firstParty = @('grimalkin.exe', 'grimalkin.com')
$payload = @($firstParty) + @('conpty.dll', 'OpenConsole.exe', 'LICENSE-ConPTY.txt', 'README.md', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE')
$extracted = Join-Path ([IO.Path]::GetTempPath()) "grimalkin-verify-$([guid]::NewGuid())"
$dir = Join-Path $env:LOCALAPPDATA 'Programs\grimalkin'
if (Test-Path $dir) { throw "Use a clean test account: $dir already exists" }

function Test-Payload([string]$Directory) {
    foreach ($name in $payload) {
        $file = Join-Path $Directory $name
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing payload: $file" }
        if ((Get-FileHash -LiteralPath $file).Hash -ne (Get-FileHash -LiteralPath (Join-Path $Dist $name)).Hash) {
            throw "Packaged bytes differ from staged output: $file"
        }
    }
    if ($Signed) {
        & $verify -Path ($firstParty | ForEach-Object { Join-Path $Directory $_ }) -ExpectedSubject $ExpectedSubject
    }
}

function Test-Launcher([string]$Directory) {
    $launcher = Join-Path (Resolve-Path $Directory).Path 'grimalkin.com'
    $output = & $launcher --version
    if ($LASTEXITCODE -ne 0 -or "$output" -notmatch 'grimalkin \d+\.\d+\.\d+') {
        throw "Launcher --version failed: $output"
    }
    $output = & $launcher --help
    if ($LASTEXITCODE -ne 0 -or "$output" -notmatch '--working-directory') {
        throw 'Launcher --help failed'
    }
    & $launcher --grimalkin-invalid-option 2>&1 | Out-Host
    if ($LASTEXITCODE -eq 0) { throw 'Launcher did not forward the error exit code' }
    $global:LASTEXITCODE = 0
}

try {
    if ($Signed) {
        & $verify -Path $Installer -ExpectedSubject $ExpectedSubject
    } else {
        Write-Warning 'UNSIGNED contributor build: no publisher or timestamp assurance'
    }
    Test-Payload $Dist
    Test-Launcher $Dist
    Expand-Archive -LiteralPath $Archive -DestinationPath $extracted
    Test-Payload $extracted
    Test-Launcher $extracted

    $setup = (Resolve-Path $Installer).Path
    $p = Start-Process $setup -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/TASKS=addtopath,contextmenu','/LOG=install.log'
    if ($p.ExitCode -ne 0) { Get-Content install.log; throw "Installer exit code $($p.ExitCode)" }
    Test-Payload $dir
    $uninstaller = Join-Path $dir 'unins000.exe'
    if (-not (Test-Path $uninstaller)) { throw 'Missing uninstaller' }
    if ($Signed) { & $verify -Path $uninstaller -ExpectedSubject $ExpectedSubject }
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (($userPath -split ';') -contains $dir)) { throw "Install dir not on user PATH: $userPath" }
    $cmd = (Get-ItemProperty 'HKCU:\Software\Classes\Directory\Background\shell\grimalkin\command').'(default)'
    if ($cmd -notlike '*--working-directory*') { throw 'Context menu command missing' }
    Test-Launcher $dir

    # _?= runs the uninstaller in place so -Wait really waits.
    $u = Start-Process $uninstaller -Wait -PassThru -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"_?=$dir"
    if ($u.ExitCode -ne 0) { throw "Uninstaller exit code $($u.ExitCode)" }
    foreach ($name in $payload) {
        if (Test-Path (Join-Path $dir $name)) { throw "Payload left after uninstall: $name" }
    }
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -contains $dir) { throw 'Install dir still on user PATH after uninstall' }
    foreach ($key in 'Directory\shell\grimalkin', 'Directory\Background\shell\grimalkin') {
        if (Test-Path "HKCU:\Software\Classes\$key") { throw "Context menu key left behind: $key" }
    }
    Write-Host "Distribution checks passed on $((Get-CimInstance Win32_OperatingSystem).Caption)"
} finally {
    if (Test-Path $extracted) { Remove-Item $extracted -Recurse -Force }
}
