//! Account data export and account deletion.

use super::*;

#[derive(Serialize)]
pub(crate) struct ExportConversation {
    pub(crate) conversation: ConversationRow,
    pub(crate) messages: Vec<MessageRow>,
    pub(crate) reactions: Vec<MessageReactionExport>,
    pub(crate) crypto_events: Vec<CryptoEventRow>,
}

#[derive(Serialize, FromRow)]
pub(crate) struct MessageReactionExport {
    pub(crate) message_id: String,
    pub(crate) user_id: String,
    pub(crate) emoji: String,
    pub(crate) created_at: String,
}

#[derive(Serialize)]
pub(crate) struct DataExport {
    pub(crate) exported_at: String,
    pub(crate) account: PublicUser,
    pub(crate) friends: Vec<PublicUser>,
    pub(crate) blocked_users: Vec<PublicUser>,
    pub(crate) submitted_reports: Vec<UserReportExport>,
    pub(crate) conversations: Vec<ExportConversation>,
}

#[derive(Serialize, FromRow)]
pub(crate) struct UserReportExport {
    pub(crate) target_id: String,
    pub(crate) conversation_id: Option<String>,
    pub(crate) category: String,
    pub(crate) details: String,
    pub(crate) created_at: String,
}

pub(crate) async fn export_data(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DataExport>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let friends = sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM friendships f JOIN users u ON u.id=CASE WHEN f.user_low=? THEN f.user_high ELSE f.user_low END WHERE f.user_low=? OR f.user_high=? ORDER BY u.display_name COLLATE NOCASE")
        .bind(&user.id).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    let blocked_users = sqlx::query_as::<_, PublicUser>(
        "SELECT u.id,u.username,u.display_name,u.avatar_data FROM user_blocks b JOIN users u ON u.id=b.blocked_id WHERE b.blocker_id=? ORDER BY b.created_at",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let submitted_reports = sqlx::query_as::<_, UserReportExport>(
        "SELECT target_id,conversation_id,category,details,created_at FROM user_reports WHERE reporter_id=? ORDER BY created_at",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT conversation_id FROM conversation_members WHERE user_id=? ORDER BY conversation_id",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let mut conversations = Vec::with_capacity(ids.len());
    for id in ids {
        let conversation = conversation_row(&state.db, &id, &user.id).await?;
        let messages = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? ORDER BY m.rowid")
            .bind(&id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
        let reactions = sqlx::query_as::<_, MessageReactionExport>(
            "SELECT r.message_id,r.user_id,r.emoji,r.created_at FROM message_reactions r JOIN messages m ON m.id=r.message_id WHERE m.conversation_id=? ORDER BY r.message_id,r.created_at",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        let crypto_events = sqlx::query_as::<_, CryptoEventRow>(
            "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND (recipient_id IS NULL OR recipient_id=?) ORDER BY sequence",
        )
        .bind(&id)
        .bind(&user.id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        conversations.push(ExportConversation {
            conversation,
            messages,
            reactions,
            crypto_events,
        });
    }
    let exported_at =
        sqlx::query_scalar::<_, String>("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')")
            .fetch_one(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
    Ok(Json(DataExport {
        exported_at,
        account: user,
        friends,
        blocked_users,
        submitted_reports,
        conversations,
    }))
}

pub(crate) async fn delete_account(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let memberships = sqlx::query_as::<_, (String, String, String)>(
        "SELECT c.id,c.kind,cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.user_id=?",
    )
    .bind(&user.id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    // Direct conversations are removed with the account (unchanged). In a
    // group only this membership is removed, like a leave: ownership passes
    // to the oldest admin, else the oldest member, by the stored join
    // sequence (founders in the order given at creation, after the
    // creator). A group is deleted only once nobody is left in it.
    let mut deleted_conversations = Vec::new();
    let mut remaining_groups = Vec::new();
    for (conversation_id, kind, role) in memberships {
        if kind != "group" {
            deleted_conversations.push(conversation_id);
            continue;
        }
        sqlx::query("DELETE FROM conversation_members WHERE conversation_id=? AND user_id=?")
            .bind(&conversation_id)
            .bind(&user.id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
        let remaining = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM conversation_members WHERE conversation_id=?",
        )
        .bind(&conversation_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if remaining.is_empty() {
            deleted_conversations.push(conversation_id);
            continue;
        }
        if role == "owner" {
            sqlx::query(
                "UPDATE conversation_members SET role='owner' WHERE conversation_id=? AND user_id=(SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY CASE role WHEN 'admin' THEN 0 ELSE 1 END, joined_seq, user_id LIMIT 1) AND NOT EXISTS(SELECT 1 FROM conversation_members WHERE conversation_id=? AND role='owner')",
            )
            .bind(&conversation_id)
            .bind(&conversation_id)
            .bind(&conversation_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
        }
        // In an MLS-locked group this account's leaf stays in the MLS roster
        // until an owner/admin client commits its removal; record it so the
        // remaining clients can tell it from an unknown member.
        sqlx::query(
            "INSERT OR IGNORE INTO crypto_group_deleted_members(conversation_id,user_id) SELECT conversation_id,? FROM conversation_crypto_locks WHERE conversation_id=?",
        )
        .bind(&user.id)
        .bind(&conversation_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        media_revocation::enqueue(&mut tx, &conversation_id, &user.id).await?;
        remaining_groups.push((conversation_id, remaining));
    }
    let mut attachment_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM attachments WHERE stored_on_disk=1 AND uploaded_by=?",
    )
    .bind(&user.id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    let mut removed_for_peers = Vec::new();
    for conversation_id in &deleted_conversations {
        let peers = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM conversation_members WHERE conversation_id=? AND user_id<>?",
        )
        .bind(conversation_id)
        .bind(&user.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if !peers.is_empty() {
            removed_for_peers.push((conversation_id.clone(), peers));
        }
        attachment_ids.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM attachments WHERE stored_on_disk=1 AND conversation_id=? AND uploaded_by<>?",
            )
            .bind(conversation_id)
            .bind(&user.id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?,
        );
        sqlx::query("DELETE FROM conversations WHERE id=?")
            .bind(conversation_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    // Cascades remove this account's sessions, friendships, own messages and
    // own uploads in the remaining groups.
    sqlx::query("DELETE FROM users WHERE id=?")
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    state.media_rooms.authorization_changed();
    for attachment_id in attachment_ids {
        let path = state.attachment_dir.join(format!("{attachment_id}.blob"));
        let _ = tokio::fs::remove_file(path).await;
    }
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id.clone()],
        event: serde_json::json!({"type":"session_revoked"}),
    });
    // Peers of the removed direct conversations drop the chat, as after any
    // other removal (the conversation is gone, so their refresh omits it).
    for (conversation_id, recipients) in removed_for_peers {
        let _ = state.events.send(ServerEvent {
            recipients,
            event: serde_json::json!({"type":"conversation_updated","conversation_id":conversation_id}),
        });
    }
    for (conversation_id, recipients) in remaining_groups {
        if let Err(error) = remove_livekit_participant(&state, &conversation_id, &user.id).await {
            tracing::warn!(conversation_id = %conversation_id, account_id = %user.id, %error, "could not remove deleted account from LiveKit");
        }
        let _ = state.events.send(ServerEvent {
            recipients,
            event: serde_json::json!({"type":"conversation_updated","conversation_id":conversation_id}),
        });
    }
    Ok(StatusCode::NO_CONTENT)
}
