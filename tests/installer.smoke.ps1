$ErrorActionPreference = 'Stop'
$version = (Get-Content -Raw package.json | ConvertFrom-Json).version
$installer = (Resolve-Path "release/NIP-Setup-$version-x64.exe").Path
$uninstallRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$existing = @(Get-ChildItem $uninstallRoot | Get-ItemProperty | Where-Object { $_.DisplayName -match '^NIP(?: \d.*)?$' })
if ($existing.Count -gt 0) { throw 'NIP is already installed for this user; refusing to change it.' }
foreach ($shortcut in @((Join-Path ([Environment]::GetFolderPath('Desktop')) 'NIP.lnk'), (Join-Path ([Environment]::GetFolderPath('Programs')) 'NIP.lnk'))) {
    if (Test-Path -LiteralPath $shortcut) { throw "Existing shortcut: $shortcut; refusing to replace it." }
}
$testRoot = Join-Path (Get-Location) ('work/installer-' + [guid]::NewGuid().ToString('N'))
$installDir = Join-Path $testRoot 'app'
$profileDir = Join-Path $testRoot 'profile'
New-Item -ItemType Directory -Path $profileDir -Force | Out-Null
$marker = Join-Path $profileDir 'keep.txt'
Set-Content -LiteralPath $marker -Value 'installer-smoke'
$previousExecutable = $env:NIP_EXECUTABLE
$previousProfile = $env:NIP_SMOKE_DATA_DIR
$desktopLink = Join-Path ([Environment]::GetFolderPath('Desktop')) 'NIP.lnk'
$startLink = Join-Path ([Environment]::GetFolderPath('Programs')) 'NIP.lnk'
try {
    $process = Start-Process -FilePath $installer -ArgumentList @('/S', "/D=$installDir") -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Installer exited $($process.ExitCode)" }
    $exe = Join-Path $installDir 'NIP.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw 'Installed executable missing' }
    $entries = @(Get-ChildItem $uninstallRoot | Get-ItemProperty | Where-Object { $_.DisplayName -match '^NIP(?: \d.*)?$' })
    if ($entries.Count -ne 1) { throw 'Expected one per-user uninstall entry' }
    if (-not (Test-Path -LiteralPath $desktopLink)) { throw 'Desktop shortcut missing' }
    if (-not (Test-Path -LiteralPath $startLink)) { throw 'Start menu shortcut missing' }
    $env:NIP_EXECUTABLE = $exe
    $env:NIP_SMOKE_DATA_DIR = $profileDir
    npm run test:package
    if ($LASTEXITCODE -ne 0) { throw 'Installed app smoke test failed' }
} finally {
    $env:NIP_EXECUTABLE = $previousExecutable
    $env:NIP_SMOKE_DATA_DIR = $previousProfile
    $uninstaller = Join-Path $installDir 'Uninstall NIP.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "Uninstaller exited $($process.ExitCode)" }
        $deadline = (Get-Date).AddSeconds(40)
        while ((Test-Path -LiteralPath (Join-Path $installDir 'NIP.exe')) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
        if (Test-Path -LiteralPath (Join-Path $installDir 'NIP.exe')) { throw 'Uninstaller left the executable behind' }
        $remaining = @(Get-ChildItem $uninstallRoot | Get-ItemProperty | Where-Object { $_.DisplayName -match '^NIP(?: \d.*)?$' })
        if ($remaining.Count -ne 0) { throw 'Uninstall entry was not removed' }
        if ((Test-Path -LiteralPath $desktopLink) -or (Test-Path -LiteralPath $startLink)) { throw 'Uninstaller left shortcuts behind' }
    }
}
if ((Get-Content -Raw -LiteralPath $marker).Trim() -ne 'installer-smoke') { throw 'Isolated profile was changed by uninstall' }
Write-Output 'PASS: per-user installation, shortcuts, installed app/server, uninstall and isolated profile preservation.'
