param(
    [Parameter(Mandatory = $true)]
    [string]$ServerExecutable
)

$ErrorActionPreference = 'Stop'
$api = $null
$process = $null
$socketA = $null
$socketB = $null
$guid = [guid]::NewGuid().ToString('N')
$dbPath = Join-Path $env:TEMP "ghostchat-$guid.sqlite"
$outPath = Join-Path $env:TEMP "ghostchat-$guid.out.log"
$errPath = Join-Path $env:TEMP "ghostchat-$guid.err.log"
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()
$oldBind = $env:GHOSTCHAT_BIND
$oldDb = $env:GHOSTCHAT_DATABASE
$env:GHOSTCHAT_BIND = "127.0.0.1:$port"
$env:GHOSTCHAT_DATABASE = $dbPath

function Post-Json([string]$Path, [object]$Body, [string]$Token) {
    $headers = @{}
    if ($Token) { $headers.Authorization = "Bearer $Token" }
    Invoke-RestMethod -Uri "$script:api$Path" -Method Post -ContentType 'application/json' -Headers $headers -Body ($Body | ConvertTo-Json -Depth 8 -Compress)
}

function Get-Json([string]$Path, [string]$Token) {
    Invoke-RestMethod -Uri "$script:api$Path" -Method Get -Headers @{ Authorization = "Bearer $Token" }
}

function Expect-Status([scriptblock]$Action, [int]$Status, [string]$Label) {
    try { & $Action | Out-Null; throw "${Label}: expected HTTP $Status but request succeeded." }
    catch {
        $actual = [int]$_.Exception.Response.StatusCode
        if ($actual -ne $Status) { throw "${Label}: expected HTTP $Status, received $actual. $($_.Exception.Message)" }
    }
    Write-Output "PASS $Label (HTTP $Status)"
}

function Open-WebSocket([string]$Token) {
    $socket = [System.Net.WebSockets.ClientWebSocket]::new()
    $null = $socket.ConnectAsync([uri]"ws://127.0.0.1:$port/v1/ws", [System.Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $auth = [System.Text.Encoding]::UTF8.GetBytes((@{ token = $Token } | ConvertTo-Json -Compress))
    $segment = [System.ArraySegment[byte]]::new($auth)
    $null = $socket.SendAsync($segment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, [System.Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $buffer = [byte[]]::new(4096)
    $receive = $socket.ReceiveAsync([System.ArraySegment[byte]]::new($buffer), [System.Threading.CancellationToken]::None)
    if (-not $receive.Wait(5000)) { throw 'WebSocket authentication timed out.' }
    $result = $receive.GetAwaiter().GetResult()
    $frame = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count) | ConvertFrom-Json
    if ($frame.type -ne 'ready') { throw 'WebSocket authentication was rejected.' }
    return $socket
}

try {
    $projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
    $process = Start-Process -FilePath $ServerExecutable -WorkingDirectory $projectRoot -PassThru -WindowStyle Hidden -RedirectStandardOutput $outPath -RedirectStandardError $errPath
    $api = "http://127.0.0.1:$port"
    $ready = $false
    for ($i = 0; $i -lt 40; $i++) {
        if ($process.HasExited) { throw "Server exited before becoming ready. See $errPath" }
        try { $health = Invoke-RestMethod "$api/health" -TimeoutSec 2; if ($health.status -eq 'ok') { $ready = $true; break } } catch {}
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) { throw "Server did not become ready. See $errPath" }
    Write-Output 'PASS server health'

    $accountA = Post-Json '/v1/accounts' @{ display_name = 'Maple'; username = "maple_$($guid.Substring(0, 6))" } $null
    $accountB = Post-Json '/v1/accounts' @{ display_name = 'River'; username = "river_$($guid.Substring(0, 6))" } $null
    $outsider = Post-Json '/v1/accounts' @{ display_name = 'Moss'; username = "moss_$($guid.Substring(0, 6))" } $null
    Write-Output 'PASS anonymous account creation'
    if ($accountA.recovery_token.Length -lt 32) { throw 'Recovery key was not generated.' }
    if ($accountA.user.id -eq $accountB.user.id) { throw 'User IDs must be unique.' }
    Write-Output 'PASS cryptographically unique account IDs and recovery key generation'

    $null = Post-Json '/v1/friends' @{ user_id = $accountB.user.id } $accountA.session_token
    $pending = Get-Json '/v1/friends/requests' $accountB.session_token
    if ($pending.Count -ne 1 -or $pending[0].user_id -ne $accountA.user.id) { throw 'Friend request was not delivered to the recipient.' }
    $null = Post-Json "/v1/friends/requests/$($pending[0].id)/accept" @{} $accountB.session_token
    if ((Get-Json '/v1/friends' $accountA.session_token).Count -ne 1) { throw 'Accepted friend relationship did not persist.' }
    Write-Output 'PASS friend request, acceptance, and relationship persistence'
    for ($i = 0; $i -lt 19; $i++) {
        try { Post-Json '/v1/friends' @{ user_id = $accountB.user.id } $accountA.session_token | Out-Null; throw 'Duplicate friend request unexpectedly succeeded.' }
        catch { if ([int]$_.Exception.Response.StatusCode -ne 409) { throw } }
    }
    Expect-Status { Post-Json '/v1/friends' @{ user_id = $accountB.user.id } $accountA.session_token } 429 'friend-request rate limit'

    $conversation = Post-Json '/v1/conversations/direct' @{ user_id = $accountB.user.id } $accountA.session_token
    $socketA = Open-WebSocket $accountA.session_token
    $socketB = Open-WebSocket $accountB.session_token
    Write-Output 'PASS authenticated WebSocket connections for two clients'

    $clientId = [guid]::NewGuid().ToString()
    $sent = Post-Json "/v1/conversations/$($conversation.id)/messages" @{ client_message_id = $clientId; content = 'Hello from the first client.' } $accountA.session_token
    $buffer = [byte[]]::new(8192)
    $receive = $socketB.ReceiveAsync([System.ArraySegment[byte]]::new($buffer), [System.Threading.CancellationToken]::None)
    if (-not $receive.Wait(5000)) { throw 'The second client did not receive the real-time message.' }
    $event = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $receive.GetAwaiter().GetResult().Count) | ConvertFrom-Json
    if ($event.type -ne 'message' -or $event.message.id -ne $sent.id) { throw 'Unexpected WebSocket message event.' }
    Write-Output 'PASS real-time message delivery from client A to client B'

    $duplicate = Post-Json "/v1/conversations/$($conversation.id)/messages" @{ client_message_id = $clientId; content = 'Hello from the first client.' } $accountA.session_token
    if ($duplicate.id -ne $sent.id) { throw 'Duplicate retry was not idempotent.' }
    $stored = Get-Json "/v1/conversations/$($conversation.id)/messages" $accountB.session_token
    if ($stored.Count -ne 1 -or $stored[0].content -ne 'Hello from the first client.') { throw 'Message persistence or duplicate protection failed.' }
    Write-Output 'PASS server ordering, persistence, and duplicate-delivery protection'

    $secondMessage = Post-Json "/v1/conversations/$($conversation.id)/messages" @{ client_message_id = [guid]::NewGuid().ToString(); content = 'Second message.' } $accountA.session_token
    $ordered = Get-Json "/v1/conversations/$($conversation.id)/messages" $accountB.session_token
    if ($ordered.Count -ne 2 -or $ordered[0].sequence -ge $ordered[1].sequence -or $ordered[0].content -ne 'Hello from the first client.') { throw 'Messages are not returned in server insertion order.' }
    $older = Get-Json "/v1/conversations/$($conversation.id)/messages?limit=1&before=$($ordered[1].sequence)" $accountB.session_token
    if ($older.Count -ne 1 -or $older[0].id -ne $sent.id) { throw 'Older-message pagination did not return the preceding message.' }
    Write-Output 'PASS stable server ordering and cursor pagination'

    Expect-Status { Post-Json "/v1/conversations/$($conversation.id)/messages" @{ client_message_id = [guid]::NewGuid().ToString(); content = 'x' * 4001 } $accountA.session_token } 400 'oversize message rejected'
    Expect-Status { Get-Json "/v1/conversations/$($conversation.id)/messages" $outsider.session_token } 404 'private conversation authorization'
    Expect-Status { Get-Json '/v1/me' 'invalid-token' } 401 'invalid session rejected'

    $oldSocket = $socketB
    $oldSocket.Abort()
    $oldSocket.Dispose()
    $socketB = Open-WebSocket $accountB.session_token
    if ((Get-Json "/v1/conversations/$($conversation.id)/messages" $accountB.session_token).Count -ne 2) { throw 'Messages did not persist across client reconnect.' }
    Write-Output 'PASS WebSocket reconnect and persisted history recovery'

    $edited = Invoke-RestMethod -Uri "$api/v1/messages/$($sent.id)" -Method Patch -ContentType 'application/json' -Headers @{ Authorization = "Bearer $($accountA.session_token)" } -Body (@{ content = 'Edited message content.' } | ConvertTo-Json -Compress)
    if ((Get-Json "/v1/conversations/$($conversation.id)/messages" $accountB.session_token)[0].content -ne 'Edited message content.') { throw 'Message edit did not persist.' }
    Write-Output 'PASS sender-authorized message editing'
    Expect-Status { Invoke-RestMethod -Uri "$api/v1/messages/$($sent.id)" -Method Delete -Headers @{ Authorization = "Bearer $($outsider.session_token)" } } 403 'message ownership authorization'
    $null = Invoke-RestMethod -Uri "$api/v1/messages/$($sent.id)" -Method Delete -Headers @{ Authorization = "Bearer $($accountA.session_token)" }
    $null = Invoke-RestMethod -Uri "$api/v1/messages/$($secondMessage.id)" -Method Delete -Headers @{ Authorization = "Bearer $($accountA.session_token)" }
    if ((Get-Json "/v1/conversations/$($conversation.id)/messages" $accountB.session_token).Count -ne 0) { throw 'Sender message deletion did not persist.' }
    Write-Output 'PASS sender-authorized message deletion'

    $recovered = Post-Json '/v1/sessions/recover' @{ recovery_token = $accountA.recovery_token } $null
    Expect-Status { Get-Json '/v1/me' $accountA.session_token } 401 'recovery revokes the previous session'
    if ((Get-Json '/v1/me' $recovered.session_token).id -ne $accountA.user.id) { throw 'Recovery did not restore the same account.' }
    Write-Output 'PASS recovery token verification, session rotation, and account recovery'
    $revoked = $false
    $revocationBuffer = [byte[]]::new(8192)
    for ($i = 0; $i -lt 8; $i++) {
        $revocationTask = $socketA.ReceiveAsync([System.ArraySegment[byte]]::new($revocationBuffer), [System.Threading.CancellationToken]::None)
        if (-not $revocationTask.Wait(5000)) { break }
        $frame = $revocationTask.GetAwaiter().GetResult()
        if ($frame.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { $revoked = $true; break }
        $event = [System.Text.Encoding]::UTF8.GetString($revocationBuffer, 0, $frame.Count) | ConvertFrom-Json
        if ($event.type -eq 'session_revoked') { $revoked = $true; break }
    }
    if (-not $revoked) { throw 'The active WebSocket was not closed after session recovery.' }
    Write-Output 'PASS active WebSocket closes immediately after session revocation'

    $export = Get-Json '/v1/data/export' $recovered.session_token
    if ($export.account.id -ne $accountA.user.id -or $export.conversations.Count -ne 1) { throw 'Account export did not include the user profile and conversation.' }
    Write-Output 'PASS authenticated account data export'
    $null = Invoke-RestMethod -Uri "$api/v1/me/account" -Method Delete -Headers @{ Authorization = "Bearer $($recovered.session_token)" }
    Expect-Status { Get-Json '/v1/me' $recovered.session_token } 401 'account deletion revokes session'
    if ((Get-Json '/v1/conversations' $accountB.session_token).Count -ne 0) { throw 'Account deletion did not remove the conversation data.' }
    Write-Output 'PASS account and associated conversation data deletion'

    $headers = @{ Authorization = "Bearer $($accountB.session_token)" }
    $null = Invoke-RestMethod -Uri "$api/v1/sessions" -Method Delete -Headers $headers
    Expect-Status { Get-Json '/v1/me' $accountB.session_token } 401 'logout revokes the session'
    Write-Output 'PASS session logout and revocation'
}
finally {
    if ($socketA) { try { $socketA.Dispose() } catch {} }
    if ($socketB) { try { $socketB.Dispose() } catch {} }
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force; $process.WaitForExit() }
    $env:GHOSTCHAT_BIND = $oldBind
    $env:GHOSTCHAT_DATABASE = $oldDb
    foreach ($file in @($dbPath, "$dbPath-wal", "$dbPath-shm", $outPath, $errPath)) { Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue }
}
