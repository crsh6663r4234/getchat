//! The event WebSocket (/v1/ws): session, typing, voice and watch-party signal relay.

use super::*;

#[derive(Deserialize)]
pub(crate) struct WsAuth {
    pub(crate) token: String,
}

pub(crate) async fn websocket(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(32 * 1024)
        .max_frame_size(32 * 1024)
        .on_upgrade(move |socket| websocket_session(socket, state))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WsVoiceSignal {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) conversation_id: String,
    pub(crate) to_user_id: String,
    pub(crate) kind: String,
    pub(crate) payload: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WsTypingSignal {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) conversation_id: String,
    pub(crate) active: bool,
}

pub(crate) async fn relay_typing_signal(state: &AppState, sender: &PublicUser, text: &str) {
    let Ok(signal) = serde_json::from_str::<WsTypingSignal>(text) else {
        return;
    };
    if signal.message_type != "typing" || signal.conversation_id.len() > 64 {
        return;
    }
    let rate_key = format!("typing:{}", sender.id);
    if !allow_rate(state, &rate_key, 16, Duration::from_secs(10))
        || !is_member(&state.db, &signal.conversation_id, &sender.id)
            .await
            .unwrap_or(false)
    {
        return;
    }
    // A blocked direct pair exchanges no typing indicators.
    if ensure_direct_not_blocked(&state.db, &signal.conversation_id, &sender.id)
        .await
        .is_err()
    {
        return;
    }
    let recipients = match sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? AND user_id<>?",
    )
    .bind(&signal.conversation_id)
    .bind(&sender.id)
    .fetch_all(&state.db)
    .await
    {
        Ok(recipients) => recipients,
        Err(_) => return,
    };
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({
            "type": "typing",
            "conversation_id": signal.conversation_id,
            "user_id": sender.id,
            "display_name": sender.display_name,
            "active": signal.active,
        }),
    });
}

// Watch Party carries playback state, never media bytes. The server validates
// the shape and relays it through the same authenticated peer/member checks as
// call setup; it must never fetch the client-provided URL.
pub(crate) fn valid_watch_party_payload(payload: &serde_json::Value) -> bool {
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    let Some(object) = payload.as_object() else {
        return false;
    };
    if object.len() != 6 {
        return false;
    }
    let valid_source = payload["source"].as_str().is_some_and(|source| {
        source.is_empty()
            || (source.len() <= 2048
                && reqwest::Url::parse(source).is_ok_and(|url| {
                    url.scheme() == "https"
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                }))
    });
    valid_source
        && payload["position"]
            .as_f64()
            .is_some_and(|value| (0.0..=86_400.0).contains(&value))
        && payload["playing"].as_bool().is_some()
        && payload["revision"]
            .as_u64()
            .is_some_and(|value| value <= 1_000_000_000)
        && payload["sentAt"]
            .as_u64()
            .is_some_and(|value| value <= MAX_SAFE_INTEGER)
        && payload["action"].as_str().is_some_and(|value| {
            matches!(
                value,
                "source" | "play" | "pause" | "seek" | "request" | "state"
            )
        })
}

pub(crate) async fn relay_voice_signal(state: &AppState, sender: &PublicUser, text: &str) {
    let Ok(signal) = serde_json::from_str::<WsVoiceSignal>(text) else {
        return;
    };
    if signal.message_type != "voice_signal"
        || signal.conversation_id.len() > 64
        || signal.to_user_id.len() > 64
    {
        return;
    }

    let valid_payload = match signal.kind.as_str() {
        "offer" | "answer" => {
            signal.payload["type"].as_str() == Some(signal.kind.as_str())
                && signal.payload["sdp"]
                    .as_str()
                    .is_some_and(|sdp| !sdp.is_empty() && sdp.len() <= 16 * 1024)
        }
        "ice" => signal.payload["candidate"]
            .as_str()
            .is_some_and(|candidate| !candidate.is_empty() && candidate.len() <= 4 * 1024),
        "hangup" | "screen_stopped" | "ghostshare_started" => signal.payload.is_null(),
        "watch_party" => valid_watch_party_payload(&signal.payload),
        // The side that falls back to the server media relay tells the
        // other side to switch too, so both ends use the same path.
        "media_path" => signal.payload == serde_json::json!({ "path": "server" }),
        // Notes are call-room state only: relay them, but never persist them
        // in the chat history or account data.
        "call_note" => signal.payload["text"]
            .as_str()
            .is_some_and(|value| value.len() <= 1_200),
        _ => false,
    };
    if !valid_payload {
        return;
    }

    let peer_id = match sqlx::query_scalar::<_, String>(
        "SELECT member.user_id FROM conversation_members member JOIN conversations c ON c.id=member.conversation_id WHERE member.conversation_id=? AND member.user_id<>? AND c.kind='direct' AND EXISTS (SELECT 1 FROM conversation_members sender WHERE sender.conversation_id=c.id AND sender.user_id=?) LIMIT 1",
    )
    .bind(&signal.conversation_id)
    .bind(&sender.id)
    .bind(&sender.id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(peer_id)) if peer_id == signal.to_user_id => peer_id,
        _ => return,
    };
    let blocked = is_blocked_pair(&state.db, &sender.id, &peer_id)
        .await
        .unwrap_or(true);
    if blocked && signal.kind != "hangup" {
        return;
    }

    let rate_key = format!("voice:{}:{peer_id}", sender.id);
    if !allow_rate(state, &rate_key, 100, Duration::from_secs(10)) {
        return;
    }

    let _ = state.events.send(ServerEvent {
        recipients: vec![peer_id],
        event: serde_json::json!({
            "type": "voice_signal",
            "conversation_id": signal.conversation_id,
            "from_user_id": sender.id,
            "kind": signal.kind,
            "payload": signal.payload,
        }),
    });
}

pub(crate) fn session_revocation_applies(
    event: &serde_json::Value,
    current_session_hash: &str,
) -> bool {
    if let Some(only_session_hash) = event["session_hash"].as_str() {
        return only_session_hash == current_session_hash;
    }
    event["except_session_hash"].as_str() != Some(current_session_hash)
}

pub(crate) async fn websocket_session(mut socket: WebSocket, state: AppState) {
    let first = tokio::time::timeout(Duration::from_secs(8), socket.recv()).await;
    let token = match first {
        Ok(Some(Ok(WsMessage::Text(text)))) => serde_json::from_str::<WsAuth>(&text)
            .ok()
            .map(|auth| auth.token),
        _ => None,
    };
    let Some(token) = token else {
        let _ = socket.close().await;
        return;
    };
    let current_session_hash = session_hash(&token);
    let user = match sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
        .bind(&current_session_hash).fetch_optional(&state.db).await { Ok(Some(user)) => user, _ => { let _ = socket.close().await; return; } };
    // Subscribe before announcing "ready": an event published right after
    // the client sees "ready" (for example a logout) must not be missed.
    let mut events = state.events.subscribe();
    if socket
        .send(WsMessage::Text(
            serde_json::json!({"type":"ready"}).to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }
    if register_online_connection(&state, &user.id) {
        broadcast_current_presence(&state, &user.id).await;
    }
    let mut session_check = tokio::time::interval(Duration::from_secs(15));
    session_check.tick().await;
    let mut keepalive = tokio::time::interval(state.keepalive.interval);
    keepalive.tick().await;
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if last_heard.elapsed() >= state.keepalive.timeout
                    || socket.send(WsMessage::Ping(Vec::new())).await.is_err()
                {
                    break;
                }
            }
            _ = session_check.tick() => {
                let active = sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE token_hash=? AND datetime(expires_at)>datetime('now'))",
                )
                .bind(&current_session_hash)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
                if active == 0 {
                    let _ = socket.send(WsMessage::Text(serde_json::json!({"type":"session_revoked"}).to_string())).await;
                    let _ = socket.send(WsMessage::Close(None)).await;
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(WsMessage::Close(_))) => break,
                None | Some(Err(_)) => break,
                Some(Ok(WsMessage::Pong(_))) => last_heard = Instant::now(),
                Some(Ok(WsMessage::Ping(bytes))) => {
                    last_heard = Instant::now();
                    if socket.send(WsMessage::Pong(bytes)).await.is_err() { break; }
                },
                Some(Ok(WsMessage::Text(text))) => {
                    last_heard = Instant::now();
                    relay_typing_signal(&state, &user, &text).await;
                    relay_voice_signal(&state, &user, &text).await;
                },
                _ => {}
            },
            event = events.recv() => match event {
                Ok(event) if event.recipients.iter().any(|id| id == &user.id) && event.event["type"] == "session_revoked" => {
                    if session_revocation_applies(&event.event, &current_session_hash) {
                        let _ = socket.send(WsMessage::Text(serde_json::json!({"type":"session_revoked"}).to_string())).await;
                        let _ = socket.send(WsMessage::Close(None)).await;
                        break;
                    }
                }
                Ok(event) if event.recipients.iter().any(|id| id == &user.id) => {
                    if socket.send(WsMessage::Text(event.event.to_string())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => { let _ = socket.send(WsMessage::Text(serde_json::json!({"type":"refresh"}).to_string())).await; }
                _ => {}
            }
        }
    }
    let _ = socket.close().await;
    if unregister_online_connection(&state, &user.id) {
        broadcast_current_presence(&state, &user.id).await;
    }
}
