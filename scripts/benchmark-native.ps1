param(
    [string]$NativeExe = "native/target/x86_64-pc-windows-msvc/release/nip-native.exe",
    [string]$ElectronExe = "$env:LOCALAPPDATA/Programs/nip-desktop/NIP.exe",
    [int]$Seconds = 10
)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path $PSScriptRoot -Parent
$results = @()
function Get-RunProcesses([int]$RootId, [datetime]$Started) {
    $all = @(Get-CimInstance Win32_Process)
    $ids = [Collections.Generic.HashSet[int]]::new()
    [void]$ids.Add($RootId)
    do {
        $added = $false
        foreach ($item in $all) {
            if ($item.CreationDate -ge $Started.AddSeconds(-1) -and $ids.Contains([int]$item.ParentProcessId)) {
                if ($ids.Add([int]$item.ProcessId)) { $added = $true }
            }
        }
    } while ($added)
    @($ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
}
foreach ($kind in @('electron', 'native')) {
    $profile = Join-Path $workspace ('work/benchmark-' + $kind + '-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $profile -Force | Out-Null
    $exe = if ($kind -eq 'native') { (Resolve-Path $NativeExe).Path } else { (Resolve-Path $ElectronExe).Path }
    $start = Get-Date
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    $oldNode = $env:ELECTRON_RUN_AS_NODE
    $oldData = $env:NIP_DATA_DIR
    $oldSynthetic = $env:NIP_NATIVE_SYNTHETIC
    try {
        Remove-Item Env:ELECTRON_RUN_AS_NODE -ErrorAction SilentlyContinue
        Remove-Item Env:NIP_NATIVE_SYNTHETIC -ErrorAction SilentlyContinue
        if ($kind -eq 'native') {
            $process = Start-Process -FilePath $exe -ArgumentList @('--data-dir', "`"$profile`"", '--local-only') -PassThru
        } else {
            $env:NIP_DATA_DIR = $profile
            $process = Start-Process -FilePath $exe -PassThru
        }
        $deadline = (Get-Date).AddSeconds(30)
        do {
            Start-Sleep -Milliseconds 50
            $process.Refresh()
            if ($process.HasExited) { throw "$kind exited before showing a window" }
        } while ($process.MainWindowHandle -eq 0 -and (Get-Date) -lt $deadline)
        if ($process.MainWindowHandle -eq 0) { throw "$kind did not show a window" }
        $readyMs = $stopwatch.ElapsedMilliseconds
        Start-Sleep -Seconds 3
        $before = @(Get-RunProcesses $process.Id $start)
        $cpuBefore = ($before | Measure-Object CPU -Sum).Sum
        $sample = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds $Seconds
        $after = @(Get-RunProcesses $process.Id $start)
        $elapsed = $sample.Elapsed.TotalSeconds
        $cpu = (($after | Measure-Object CPU -Sum).Sum - $cpuBefore) / $elapsed * 100
        $results += [pscustomobject]@{
            app = $kind; executable = $exe; scenario = 'fresh profile, home window, no server or audio'
            processes = $after.Count; window_visible_ms = $readyMs; sample_seconds = [math]::Round($elapsed, 3)
            working_set_total_mib = [math]::Round(($after | Measure-Object WorkingSet64 -Sum).Sum / 1MB, 2)
            private_bytes_total_mib = [math]::Round(($after | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB, 2)
            cpu_one_core_percent = [math]::Round($cpu, 3)
            cpu_machine_percent = [math]::Round($cpu / [Environment]::ProcessorCount, 3)
            logical_processors = [Environment]::ProcessorCount
        }
    } finally {
        $env:ELECTRON_RUN_AS_NODE = $oldNode
        $env:NIP_DATA_DIR = $oldData
        $env:NIP_NATIVE_SYNTHETIC = $oldSynthetic
        if ($process) {
            Get-RunProcesses $process.Id $start | Where-Object StartTime -ge $start.AddSeconds(-1) | Stop-Process -ErrorAction SilentlyContinue
        }
        $process = $null
    }
}
$out = Join-Path $workspace 'work/native-benchmark.json'
$results | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 -LiteralPath $out
$results | ConvertTo-Json -Depth 4
Write-Output "Saved $out. Working set is summed across descendants and may double-count shared pages; private bytes are reported separately. Startup means first native window handle, not interaction or audio latency."
