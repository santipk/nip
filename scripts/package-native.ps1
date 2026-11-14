param([switch]$TestBuild, [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path $PSScriptRoot -Parent
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
$manifest = Get-Content -Raw (Join-Path $workspace 'native/Cargo.toml')
$version = [regex]::Match($manifest, '(?m)^version = "([^"]+)"').Groups[1].Value
Push-Location (Join-Path $workspace 'native')
try {
    if (-not $SkipBuild) {
        & $cargo build --release --locked --bin nip-native
        if ($LASTEXITCODE -ne 0) { throw 'Native release build failed' }
    }
    $binary = Join-Path $workspace 'native/target/x86_64-pc-windows-msvc/release/nip-native.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw 'Verified native executable missing' }
    $outDir = Join-Path $workspace $(if ($TestBuild) { 'work/native-installer-test' } else { 'release/native' })
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
    $notices = Join-Path $outDir 'THIRD_PARTY_NOTICES.txt'
    # Include license texts from the locked dependency sources, including vendored libopus/fonts.
    $metadataText = & $cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect locked dependency licenses' }
    $metadata = $metadataText | ConvertFrom-Json
    $noticeText = [Text.StringBuilder]::new()
    [void]$noticeText.AppendLine('NIP Native — third-party notices (locked Rust dependency graph)')
    foreach ($package in ($metadata.packages | Sort-Object name,version)) {
        if ($package.name -eq 'nip-native') { continue }
        [void]$noticeText.AppendLine("`n=== $($package.name) $($package.version) — $($package.license) ===")
        [void]$noticeText.AppendLine($package.repository)
        $source = Split-Path $package.manifest_path -Parent
        $licenses = @(Get-ChildItem -LiteralPath $source -File -Recurse | Where-Object { $_.Name -match '(LICENSE|LICENCE|COPYING|NOTICE)' })
        foreach ($file in $licenses) {
            [void]$noticeText.AppendLine("`n--- $($file.Name) ---")
            [void]$noticeText.AppendLine([IO.File]::ReadAllText($file.FullName))
        }
        if ($licenses.Count -eq 0) { [void]$noticeText.AppendLine('License expression above; see linked repository for full upstream terms.') }
    }
    [IO.File]::WriteAllText($notices, $noticeText.ToString(), [Text.UTF8Encoding]::new($false))
    $makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
    if (-not $makensis) {
        foreach ($programRoot in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
            if (-not $programRoot) { continue }
            $candidate = Join-Path $programRoot 'NSIS/makensis.exe'
            if (Test-Path -LiteralPath $candidate) { $makensis = $candidate; break }
        }
    }
    if (-not $makensis) {
        $cache = Join-Path $env:LOCALAPPDATA 'electron-builder/Cache/nsis-3.0.4.1/nsis-3.0.4.1-1mx3n/Bin/makensis.exe'
        if (Test-Path -LiteralPath $cache) { $makensis = $cache }
    }
    if (-not $makensis) { throw 'Install official NSIS (https://nsis.sourceforge.io) to build the installer.' }
    $output = Join-Path $outDir "NIP-Native-Setup-$version-x64.exe"
    $arguments = @('/V2', '/INPUTCHARSET', 'UTF8', "/DVERSION=$version", "/DBINARY=$binary", "/DNOTICES=$notices", "/DOUTPUT=$output")
    if ($TestBuild) { $arguments += '/DTEST_BUILD' }
    $arguments += 'installer.nsi'
    & $makensis @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Native installer compilation failed' }
    $hash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($output))" | Set-Content -Encoding ascii (Join-Path $outDir 'SHA256SUMS.txt')
    Write-Output $output
} finally { Pop-Location }
