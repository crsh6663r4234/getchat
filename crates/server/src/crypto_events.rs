//! The encrypted (MLS) event relay.

use super::*;

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct CryptoEventRow {
    pub(crate) sequence: i64,
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    pub(crate) sender_id: String,
    pub(crate) recipient_id: Option<String>,
    pub(crate) payload: String,
    pub(crate) created_at: String,
}

/// A relay event as listed for a member, with the setup generation of its
/// conversation at the time of the listing.
#[derive(Serialize, FromRow)]
pub(crate) struct CryptoEventListRow {
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub(crate) event: CryptoEventRow,
    pub(crate) setup_generation: i64,
    pub(crate) commit_seq: i64,
}

#[derive(Deserialize)]
pub(crate) struct SendCryptoEvent {
    pub(crate) client_event_id: String,
    pub(crate) recipient_user_id: Option<String>,
    pub(crate) payload: String,
}

pub(crate) async fn list_crypto_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<CryptoEventListRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }

    let after = query
        .get("after")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, 100);
    // The generation is read in the same statement as the events, so every
    // listed event belongs to the setup generation it is listed with (a
    // takeover deletes the older setup's events in its own transaction).
    // Likewise commit_seq: every commit counted in it was relayed before
    // this listing, so a member that has applied the listed events (to the
    // end of the list) has applied at least that many commits.
    let events = sqlx::query_as::<_, CryptoEventListRow>(
        "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at,COALESCE((SELECT l.setup_generation FROM conversation_crypto_locks l WHERE l.conversation_id=crypto_events.conversation_id),0) AS setup_generation,COALESCE((SELECT l.commit_seq FROM conversation_crypto_locks l WHERE l.conversation_id=crypto_events.conversation_id),0) AS commit_seq FROM crypto_events WHERE conversation_id=? AND sequence>? AND (recipient_id IS NULL OR recipient_id=?) ORDER BY sequence ASC LIMIT ?",
    )
    .bind(&conversation_id)
    .bind(after)
    .bind(&user.id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;

    Ok(Json(events))
}

pub(crate) async fn send_crypto_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<SendCryptoEvent>,
) -> Result<Json<CryptoEventRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    let recipient_id = body
        .recipient_user_id
        .as_deref()
        .map(|value| Uuid::parse_str(value).map(|id| id.to_string()))
        .transpose()
        .map_err(|_| ApiError::bad("Invalid encrypted event recipient."))?;
    if let Some(recipient_id) = recipient_id.as_deref() {
        if !is_member(&state.db, &conversation_id, recipient_id).await? {
            return Err(ApiError::not_found());
        }
    }
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked == 0 {
        return Err(ApiError::conflict(
            "Start the conversation's authenticated MLS setup before sending crypto events.",
        ));
    }
    if !allow_rate(
        &state,
        &format!("crypto-event:{}", user.id),
        120,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please slow down and try again.",
        ));
    }
    if body.client_event_id.is_empty()
        || body.client_event_id.len() > 64
        || !body
            .client_event_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApiError::bad("Invalid encrypted event ID."));
    }
    if body.payload.is_empty() || body.payload.len() > MAX_CRYPTO_PAYLOAD_BYTES * 4 / 3 + 8 {
        return Err(ApiError::crypto_payload_too_large());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(body.payload.as_bytes())
        .map_err(|_| ApiError::bad("Encrypted events must use URL-safe base64."))?;
    if decoded.is_empty() || decoded.len() > MAX_CRYPTO_PAYLOAD_BYTES {
        return Err(ApiError::crypto_payload_too_large());
    }

    let id = Uuid::new_v4().to_string();
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload) VALUES(?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&conversation_id)
    .bind(&user.id)
    .bind(&body.client_event_id)
    .bind(&recipient_id)
    .bind(&body.payload)
    .execute(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let event = sqlx::query_as::<_, CryptoEventRow>(
        "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sender_id=? AND client_event_id=?",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .bind(&body.client_event_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;

    if event.recipient_id != recipient_id || event.payload != body.payload {
        return Err(ApiError::conflict(
            "This encrypted event ID was already used for a different event.",
        ));
    }

    if inserted.rows_affected() > 0 {
        publish_crypto_event(&state, &event).await?;
    }
    Ok(Json(event))
}

pub(crate) async fn publish_crypto_event(
    state: &AppState,
    event: &CryptoEventRow,
) -> Result<(), ApiError> {
    let recipients = if let Some(recipient_id) = event.recipient_id.as_deref() {
        vec![recipient_id.to_owned()]
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM conversation_members WHERE conversation_id=?",
        )
        .bind(&event.conversation_id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
    };
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"crypto_event","event":event}),
    });
    Ok(())
}
