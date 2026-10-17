param(
    [string]$DesktopExecutable = (Join-Path $PSScriptRoot '..\target\release\ghostchat-desktop.exe'),
    [string]$ServerExecutable = (Join-Path $PSScriptRoot '..\target\release\ghostchat-server.exe'),
    [int]$SampleSeconds = 15
)

$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class MemoryInfo {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Auto)]
  public struct MEMORYSTATUSEX { public uint dwLength; public uint dwMemoryLoad; public ulong ullTotalPhys; public ulong ullAvailPhys; public ulong ullTotalPageFile; public ulong ullAvailPageFile; public ulong ullTotalVirtual; public ulong ullAvailVirtual; public ulong ullAvailExtendedVirtual; }
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX lpBuffer);
  public static MEMORYSTATUSEX Read() { var value = new MEMORYSTATUSEX(); value.dwLength=(uint)Marshal.SizeOf(typeof(MEMORYSTATUSEX)); if(!GlobalMemoryStatusEx(ref value)) throw new Exception("GlobalMemoryStatusEx failed"); return value; }
}
'@
$desktop = (Resolve-Path $DesktopExecutable).Path
$server = (Resolve-Path $ServerExecutable).Path
$oldBind = $env:GHOSTCHAT_BIND
$oldDatabase = $env:GHOSTCHAT_DATABASE
$runId = [guid]::NewGuid().ToString('N')
$database = Join-Path $env:TEMP "ghostchat-performance-$runId.sqlite"
$stdout = Join-Path $env:TEMP "ghostchat-performance-$runId.out.log"
$stderr = Join-Path $env:TEMP "ghostchat-performance-$runId.err.log"
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()
$serverProcess = $null
$desktopProcess = $null
$webViewBefore = @((Get-Process -Name 'msedgewebview2' -ErrorAction SilentlyContinue).Id)

function Get-Processes([int[]]$Ids) {
    $results = @()
    foreach ($id in ($Ids | Select-Object -Unique)) {
        try { $results += Get-Process -Id $id -ErrorAction Stop } catch {}
    }
    return $results
}

try {
    $env:GHOSTCHAT_BIND = "127.0.0.1:$port"
    $env:GHOSTCHAT_DATABASE = $database
    $serverProcess = Start-Process -FilePath $server -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $health = $false
    for ($i = 0; $i -lt 40; $i++) {
        if ($serverProcess.HasExited) { throw "Standalone server exited. See $stderr" }
        try { if ((Invoke-RestMethod -Uri "http://127.0.0.1:$port/health" -TimeoutSec 2).status -eq 'ok') { $health = $true; break } } catch {}
        Start-Sleep -Milliseconds 250
    }
    if (-not $health) { throw "Standalone server did not become ready. See $stderr" }

    $startup = [System.Diagnostics.Stopwatch]::StartNew()
    $desktopProcess = Start-Process -FilePath $desktop -PassThru
    for ($i = 0; $i -lt 300; $i++) {
        $desktopProcess.Refresh()
        if ($desktopProcess.HasExited) { throw "Desktop app exited with code $($desktopProcess.ExitCode)." }
        if ($desktopProcess.MainWindowHandle -ne 0) { break }
        Start-Sleep -Milliseconds 100
    }
    $startup.Stop()
    if ($desktopProcess.MainWindowHandle -eq 0) { throw 'Desktop window did not appear within 30 seconds.' }
    Start-Sleep -Seconds 5

    $webViews = @(Get-Process -Name 'msedgewebview2' -ErrorAction SilentlyContinue | Where-Object { $_.Id -notin $webViewBefore })
    $desktopIds = @($desktopProcess.Id) + @($webViews.Id)
    $combinedIds = $desktopIds + @($serverProcess.Id)
    $before = Get-Processes $combinedIds
    $cpuBefore = @{}
    foreach ($process in $before) { $cpuBefore[$process.Id] = $process.CPU }
    $sampleStart = Get-Date
    Start-Sleep -Seconds $SampleSeconds
    $sampleEnd = Get-Date
    $after = Get-Processes $combinedIds
    $elapsed = ($sampleEnd - $sampleStart).TotalSeconds
    $logicalCpus = [math]::Max(1, [int]$env:NUMBER_OF_PROCESSORS)

    function Summarize([System.Diagnostics.Process[]]$Processes, [hashtable]$CpuStart, [double]$Seconds, [int]$LogicalCpus) {
        $working = 0L; $private = 0L; $cpuDelta = 0.0
        foreach ($process in $Processes) {
            $working += $process.WorkingSet64
            $private += $process.PrivateMemorySize64
            if ($CpuStart.ContainsKey($process.Id) -and $null -ne $process.CPU) { $cpuDelta += [math]::Max(0, $process.CPU - $CpuStart[$process.Id]) }
        }
        [pscustomobject]@{
            ProcessCount = $Processes.Count
            WorkingSetMB = [math]::Round($working / 1MB, 1)
            PrivateMemoryMB = [math]::Round($private / 1MB, 1)
            CpuTimeSeconds = [math]::Round($cpuDelta, 3)
            CpuPercentOfMachine = [math]::Round(($cpuDelta / $Seconds) * 100 / $LogicalCpus, 3)
        }
    }

    $desktopStats = Summarize (Get-Processes $desktopIds) $cpuBefore $elapsed $logicalCpus
    $serverStats = Summarize @((Get-Processes @($serverProcess.Id))) $cpuBefore $elapsed $logicalCpus
    $combinedStats = Summarize $after $cpuBefore $elapsed $logicalCpus
    $osRegistry = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SOFTWARE\Microsoft\Windows NT\CurrentVersion')
    $cpuRegistry = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('HARDWARE\DESCRIPTION\System\CentralProcessor\0')
    $osName = $osRegistry.GetValue('ProductName')
    $osBuild = $osRegistry.GetValue('CurrentBuild')
    $cpuModel = $cpuRegistry.GetValue('ProcessorNameString')
    $osRegistry.Dispose(); $cpuRegistry.Dispose()
    [pscustomobject]@{
        OS = $osName
        OSBuild = $osBuild
        CPU = $cpuModel
        LogicalProcessors = $logicalCpus
        InstalledMemoryGB = [math]::Round([MemoryInfo]::Read().ullTotalPhys / 1GB, 1)
        App = $desktopStats
        Server = $serverStats
        Combined = $combinedStats
        WindowStartupMilliseconds = [math]::Round($startup.Elapsed.TotalMilliseconds)
        WindowTitle = $desktopProcess.MainWindowTitle
        WindowWebView2Processes = $webViews.Count
        SampleSeconds = [math]::Round($elapsed, 1)
    } | ConvertTo-Json -Depth 5
}
finally {
    if ($desktopProcess -and -not $desktopProcess.HasExited) {
        $desktopProcess.CloseMainWindow() | Out-Null
        if (-not $desktopProcess.WaitForExit(5000)) { Stop-Process -Id $desktopProcess.Id -Force }
    }
    if ($serverProcess -and -not $serverProcess.HasExited) { Stop-Process -Id $serverProcess.Id -Force }
    $env:GHOSTCHAT_BIND = $oldBind
    $env:GHOSTCHAT_DATABASE = $oldDatabase
    foreach ($file in @($database, "$database-wal", "$database-shm", $stdout, $stderr)) { Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue }
}
