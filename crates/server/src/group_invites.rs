//! Group invite links.

use super::*;

#[derive(Deserialize)]
pub(crate) struct CreateGroupInviteRequest {
    pub(crate) max_uses: i64,
}

#[derive(Serialize, FromRow)]
pub(crate) struct GroupInviteRow {
    pub(crate) id: String,
    pub(crate) created_by: String,
    pub(crate) expires_at: String,
    pub(crate) max_uses: i64,
    pub(crate) use_count: i64,
    pub(crate) created_at: String,
}

#[derive(Serialize)]
pub(crate) struct GroupInviteCreated {
    pub(crate) id: String,
    pub(crate) code: String,
    pub(crate) expires_at: String,
    pub(crate) max_uses: i64,
}

pub(crate) async fn list_group_invites(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<Vec<GroupInviteRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    require_group_manager(&state.db, &conversation_id, &user.id).await?;
    let rows = sqlx::query_as::<_, GroupInviteRow>(
        "SELECT id,created_by,expires_at,max_uses,use_count,created_at FROM group_invites WHERE conversation_id=? AND revoked_at IS NULL AND datetime(expires_at)>datetime('now') AND use_count<max_uses ORDER BY created_at DESC",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

pub(crate) async fn create_group_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<CreateGroupInviteRequest>,
) -> Result<Json<GroupInviteCreated>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    require_group_manager(&state.db, &conversation_id, &user.id).await?;
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked != 0 {
        return Err(ApiError::conflict(
            "Invite codes are disabled for encrypted groups; use the MLS membership flow instead.",
        ));
    }
    if !(1..=100).contains(&body.max_uses) {
        return Err(ApiError::bad("Invite codes can be used 1 to 100 times."));
    }
    if !allow_rate(
        &state,
        &format!("group-invite-create:{}:{conversation_id}", user.id),
        20,
        Duration::from_secs(60 * 60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Group invite creation is temporarily limited.",
        ));
    }
    let mut random = [0u8; 32];
    OsRng.fill_bytes(&mut random);
    let code = URL_SAFE_NO_PAD.encode(random);
    let invite_id = Uuid::new_v4().to_string();
    let invite = sqlx::query_as::<_, (String, i64)>(
        "INSERT INTO group_invites(id,conversation_id,token_hash,created_by,expires_at,max_uses) VALUES(?,?,?, ?,strftime('%Y-%m-%dT%H:%M:%fZ','now','+7 days'),?) RETURNING expires_at,max_uses",
    )
    .bind(&invite_id)
    .bind(&conversation_id)
    .bind(session_hash(&code))
    .bind(&user.id)
    .bind(body.max_uses)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(GroupInviteCreated {
        id: invite_id,
        code,
        expires_at: invite.0,
        max_uses: invite.1,
    }))
}

pub(crate) async fn revoke_group_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((conversation_id, invite_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    require_group_manager(&state.db, &conversation_id, &user.id).await?;
    let result = sqlx::query(
        "UPDATE group_invites SET revoked_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=? AND conversation_id=? AND revoked_at IS NULL",
    )
    .bind(invite_id)
    .bind(conversation_id)
    .execute(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct JoinGroupInviteRequest {
    pub(crate) code: String,
}

#[derive(FromRow)]
pub(crate) struct GroupInviteUse {
    pub(crate) conversation_id: String,
    pub(crate) created_by: String,
}

pub(crate) async fn join_group_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<JoinGroupInviteRequest>,
) -> Result<Json<ConversationRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let decoded = URL_SAFE_NO_PAD
        .decode(&body.code)
        .map_err(|_| ApiError::not_found())?;
    if decoded.len() != 32 || body.code.len() > 64 {
        return Err(ApiError::not_found());
    }
    if !allow_rate(
        &state,
        &format!("group-invite-join:{}", user.id),
        30,
        Duration::from_secs(60 * 60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Group invite attempts are temporarily limited.",
        ));
    }
    let token_hash = session_hash(&body.code);
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let invite = sqlx::query_as::<_, GroupInviteUse>(
        "UPDATE group_invites SET use_count=use_count+1 WHERE token_hash=? AND revoked_at IS NULL AND datetime(expires_at)>datetime('now') AND use_count<max_uses AND EXISTS(SELECT 1 FROM conversations c WHERE c.id=group_invites.conversation_id AND c.kind='group') AND NOT EXISTS(SELECT 1 FROM conversation_crypto_locks l WHERE l.conversation_id=group_invites.conversation_id) AND EXISTS(SELECT 1 FROM conversation_members owner WHERE owner.conversation_id=group_invites.conversation_id AND owner.user_id=group_invites.created_by AND owner.role IN('owner','admin')) AND (SELECT count(*) FROM conversation_members cm WHERE cm.conversation_id=group_invites.conversation_id)<16 AND NOT EXISTS(SELECT 1 FROM conversation_members existing WHERE existing.conversation_id=group_invites.conversation_id AND existing.user_id=?) RETURNING conversation_id,created_by",
    )
    .bind(token_hash)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    let Some(invite) = invite else {
        let existing = sqlx::query_scalar::<_, String>(
            "SELECT gi.conversation_id FROM group_invites gi JOIN conversations c ON c.id=gi.conversation_id JOIN conversation_members cm ON cm.conversation_id=gi.conversation_id AND cm.user_id=? WHERE gi.token_hash=? AND c.kind='group'",
        )
        .bind(&user.id)
        .bind(&session_hash(&body.code))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        tx.rollback().await.map_err(|_| ApiError::internal())?;
        if let Some(conversation_id) = existing {
            return conversation_row(&state.db, &conversation_id, &user.id)
                .await
                .map(Json);
        }
        return Err(ApiError::not_found());
    };
    sqlx::query(
        "INSERT INTO conversation_members(conversation_id,user_id,role) VALUES(?,?,'member')",
    )
    .bind(&invite.conversation_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let recipients = group_recipients(&state.db, &invite.conversation_id).await?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":invite.conversation_id,"joined_user_id":user.id,"invited_by":invite.created_by}),
    });
    conversation_row(&state.db, &invite.conversation_id, &user.id)
        .await
        .map(Json)
}
