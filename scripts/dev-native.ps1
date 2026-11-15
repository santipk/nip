param(
    [ValidatePattern('^[a-zA-Z0-9_-]+$')][string]$Profile = 'dev',
    [switch]$Release,
    [switch]$AllowNetwork
)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path $PSScriptRoot -Parent
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
if (-not (Test-Path -LiteralPath $cargo)) { throw 'Install Rust with rustup from https://rustup.rs and MSVC build tools first.' }
$data = Join-Path $workspace "work/native-$Profile"
New-Item -ItemType Directory -Path $data -Force | Out-Null
Push-Location (Join-Path $workspace 'native')
try {
    $arguments = @('run', '--locked')
    if ($Release) { $arguments += '--release' }
    $arguments += @('--bin', 'nip-native', '--', '--data-dir', $data)
    if (-not $AllowNetwork) { $arguments += '--local-only' }
    & $cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "Native run failed: $LASTEXITCODE" }
} finally { Pop-Location }
