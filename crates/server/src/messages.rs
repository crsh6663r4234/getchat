//! Messages: listing, sending, editing, deleting, reactions, pins, saved messages, read state and search.

use super::*;

pub(crate) const MESSAGE_LIMIT: usize = 4_000;

pub(crate) const PAGE_SIZE: i64 = 50;

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct MessageRow {
    pub(crate) sequence: i64,
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    pub(crate) sender_id: String,
    pub(crate) sender_name: String,
    pub(crate) sender_avatar: Option<String>,
    pub(crate) client_message_id: String,
    pub(crate) content: String,
    pub(crate) created_at: String,
    pub(crate) edited_at: Option<String>,
    pub(crate) pinned_at: Option<String>,
    pub(crate) reply_to_id: Option<String>,
    pub(crate) reply_to_content: Option<String>,
    pub(crate) reply_to_sender: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SavedMessageState {
    pub(crate) saved: bool,
}

pub(crate) async fn list_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<MessageRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(PAGE_SIZE)
        .clamp(1, PAGE_SIZE);
    let before = query
        .get("before")
        .and_then(|value| value.parse::<i64>().ok());
    let rows = if let Some(before) = before {
        sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.rowid<? ORDER BY m.rowid DESC LIMIT ?")
            .bind(&id).bind(before).bind(limit).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?
    } else {
        sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? ORDER BY m.rowid DESC LIMIT ?")
            .bind(&id).bind(limit).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?
    };
    Ok(Json(rows.into_iter().rev().collect()))
}

#[derive(Deserialize)]
pub(crate) struct ReadConversationRequest {
    pub(crate) sequence: i64,
}

#[derive(Serialize)]
pub(crate) struct ReadConversationReply {
    pub(crate) last_read_sequence: i64,
}

pub(crate) async fn mark_conversation_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<ReadConversationRequest>,
) -> Result<Json<ReadConversationReply>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if body.sequence < 0 {
        return Err(ApiError::bad("Read positions must be zero or greater."));
    }
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    let max_sequence = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(rowid) FROM messages WHERE conversation_id=?",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .unwrap_or(0);
    if body.sequence > max_sequence {
        return Err(ApiError::bad(
            "The read position is newer than this conversation.",
        ));
    }
    if !allow_rate(
        &state,
        &format!("read-position:{}:{conversation_id}", user.id),
        120,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Read-position updates are temporarily limited.",
        ));
    }
    let advanced = sqlx::query_scalar::<_, i64>(
        "INSERT INTO conversation_reads(conversation_id,user_id,last_read_sequence) VALUES(?,?,?) \
         ON CONFLICT(conversation_id,user_id) DO UPDATE SET \
         last_read_sequence=max(conversation_reads.last_read_sequence,excluded.last_read_sequence), \
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE excluded.last_read_sequence>conversation_reads.last_read_sequence \
         RETURNING last_read_sequence",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .bind(body.sequence)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let last_read_sequence = if let Some(sequence) = advanced {
        sequence
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT last_read_sequence FROM conversation_reads WHERE conversation_id=? AND user_id=?",
        )
        .bind(&conversation_id)
        .bind(&user.id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
    };
    let recipients = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? AND user_id<>?",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if advanced.is_some() {
        let _ = state.events.send(ServerEvent {
            recipients,
            event: serde_json::json!({
                "type": "read_receipt",
                "conversation_id": conversation_id,
                "user_id": user.id,
                "sequence": last_read_sequence,
            }),
        });
    }
    Ok(Json(ReadConversationReply { last_read_sequence }))
}

#[derive(Serialize, FromRow)]
pub(crate) struct MessageReactionSummary {
    pub(crate) message_id: String,
    pub(crate) emoji: String,
    pub(crate) count: i64,
    pub(crate) reacted: i64,
}

pub(crate) async fn list_conversation_reactions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<Vec<MessageReactionSummary>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    let reactions = sqlx::query_as::<_, MessageReactionSummary>(
        "SELECT r.message_id,r.emoji,count(*) AS count,max(CASE WHEN r.user_id=? THEN 1 ELSE 0 END) AS reacted FROM message_reactions r JOIN messages m ON m.id=r.message_id WHERE m.conversation_id=? GROUP BY r.message_id,r.emoji ORDER BY r.message_id,r.emoji",
    )
    .bind(&user.id)
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(reactions))
}

#[derive(Deserialize)]
pub(crate) struct ToggleMessageReaction {
    pub(crate) emoji: String,
}

pub(crate) async fn toggle_message_reaction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
    Json(body): Json<ToggleMessageReaction>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !matches!(body.emoji.as_str(), "👍" | "❤️" | "😂" | "👀" | "🎉") {
        return Err(ApiError::bad("Choose a supported reaction."));
    }
    let conversation_id =
        sqlx::query_scalar::<_, String>("SELECT conversation_id FROM messages WHERE id=?")
            .bind(&message_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::not_found)?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    if !allow_rate(
        &state,
        &format!("reaction:{}", user.id),
        90,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please slow down and try again.",
        ));
    }
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM message_reactions WHERE message_id=? AND user_id=? AND emoji=?",
    )
    .bind(&message_id)
    .bind(&user.id)
    .bind(&body.emoji)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if exists > 0 {
        sqlx::query("DELETE FROM message_reactions WHERE message_id=? AND user_id=? AND emoji=?")
            .bind(&message_id)
            .bind(&user.id)
            .bind(&body.emoji)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    } else {
        sqlx::query("INSERT INTO message_reactions(message_id,user_id,emoji) VALUES(?,?,?)")
            .bind(&message_id)
            .bind(&user.id)
            .bind(&body.emoji)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let recipients = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"reaction_updated","conversation_id":conversation_id,"message_id":message_id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn toggle_message_pin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let conversation_id =
        sqlx::query_scalar::<_, String>("SELECT conversation_id FROM messages WHERE id=?")
            .bind(&message_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::not_found)?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    if !allow_rate(
        &state,
        &format!("pin:{}", user.id),
        30,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please slow down and try again.",
        ));
    }
    sqlx::query("UPDATE messages SET pinned_at=CASE WHEN pinned_at IS NULL THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') ELSE NULL END WHERE id=?")
        .bind(&message_id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let recipients = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(&conversation_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"message_pin_updated","conversation_id":conversation_id,"message_id":message_id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn toggle_saved_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Result<Json<SavedMessageState>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let conversation_id =
        sqlx::query_scalar::<_, String>("SELECT conversation_id FROM messages WHERE id=?")
            .bind(&message_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::not_found)?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    if !allow_rate(
        &state,
        &format!("save:{}", user.id),
        60,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please slow down and try again.",
        ));
    }
    let removed = sqlx::query("DELETE FROM saved_messages WHERE user_id=? AND message_id=?")
        .bind(&user.id)
        .bind(&message_id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        .rows_affected()
        > 0;
    let saved = if removed {
        false
    } else {
        sqlx::query("INSERT INTO saved_messages(user_id,message_id) VALUES(?,?)")
            .bind(&user.id)
            .bind(&message_id)
            .execute(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
        true
    };
    Ok(Json(SavedMessageState { saved }))
}

pub(crate) async fn list_saved_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MessageRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM saved_messages s JOIN messages m ON m.id=s.message_id JOIN users u ON u.id=m.sender_id JOIN conversation_members c ON c.conversation_id=m.conversation_id AND c.user_id=s.user_id WHERE s.user_id=? ORDER BY s.saved_at DESC LIMIT 200")
        .bind(&user.id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub(crate) struct SendMessage {
    pub(crate) client_message_id: String,
    pub(crate) content: String,
    pub(crate) reply_to_id: Option<String>,
    /// Encrypted attachments (uploaded by the sender to this conversation)
    /// whose keys this e2e1 message carries; they are deleted with it.
    #[serde(default)]
    pub(crate) attachment_ids: Vec<String>,
    /// The conversation's commit sequence the sender's device had applied
    /// when it encrypted this message. Optional (older clients, direct
    /// conversations); when given, a new message is refused once the
    /// conversation has moved on, because devices that applied the newer
    /// commit can no longer decrypt it.
    #[serde(default)]
    pub(crate) expected_commit_seq: Option<i64>,
}

pub(crate) async fn send_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<SendMessage>,
) -> Result<Json<MessageRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    ensure_direct_not_blocked(&state.db, &conversation_id, &user.id).await?;
    if !allow_rate(
        &state,
        &format!("msg:{}", user.id),
        30,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please slow down and try again.",
        ));
    }
    let content = body.content.trim();
    if content.is_empty() || content.chars().count() > MESSAGE_LIMIT {
        return Err(ApiError::bad(
            "Messages must contain 1 to 4,000 characters.",
        ));
    }
    if content.starts_with(ATTACHMENT_MARKER) {
        return Err(ApiError::bad(
            "Attachment messages must use the upload action.",
        ));
    }
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked != 0 {
        let Some(payload) = content.strip_prefix("e2e1:") else {
            return Err(ApiError::conflict(
                "This encrypted conversation only accepts MLS-encrypted text messages.",
            ));
        };
        let decoded = URL_SAFE_NO_PAD
            .decode(payload.as_bytes())
            .map_err(|_| ApiError::bad("Encrypted messages must use URL-safe base64."))?;
        if decoded.is_empty() || decoded.len() > MAX_CRYPTO_PAYLOAD_BYTES {
            return Err(ApiError::bad("The encrypted message has an invalid size."));
        }
    }
    if body.client_message_id.len() > 64 || body.client_message_id.is_empty() {
        return Err(ApiError::bad("Invalid message ID."));
    }
    let mut attachment_ids = body
        .attachment_ids
        .iter()
        .map(|id| {
            Uuid::parse_str(id)
                .map(|id| id.to_string())
                .map_err(|_| ApiError::bad("Invalid encrypted attachment ID."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    attachment_ids.sort();
    attachment_ids.dedup();
    if attachment_ids.len() > 8 || (!attachment_ids.is_empty() && crypto_locked == 0) {
        return Err(ApiError::bad(
            "Only encrypted messages can carry encrypted attachments.",
        ));
    }
    if let Some(reply_to_id) = body.reply_to_id.as_deref() {
        let can_reply = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM messages WHERE id=? AND conversation_id=?",
        )
        .bind(reply_to_id)
        .bind(&conversation_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        if can_reply == 0 {
            return Err(ApiError::bad(
                "Replies must point to a message in this conversation.",
            ));
        }
    }
    let id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    // A client id is deduplicated per sender, but another sender's message
    // with the same client id in this conversation is refused: a reader that
    // caches texts by client id must not show that message's text under this
    // one. The check is part of the insert so no concurrent send slips past it.
    // All parameters are numbered: sqlx binds a plain ? after a ?N to the
    // wrong argument.
    // The same insert refuses a message encrypted on a group state the
    // conversation has left (?7, see SendMessage::expected_commit_seq): a
    // membership commit accepted in between cannot slip past it either.
    let inserted = sqlx::query("INSERT OR IGNORE INTO messages(id,conversation_id,sender_id,client_message_id,content,reply_to_id) SELECT ?1,?2,?3,?4,?5,?6 WHERE NOT EXISTS(SELECT 1 FROM messages WHERE conversation_id=?2 AND client_message_id=?4 AND sender_id<>?3) AND (?7 IS NULL OR NOT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?2 AND commit_seq<>?7))")
        .bind(&id).bind(&conversation_id).bind(&user.id).bind(&body.client_message_id).bind(content).bind(body.reply_to_id.as_deref()).bind(body.expected_commit_seq).execute(&mut *tx).await.map_err(|error| {
            if error.as_database_error().is_some_and(|database_error| database_error.message().contains("encrypted conversation only accepts MLS content")) {
                ApiError::conflict("This encrypted conversation only accepts MLS-encrypted text messages.")
            } else {
                ApiError::internal()
            }
        })?;
    if inserted.rows_affected() > 0 {
        for attachment_id in &attachment_ids {
            let linked = sqlx::query(
                "UPDATE attachments SET message_id=? WHERE id=? AND conversation_id=? AND uploaded_by=? AND encrypted=1 AND message_id IS NULL",
            )
            .bind(&id)
            .bind(attachment_id)
            .bind(&conversation_id)
            .bind(&user.id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            if linked.rows_affected() != 1 {
                return Err(ApiError::bad(
                    "An encrypted attachment must be your own unsent upload in this conversation.",
                ));
            }
        }
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let message = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.sender_id=? AND m.client_message_id=?")
        .bind(&conversation_id).bind(&user.id).bind(&body.client_message_id).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?
        .ok_or_else(|| {
            ApiError::conflict("This message ID is already used in this conversation.")
                .with_code("client_message_id_in_use")
        });
    // Nothing was stored and the sender has no earlier row of this id (a
    // retry of a stored message is answered above whatever it carries).
    let message = match message {
        Ok(message) => message,
        Err(id_in_use) => {
            let other_sender = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation_id=? AND client_message_id=? AND sender_id<>?)",
            )
            .bind(&conversation_id)
            .bind(&body.client_message_id)
            .bind(&user.id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
            if other_sender != 0 || body.expected_commit_seq.is_none() {
                return Err(id_in_use);
            }
            return Err(ApiError::conflict(
                "The encrypted group changed after this message was encrypted. Sync the group and send it again.",
            )
            .with_code("stale_commit_seq"));
        }
    };
    if inserted.rows_affected() > 0 {
        publish_message(&state, &conversation_id, &message).await?;
    }
    Ok(Json(message))
}

pub(crate) async fn publish_message(
    state: &AppState,
    conversation_id: &str,
    message: &MessageRow,
) -> Result<(), ApiError> {
    let recipients = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let event = serde_json::json!({"type":"message","message":message});
    let _ = state.events.send(ServerEvent { recipients, event });
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct EditMessage {
    pub(crate) content: String,
}

pub(crate) async fn edit_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<EditMessage>,
) -> Result<Json<MessageRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let content = body.content.trim();
    if content.is_empty() || content.chars().count() > MESSAGE_LIMIT {
        return Err(ApiError::bad(
            "Messages must contain 1 to 4,000 characters.",
        ));
    }
    if content.starts_with(ATTACHMENT_MARKER) {
        return Err(ApiError::bad(
            "Attachment messages must use the upload action.",
        ));
    }
    let msg = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.id=?")
        .bind(&id).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?.ok_or_else(ApiError::not_found)?;
    if !is_member(&state.db, &msg.conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    ensure_direct_not_blocked(&state.db, &msg.conversation_id, &user.id).await?;
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&msg.conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked != 0 {
        return Err(ApiError::conflict(
            "Messages cannot be edited in an encrypted conversation; send a new encrypted message instead.",
        ));
    }
    if msg.sender_id != user.id {
        return Err(ApiError::forbidden());
    }
    if msg.content.starts_with(ATTACHMENT_MARKER) {
        return Err(ApiError::bad("File messages cannot be edited."));
    }
    sqlx::query(
        "UPDATE messages SET content=?,edited_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?",
    )
    .bind(content)
    .bind(&id)
    .execute(&state.db)
    .await
    .map_err(|error| {
        if error.as_database_error().is_some_and(|database_error| database_error.message().contains("encrypted conversation does not allow message edits")) {
            ApiError::conflict("Messages cannot be edited in an encrypted conversation; send a new encrypted message instead.")
        } else {
            ApiError::internal()
        }
    })?;
    let updated = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.id=?")
        .bind(&id).fetch_one(&state.db).await.map_err(|_| ApiError::internal())?;
    publish_message(&state, &updated.conversation_id, &updated).await?;
    Ok(Json(updated))
}

pub(crate) async fn delete_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let msg = sqlx::query_as::<_, (String, String, String)>(
        "SELECT conversation_id,sender_id,content FROM messages WHERE id=?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if msg.1 != user.id {
        return Err(ApiError::forbidden());
    }
    let attachment_id = msg.2.strip_prefix(ATTACHMENT_MARKER).map(str::to_owned);
    let stored_on_disk = if let Some(attachment_id) = attachment_id.as_deref() {
        sqlx::query_scalar::<_, i64>(
            "SELECT stored_on_disk FROM attachments WHERE id=? AND conversation_id=?",
        )
        .bind(attachment_id)
        .bind(&msg.0)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        .unwrap_or(0)
            == 1
    } else {
        false
    };
    let encrypted_attachment_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM attachments WHERE message_id=? AND conversation_id=? AND encrypted=1",
    )
    .bind(&id)
    .bind(&msg.0)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM attachments WHERE message_id=? AND conversation_id=? AND encrypted=1")
        .bind(&id)
        .bind(&msg.0)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    if let Some(attachment_id) = attachment_id.as_deref() {
        sqlx::query("DELETE FROM attachments WHERE id=? AND conversation_id=?")
            .bind(attachment_id)
            .bind(&msg.0)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    sqlx::query("DELETE FROM messages WHERE id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    if stored_on_disk {
        if let Some(attachment_id) = attachment_id {
            let path = state.attachment_dir.join(format!("{attachment_id}.blob"));
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    for attachment_id in encrypted_attachment_ids {
        let path = state.attachment_dir.join(format!("{attachment_id}.blob"));
        let _ = tokio::fs::remove_file(path).await;
    }
    let recipients = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(&msg.0)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"deleted","id":id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn search_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<MessageRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let q = query.get("q").map(|v| v.trim()).unwrap_or("");
    if q.len() < 2 || q.len() > 128 {
        return Ok(Json(vec![]));
    }
    let rows = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id JOIN conversation_members c ON c.conversation_id=m.conversation_id WHERE c.user_id=? AND m.content LIKE ? ORDER BY m.rowid DESC LIMIT 50")
        .bind(user.id).bind(format!("%{q}%")).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}
