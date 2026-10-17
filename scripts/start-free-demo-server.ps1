$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$port = 38241
$apiUrl = "http://127.0.0.1:$port"
$dataDirectory = Join-Path $env:LOCALAPPDATA "GhostChat\shared-server"
$cloudflaredDirectory = Join-Path $repoRoot "tools"
$cloudflaredPath = Join-Path $cloudflaredDirectory "cloudflared.exe"
$serverPath = Join-Path $repoRoot "target\release\ghostchat-server.exe"
$serverProcess = $null
$tunnelProcess = $null
$oldBind = $env:GHOSTCHAT_BIND
$oldDatabase = $env:GHOSTCHAT_DATABASE

function Test-GhostChatHealth {
    try {
        $health = Invoke-RestMethod -Uri "$apiUrl/health" -TimeoutSec 2
        return $health.status -eq "ok"
    }
    catch {
        return $false
    }
}

function Test-GhostChatVoiceSupport {
    try {
        Invoke-WebRequest -Uri "$apiUrl/v1/voice/ice" -TimeoutSec 3 -ErrorAction Stop | Out-Null
        return $true
    }
    catch {
        $response = $_.Exception.Response
        if (-not $response) { throw "Could not verify voice support on the running GhostChat server." }
        $status = [int]$response.StatusCode
        if ($status -eq 401 -or $status -eq 403) { return $true }
        if ($status -eq 404) { return $false }
        throw "Could not verify voice support on the running GhostChat server (HTTP $status)."
    }
}

function Install-LatestGhostChatServer {
    $assetName = "ghostchat-server-windows-x64.exe"
    $checksumName = "$assetName.sha256"
    $checksumPath = "$serverPath.sha256"
    $temporaryBinary = "$serverPath.download"
    $temporaryChecksum = "$checksumPath.download"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $serverPath) | Out-Null
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/crsh6663r4234/getchat/releases/latest" -Headers @{
            Accept = "application/vnd.github+json"
            "User-Agent" = "GhostChat-demo-server"
        } -TimeoutSec 20
        $binaryAsset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
        $hashAsset = $release.assets | Where-Object { $_.name -eq $checksumName } | Select-Object -First 1
        if (-not $binaryAsset -or -not $hashAsset) { throw "The latest GhostChat release does not contain the standalone server files." }
        foreach ($asset in @($binaryAsset, $hashAsset)) {
            if ($asset.browser_download_url -notmatch '^https://github\.com/crsh6663r4234/getchat/releases/download/') {
                throw "GitHub returned an unexpected release download address."
            }
        }
        Invoke-WebRequest -Uri $binaryAsset.browser_download_url -OutFile $temporaryBinary -TimeoutSec 120
        Invoke-WebRequest -Uri $hashAsset.browser_download_url -OutFile $temporaryChecksum -TimeoutSec 20
        $checksumText = Get-Content -LiteralPath $temporaryChecksum -Raw
        $match = [regex]::Match($checksumText, '(?im)^\s*([a-f0-9]{64})\s+\*?ghostchat-server-windows-x64\.exe\s*$')
        if (-not $match.Success) { throw "The standalone server checksum file is invalid." }
        $expectedHash = $match.Groups[1].Value.ToLowerInvariant()
        $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $temporaryBinary).Hash.ToLowerInvariant()
        if ($actualHash -ne $expectedHash) { throw "The standalone server download did not match its published SHA-256 checksum." }
        Move-Item -LiteralPath $temporaryBinary -Destination $serverPath -Force
        Move-Item -LiteralPath $temporaryChecksum -Destination $checksumPath -Force
    }
    finally {
        foreach ($temporaryPath in @($temporaryBinary, $temporaryChecksum)) {
            if (Test-Path -LiteralPath $temporaryPath) { Remove-Item -LiteralPath $temporaryPath -Force }
        }
    }
}

function Get-Cloudflared {
    $installed = Get-Command cloudflared.exe -ErrorAction SilentlyContinue
    if (-not $installed) {
        $installed = Get-Command cloudflared -ErrorAction SilentlyContinue
    }
    if ($installed) {
        if ($installed.Source) { return $installed.Source }
        return $installed.Path
    }

    New-Item -ItemType Directory -Force -Path $cloudflaredDirectory | Out-Null
    $cachedChecksumPath = "$cloudflaredPath.sha256"
    if (Test-Path -LiteralPath $cloudflaredPath) {
        if (-not (Test-Path -LiteralPath $cachedChecksumPath)) {
            throw "An unverified cloudflared.exe already exists in the project tools folder; it was left untouched. Install cloudflared from Cloudflare and run this script again."
        }
        $cachedExpectedHash = (Get-Content -LiteralPath $cachedChecksumPath -Raw).Trim().ToLowerInvariant()
        $cachedActualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $cloudflaredPath).Hash.ToLowerInvariant()
        if ($cachedExpectedHash -notmatch "^[a-f0-9]{64}$" -or $cachedActualHash -ne $cachedExpectedHash) {
            throw "The cached cloudflared.exe no longer matches its verified checksum; it was left untouched."
        }
        return $cloudflaredPath
    }

    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/cloudflare/cloudflared/releases/latest" -Headers @{ "User-Agent" = "GhostChat"; "Accept" = "application/vnd.github+json" }
    $assetName = "cloudflared-windows-amd64.exe"
    $asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
    $checksumPattern = "(?im)^\s*$([regex]::Escape($assetName)):\s*([a-f0-9]{64})\s*$"
    $checksumMatch = [regex]::Match([string]$release.body, $checksumPattern)
    if (-not $asset -or -not $checksumMatch.Success) {
        throw "Could not find the official cloudflared download and checksum. Install it from https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/ and run this script again."
    }

    $temporaryDownload = Join-Path $cloudflaredDirectory ("cloudflared-{0}.part" -f [guid]::NewGuid().ToString("N"))
    try {
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $temporaryDownload
        $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $temporaryDownload).Hash.ToLowerInvariant()
        $expectedHash = $checksumMatch.Groups[1].Value.ToLowerInvariant()
        if ($actualHash -ne $expectedHash) {
            throw "The cloudflared download did not match Cloudflare's published SHA-256 checksum. The file was rejected."
        }
        Move-Item -LiteralPath $temporaryDownload -Destination $cloudflaredPath -Force | Out-Null
        Set-Content -LiteralPath $cachedChecksumPath -Value $expectedHash -NoNewline
    }
    finally {
        if (Test-Path -LiteralPath $temporaryDownload) {
            Remove-Item -LiteralPath $temporaryDownload -Force
        }
    }

    return $cloudflaredPath
}

try {
    $cloudflared = Get-Cloudflared
    $configPaths = @(
        (Join-Path $env:USERPROFILE ".cloudflared\config.yml"),
        (Join-Path $env:USERPROFILE ".cloudflared\config.yaml")
    )
    if ($configPaths | Where-Object { Test-Path -LiteralPath $_ }) {
        throw "A cloudflared config.yml/config.yaml already exists. Quick Tunnels do not support that config; it was left untouched."
    }

    $serverHealthy = Test-GhostChatHealth
    if ($serverHealthy -and -not (Test-GhostChatVoiceSupport)) {
        throw "An older GhostChat server is still running on port $port. Stop the PowerShell window that started it with Ctrl+C, then run this script again. The script will download the latest server and reuse its existing database at $dataDirectory."
    }

    if (-not $serverHealthy) {
        Install-LatestGhostChatServer
        if (-not (Test-Path -LiteralPath $serverPath)) {
            throw "The server binary was not created at $serverPath."
        }

        New-Item -ItemType Directory -Force -Path $dataDirectory | Out-Null
        $env:GHOSTCHAT_BIND = "127.0.0.1:$port"
        $env:GHOSTCHAT_DATABASE = Join-Path $dataDirectory "ghostchat.sqlite"
        $stdoutLog = Join-Path $dataDirectory "server-stdout.log"
        $stderrLog = Join-Path $dataDirectory "server-stderr.log"
        $serverProcess = Start-Process -FilePath $serverPath -WorkingDirectory $dataDirectory -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutLog -RedirectStandardError $stderrLog

        $ready = $false
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            if (Test-GhostChatHealth) { $ready = $true; break }
            if ($serverProcess.HasExited) { break }
            Start-Sleep -Seconds 1
        }
        if (-not $ready) {
            if (Test-Path -LiteralPath $stderrLog) {
                Get-Content -LiteralPath $stderrLog -Tail 20 | ForEach-Object { Write-Host $_ }
            }
            throw "GhostChat did not become healthy on port $port."
        }
    }

    Write-Host "GhostChat is healthy at $apiUrl. Starting a temporary public tunnel."
    Write-Host "Keep this window and the host PC online. Press Ctrl+C to stop the tunnel."
    Write-Host "The tunnel URL will be copied automatically; paste it in Settings > Server address on each client."
    Write-Host ""

    New-Item -ItemType Directory -Force -Path $dataDirectory | Out-Null
    $tunnelLogBase = Join-Path $dataDirectory ("tunnel-{0}" -f [guid]::NewGuid().ToString("N"))
    $tunnelStdout = "$tunnelLogBase-stdout.log"
    $tunnelStderr = "$tunnelLogBase-stderr.log"
    $tunnelProcess = Start-Process -FilePath $cloudflared -ArgumentList @("tunnel", "--no-autoupdate", "--url", $apiUrl) -WorkingDirectory $dataDirectory -WindowStyle Hidden -PassThru -RedirectStandardOutput $tunnelStdout -RedirectStandardError $tunnelStderr

    $publicUrl = $null
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $publicUrl -and -not $tunnelProcess.HasExited -and (Get-Date) -lt $deadline) {
        foreach ($logPath in @($tunnelStdout, $tunnelStderr)) {
            if (Test-Path -LiteralPath $logPath) {
                $logText = Get-Content -LiteralPath $logPath -Raw -ErrorAction SilentlyContinue
                if ($logText -match "https://[a-z0-9-]+\.trycloudflare\.com") {
                    $publicUrl = $Matches[0]
                    break
                }
            }
        }
        if (-not $publicUrl) { Start-Sleep -Milliseconds 500 }
    }

    if (-not $publicUrl) {
        foreach ($logPath in @($tunnelStdout, $tunnelStderr)) {
            if (Test-Path -LiteralPath $logPath) {
                Get-Content -LiteralPath $logPath -Tail 30 | ForEach-Object { Write-Host $_ }
            }
        }
        throw "Cloudflare did not provide a public tunnel URL within 60 seconds."
    }

    try { Set-Clipboard -Value $publicUrl } catch { }
    Write-Host "GhostChat public URL: $publicUrl"
    Write-Host "URL copied to clipboard. Anyone with it can try to register a chat account."
    Write-Host "Keep this window and the host PC online. Press Ctrl+C to stop the tunnel."
    while (-not $tunnelProcess.HasExited) {
        Start-Sleep -Seconds 2
        $tunnelProcess.Refresh()
    }
}
finally {
    $env:GHOSTCHAT_BIND = $oldBind
    $env:GHOSTCHAT_DATABASE = $oldDatabase
    if ($serverProcess -and -not $serverProcess.HasExited) {
        Stop-Process -Id $serverProcess.Id -ErrorAction SilentlyContinue
    }
    if ($tunnelProcess -and -not $tunnelProcess.HasExited) {
        Stop-Process -Id $tunnelProcess.Id -ErrorAction SilentlyContinue
    }
}
