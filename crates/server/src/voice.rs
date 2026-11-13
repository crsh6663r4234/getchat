//! Voice and media signalling credentials: ICE servers, LiveKit tokens and room administration.

use super::*;

pub(crate) const TURN_CREDENTIAL_TTL_SECONDS: u64 = 12 * 60 * 60;

#[derive(Serialize)]
pub(crate) struct IceServer {
    pub(crate) urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) credential: Option<String>,
}

pub(crate) async fn voice_ice_servers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mut ice_servers = vec![IceServer {
        urls: state.voice.stun_urls.clone(),
        username: None,
        credential: None,
    }];

    let turn_available =
        !state.voice.turn_urls.is_empty() && state.voice.turn_shared_secret.is_some();
    if !state.voice.turn_urls.is_empty() {
        if let Some(secret) = state.voice.turn_shared_secret.as_deref() {
            let expires_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_add(TURN_CREDENTIAL_TTL_SECONDS);
            let username = format!("{expires_at}:{}", user.id);
            let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes())
                .map_err(|_| ApiError::internal())?;
            mac.update(username.as_bytes());
            ice_servers.push(IceServer {
                urls: state.voice.turn_urls.clone(),
                username: Some(username),
                credential: Some(STANDARD.encode(mac.finalize().into_bytes())),
            });
        }
    }

    Ok(Json(serde_json::json!({
        "ice_servers": ice_servers,
        "turn_available": turn_available,
        // Calls can always fall back to relaying media through this server.
        "media_relay": true,
        "group_voice_available": state.voice.livekit_url.is_some()
            && state.voice.livekit_api_key.is_some()
            && state.voice.livekit_api_secret.is_some()
    })))
}

pub(crate) async fn group_voice_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let participant = sqlx::query_as::<_, (String, String)>(
        "SELECT u.display_name,c.kind FROM conversations c JOIN conversation_members cm ON cm.conversation_id=c.id JOIN users u ON u.id=cm.user_id WHERE c.id=? AND cm.user_id=?",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if participant.1 != "group" {
        return Err(ApiError::not_found());
    }
    media_revocation::admit(&state, &conversation_id).await?;
    let api_key =
        state.voice.livekit_api_key.as_deref().ok_or_else(|| {
            ApiError::unavailable("Group voice is not configured on this server.")
        })?;
    let api_secret =
        state.voice.livekit_api_secret.as_deref().ok_or_else(|| {
            ApiError::unavailable("Group voice is not configured on this server.")
        })?;
    let server_url =
        state.voice.livekit_url.as_deref().ok_or_else(|| {
            ApiError::unavailable("Group voice is not configured on this server.")
        })?;
    if !allow_rate(
        &state,
        &format!("livekit-token:{}", user.id),
        12,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before requesting another group voice token.",
        ));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let identity = format!("{}-{}", user.id, Uuid::new_v4().simple());
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let metadata = serde_json::json!({"account_id": user.id}).to_string();
    let claims = serde_json::json!({
        "iss": api_key,
        "sub": identity,
        "name": participant.0,
        "metadata": metadata,
        "iat": now,
        "nbf": now.saturating_sub(5),
        "exp": now.saturating_add(media_revocation::JOIN_TTL),
        "video": {
            "roomJoin": true,
            "room": conversation_id,
            "canPublish": true,
            "canPublishSources": ["microphone", "screen_share", "screen_share_audio"],
            "canSubscribe": true,
            "canPublishData": false
        }
    });
    let payload =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| ApiError::internal())?);
    let signing_input = format!("{header}.{payload}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_secret.as_bytes()).map_err(|_| ApiError::internal())?;
    mac.update(signing_input.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let token = format!("{signing_input}.{signature}");

    Ok(Json(serde_json::json!({
        "url": server_url,
        "token": token,
        "room": conversation_id,
        "expires_at": now.saturating_add(media_revocation::JOIN_TTL)
    })))
}

// GhostShare uses a separate LiveKit room for one-to-one screen sharing. Audio
// remains on the existing direct-call path; separating the screen publisher lets
// the SFU adapt layers without renegotiating or destabilising the call itself.
pub(crate) async fn ghost_share_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let participant = sqlx::query_as::<_, (String, String)>(
        "SELECT u.display_name,c.kind FROM conversations c JOIN conversation_members cm ON cm.conversation_id=c.id JOIN users u ON u.id=cm.user_id WHERE c.id=? AND cm.user_id=?",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if participant.1 != "direct" {
        return Err(ApiError::not_found());
    }
    // A block forbids GhostShare like it forbids call signaling.
    ensure_direct_not_blocked(&state.db, &conversation_id, &user.id).await?;
    media_revocation::admit(&state, &conversation_id).await?;
    let api_key = state.voice.livekit_api_key.as_deref().ok_or_else(|| {
        ApiError::unavailable("GhostShare needs a configured LiveKit media server.")
    })?;
    let api_secret = state.voice.livekit_api_secret.as_deref().ok_or_else(|| {
        ApiError::unavailable("GhostShare needs a configured LiveKit media server.")
    })?;
    let server_url = state.voice.livekit_url.as_deref().ok_or_else(|| {
        ApiError::unavailable("GhostShare needs a configured LiveKit media server.")
    })?;
    if !allow_rate(
        &state,
        &format!("ghostshare-token:{}", user.id),
        12,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before requesting another GhostShare token.",
        ));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let room = format!("ghostshare-{conversation_id}");
    let identity = format!("{}-{}", user.id, Uuid::new_v4().simple());
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let metadata = serde_json::json!({"account_id": user.id, "purpose": "ghostshare"}).to_string();
    let claims = serde_json::json!({
        "iss": api_key, "sub": identity, "name": participant.0, "metadata": metadata,
        "iat": now, "nbf": now.saturating_sub(5), "exp": now.saturating_add(media_revocation::JOIN_TTL),
        "video": {"roomJoin": true, "room": room, "canPublish": true,
          "canPublishSources": ["screen_share", "screen_share_audio"], "canSubscribe": true, "canPublishData": false}
    });
    let payload =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| ApiError::internal())?);
    let signing_input = format!("{header}.{payload}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_secret.as_bytes()).map_err(|_| ApiError::internal())?;
    mac.update(signing_input.as_bytes());
    let token = format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    Ok(Json(
        serde_json::json!({"url": server_url, "token": token, "room": room, "expires_at": now.saturating_add(media_revocation::JOIN_TTL)}),
    ))
}

pub(crate) async fn remove_livekit_participant(
    state: &AppState,
    conversation_id: &str,
    account_id: &str,
) -> Result<(), String> {
    let (Some(server_url), Some(api_key), Some(api_secret)) = (
        state.voice.livekit_url.as_deref(),
        state.voice.livekit_api_key.as_deref(),
        state.voice.livekit_api_secret.as_deref(),
    ) else {
        return Ok(());
    };
    let api_base = server_url
        .strip_prefix("wss://")
        .map(|host| format!("https://{host}"))
        .or_else(|| {
            server_url
                .strip_prefix("ws://")
                .map(|host| format!("http://{host}"))
        })
        .ok_or_else(|| "LiveKit server URL has an unsupported scheme".to_owned())?;
    let admin_token = livekit_room_admin_token(api_key, api_secret, conversation_id)?;
    let client = reqwest::Client::new();
    let list_response = client
        .post(format!(
            "{api_base}/twirp/livekit.RoomService/ListParticipants"
        ))
        .timeout(Duration::from_secs(5))
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({"room": conversation_id}))
        .send()
        .await
        .map_err(|error| format!("participant list request failed: {error}"))?;
    if list_response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    let list_response = list_response
        .error_for_status()
        .map_err(|error| format!("participant list rejected: {error}"))?;
    let body: serde_json::Value = list_response
        .json()
        .await
        .map_err(|error| format!("participant list response was invalid: {error}"))?;
    let participants = body
        .get("participants")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "participant list response had no participants array".to_owned())?;
    let identity_prefix = format!("{account_id}-");
    let identities = participants
        .iter()
        .filter_map(|participant| {
            let identity = participant.get("identity")?.as_str()?;
            let metadata_matches = participant
                .get("metadata")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                .and_then(|value| {
                    value
                        .get("account_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .is_some_and(|id| id == account_id);
            (metadata_matches || identity.starts_with(&identity_prefix))
                .then(|| identity.to_owned())
        })
        .collect::<Vec<_>>();

    for identity in identities {
        let response = client
            .post(format!(
                "{api_base}/twirp/livekit.RoomService/RemoveParticipant"
            ))
            .timeout(Duration::from_secs(5))
            .bearer_auth(&admin_token)
            .json(&serde_json::json!({"room": conversation_id, "identity": identity}))
            .send()
            .await
            .map_err(|error| format!("participant removal request failed: {error}"))?;
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            response
                .error_for_status()
                .map_err(|error| format!("participant removal was rejected: {error}"))?;
        }
    }
    Ok(())
}

pub(crate) fn livekit_room_admin_token(
    api_key: &str,
    api_secret: &str,
    conversation_id: &str,
) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iss": api_key,
        "sub": "ghostchat-server",
        "iat": now,
        "nbf": now.saturating_sub(5),
        "exp": now.saturating_add(60),
        "video": {"roomAdmin": true, "room": conversation_id}
    });
    let payload =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|error| error.to_string())?);
    let signing_input = format!("{header}.{payload}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_secret.as_bytes()).map_err(|error| error.to_string())?;
    mac.update(signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
