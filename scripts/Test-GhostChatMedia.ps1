# Run on the Windows VDS. Never prints API keys or secrets.
[CmdletBinding()]
param([string]$LiveKitExe = 'C:\GhostChatMedia\livekit-server.exe')
$ErrorActionPreference = 'Stop'
$results = @()
foreach ($name in @('GHOSTCHAT_LIVEKIT_URL', 'GHOSTCHAT_LIVEKIT_API_KEY', 'GHOSTCHAT_LIVEKIT_API_SECRET')) {
    $value = [Environment]::GetEnvironmentVariable($name, 'Process')
    if ([string]::IsNullOrWhiteSpace($value)) { $value = [Environment]::GetEnvironmentVariable($name, 'Machine') }
    $present = -not [string]::IsNullOrWhiteSpace($value)
    $results += [pscustomobject]@{ Check = $name; Result = $(if ($present) { 'SET' } else { 'MISSING' }) }
    if ($name -eq 'GHOSTCHAT_LIVEKIT_URL' -and $present) {
        $endpoint = $null
        if ([Uri]::TryCreate($value, [UriKind]::Absolute, [ref]$endpoint) -and $endpoint.Scheme -eq 'wss') {
            try {
                $reachable = Test-NetConnection -ComputerName $endpoint.DnsSafeHost -Port $endpoint.Port -InformationLevel Quiet -WarningAction SilentlyContinue
                $results += [pscustomobject]@{ Check = 'LiveKit TLS port'; Result = $(if ($reachable) { 'REACHABLE (TCP only)' } else { 'UNREACHABLE' }) }
            } catch { $results += [pscustomobject]@{ Check = 'LiveKit TLS port'; Result = 'CHECK FAILED' } }
        } else { $results += [pscustomobject]@{ Check = 'LiveKit URL format'; Result = 'INVALID: expected wss://host' } }
    }
}
if (Test-Path -LiteralPath $LiveKitExe -PathType Leaf) {
    try {
        $versionOutput = & $LiveKitExe --version 2>&1
        if ($LASTEXITCODE -eq 0) {
            $results += [pscustomobject]@{ Check = 'LiveKit executable'; Result = 'RUNS' }
        } else { $results += [pscustomobject]@{ Check = 'LiveKit executable'; Result = 'FAILED TO RUN' } }
    } catch { $results += [pscustomobject]@{ Check = 'LiveKit executable'; Result = 'FAILED TO RUN ON THIS WINDOWS' } }
} else { $results += [pscustomobject]@{ Check = 'LiveKit executable'; Result = 'NOT FOUND (fine if hosted elsewhere)' } }
$results | Format-Table -AutoSize
Write-Host 'TCP reachability does not prove TLS, authentication, UDP or a working call.'
Write-Host 'Restart GhostChat server after changing environment settings. Do not share keys or secrets.'
