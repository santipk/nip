param([switch]$TestBuild)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path $PSScriptRoot -Parent
if (-not $TestBuild -and $env:CI -ne 'true') { throw 'Production installer smoke is restricted to a fresh CI runner. Use -TestBuild locally.' }
$manifest = Get-Content -Raw (Join-Path $workspace 'native/Cargo.toml')
$version = [regex]::Match($manifest, '(?m)^version = "([^"]+)"').Groups[1].Value
$product = if ($TestBuild) { 'NIP Native Test' } else { 'NIP Native' }
$exeName = if ($TestBuild) { 'NIPNativeTest.exe' } else { 'NIPNative.exe' }
$keyName = if ($TestBuild) { 'NIPNativeInstallerTest' } else { 'NIPNative' }
$reg = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$keyName"
if (Test-Path -LiteralPath $reg) { throw "Existing $product installation; refusing to replace it" }
$shortcuts = @((Join-Path ([Environment]::GetFolderPath('Desktop')) "$product.lnk"), (Join-Path ([Environment]::GetFolderPath('Programs')) "$product.lnk"))
foreach ($link in $shortcuts) { if (Test-Path -LiteralPath $link) { throw "Existing shortcut: $link" } }
$outDir = if ($TestBuild) { 'work/native-installer-test' } else { 'release/native' }
$installer = (Resolve-Path (Join-Path $workspace "$outDir/NIP-Native-Setup-$version-x64.exe")).Path
$testRoot = Join-Path $workspace ('work/native-install-smoke-' + [guid]::NewGuid().ToString('N'))
$installDir = Join-Path $testRoot 'app'
$profile = Join-Path $testRoot 'profile'
New-Item -ItemType Directory -Path $profile -Force | Out-Null
$marker = Join-Path $profile 'keep.txt'
Set-Content -LiteralPath $marker -Value 'keep native user profile'
$realElectron = Join-Path $env:LOCALAPPDATA 'Programs/nip-desktop/NIP.exe'
$oldHash = if (Test-Path -LiteralPath $realElectron) { (Get-FileHash -LiteralPath $realElectron -Algorithm SHA256).Hash } else { $null }
try {
    $setup = Start-Process -FilePath $installer -ArgumentList @('/S', "/D=$installDir") -WindowStyle Hidden -Wait -PassThru
    if ($setup.ExitCode -ne 0) { throw "Installer exit $($setup.ExitCode)" }
    $exe = Join-Path $installDir $exeName
    if (-not (Test-Path -LiteralPath $exe)) { throw 'Installed executable missing' }
    if ((Get-ItemProperty -LiteralPath $reg).InstallLocation -ne $installDir) { throw 'Unexpected uninstall registration' }
    foreach ($link in $shortcuts) { if (Test-Path -LiteralPath $link) { throw 'Silent install must not create unchecked interactive shortcuts' } }
    $startupError = Join-Path $testRoot 'startup-stderr.txt'
    $startupOutput = Join-Path $testRoot 'startup-stdout.txt'
    $app = Start-Process -FilePath $exe -ArgumentList @('--data-dir', "`"$profile`"", '--quit-after', '4') -RedirectStandardError $startupError -RedirectStandardOutput $startupOutput -PassThru
    $deadline = (Get-Date).AddSeconds(20)
    do { Start-Sleep -Milliseconds 50; $app.Refresh() } while (-not $app.HasExited -and $app.MainWindowHandle -eq 0 -and (Get-Date) -lt $deadline)
    if ($app.MainWindowHandle -eq 0) { throw 'Installed native app did not show a window' }
    if (-not $app.WaitForExit(15000)) { Stop-Process -Id $app.Id; throw 'Installed app did not honor bounded development run' }
    if ($app.ExitCode -ne 0) { throw "Installed app exit $($app.ExitCode): $([IO.File]::ReadAllText($startupError))" }
} finally {
    $uninstaller = Join-Path $installDir 'Uninstall NIP Native.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $uninstall = Start-Process -FilePath $uninstaller -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
        if ($uninstall.ExitCode -ne 0) { throw "Uninstaller exit $($uninstall.ExitCode)" }
        if (Test-Path -LiteralPath (Join-Path $installDir $exeName)) { throw 'Uninstall left the executable' }
        if (Test-Path -LiteralPath $reg) { throw 'Uninstall left the registration' }
        foreach ($link in $shortcuts) { if (Test-Path -LiteralPath $link) { throw 'Uninstall left a shortcut' } }
    } elseif (Test-Path -LiteralPath (Join-Path $installDir $exeName)) { throw 'Installed uninstaller missing' }
}
if ((Get-Content -LiteralPath $marker -Raw).Trim() -ne 'keep native user profile') { throw 'Profile marker changed' }
if ($oldHash -and (Get-FileHash -LiteralPath $realElectron -Algorithm SHA256).Hash -ne $oldHash) { throw 'Existing Electron installation changed' }
'PASS: native install, isolated executable/window, bounded launch, uninstall, profile retention, existing Electron unchanged.'
