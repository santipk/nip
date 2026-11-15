param([switch]$TestBuild, [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path $PSScriptRoot -Parent
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
$manifest = Get-Content -Raw (Join-Path $workspace 'native/Cargo.toml')
$version = [regex]::Match($manifest, '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $version) { throw 'Native package version missing' }
function Get-NativeSourceFingerprint {
    [string[]]$paths = @(& git -C $workspace ls-files --cached --others --exclude-standard -- native scripts/package-native.ps1 LICENSE)
    if ($LASTEXITCODE -ne 0) { throw 'Could not inventory native source inputs' }
    [Array]::Sort($paths, [StringComparer]::Ordinal)
    $entries = foreach ($path in $paths) {
        $fullPath = Join-Path $workspace $path
        $hash = if (Test-Path -LiteralPath $fullPath -PathType Leaf) { (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'deleted' }
        "$path`t$hash"
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($entries -join "`n"))))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}
$sourceHash = Get-NativeSourceFingerprint
Push-Location (Join-Path $workspace 'native')
try {
    if (-not $SkipBuild) {
        & $cargo build --release --locked --bins
        if ($LASTEXITCODE -ne 0) { throw 'Native release build failed' }
    }
    $binary = Join-Path $workspace 'native/target/x86_64-pc-windows-msvc/release/nip-native.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw 'Verified native executable missing' }
    $relayBinary = Join-Path $workspace 'native/target/x86_64-pc-windows-msvc/release/nip-relay.exe'
    if (-not (Test-Path -LiteralPath $relayBinary)) { throw 'Verified relay executable missing' }
    $outDir = Join-Path $workspace $(if ($TestBuild) { 'work/native-installer-test' } else { 'release/native' })
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
    $relayOutput = Join-Path $outDir "NIP-Relay-$version-x64.exe"
    Copy-Item -LiteralPath $relayBinary -Destination $relayOutput -Force
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
    if ((Get-NativeSourceFingerprint) -ne $sourceHash) { throw 'Native source inputs changed during packaging; rebuild from a stable working tree.' }
    $artifacts = foreach ($file in @($binary, $output, $relayOutput, $notices)) {
        [ordered]@{
            path = $file.Substring($workspace.Length + 1).Replace('\', '/')
            bytes = (Get-Item -LiteralPath $file).Length
            sha256 = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }
    $commit = & git -C $workspace rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify source commit' }
    $status = @(& git -C $workspace status --porcelain --untracked-files=normal)
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify working tree state' }
    $rustVersion = & $cargo --version
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify Cargo version' }
    $compilerVersion = & (Join-Path (Split-Path $cargo -Parent) 'rustc.exe') --version
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify Rust compiler version' }
    $candidate = [ordered]@{
        schemaVersion = 1
        createdUtc = [DateTime]::UtcNow.ToString('o')
        version = $version
        target = 'x86_64-pc-windows-msvc'
        testBuild = [bool]$TestBuild
        buildSkipped = [bool]$SkipBuild
        sourceCommit = "$commit"
        workingTreeDirty = $status.Count -gt 0
        nativeSourceSha256 = $sourceHash
        cargoVersion = "$rustVersion"
        rustcVersion = "$compilerVersion"
        cargoLockSha256 = (Get-FileHash -LiteralPath (Join-Path $workspace 'native/Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
        artifacts = @($artifacts)
    }
    [IO.File]::WriteAllText((Join-Path $outDir 'candidate-manifest.json'), ($candidate | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
    $sums = foreach ($artifact in $artifacts | Select-Object -Skip 1) { "$($artifact.sha256)  $([IO.Path]::GetFileName($artifact.path))" }
    [IO.File]::WriteAllLines((Join-Path $outDir 'SHA256SUMS.txt'), [string[]]$sums, [Text.Encoding]::ASCII)
    Write-Output $output
} finally { Pop-Location }
