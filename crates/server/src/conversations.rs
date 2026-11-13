//! Conversations: direct and group creation, listing, members, renaming and the membership checks.

use super::*;

#[derive(Deserialize)]
pub(crate) struct DirectConversationRequest {
    pub(crate) user_id: String,
}

#[derive(Serialize, FromRow)]
pub(crate) struct ConversationRow {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) title: Option<String>,
    pub(crate) peer_id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) avatar_data: Option<String>,
    pub(crate) member_count: i64,
    pub(crate) member_names: Option<String>,
    pub(crate) last_message: Option<String>,
    pub(crate) last_at: Option<String>,
    pub(crate) unread_count: i64,
    pub(crate) peer_read_sequence: Option<i64>,
}

pub(crate) async fn create_direct_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DirectConversationRequest>,
) -> Result<Json<ConversationRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if body.user_id == user.id {
        return Err(ApiError::bad(
            "You cannot start a conversation with yourself.",
        ));
    }
    if is_blocked_pair(&state.db, &user.id, &body.user_id).await? {
        return Err(ApiError::forbidden());
    }
    let (low, high) = ordered(&user.id, &body.user_id);
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM friendships WHERE user_low=? AND user_high=?",
    )
    .bind(&low)
    .bind(&high)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
        == 0
    {
        return Err(ApiError::forbidden());
    }
    let existing = sqlx::query_scalar::<_, String>("SELECT a.conversation_id FROM conversation_members a JOIN conversation_members b ON b.conversation_id=a.conversation_id JOIN conversations c ON c.id=a.conversation_id WHERE c.kind='direct' AND a.user_id=? AND b.user_id=? LIMIT 1")
        .bind(&user.id).bind(&body.user_id).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?;
    let id = if let Some(id) = existing {
        id
    } else {
        let id = Uuid::new_v4().to_string();
        let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
        sqlx::query("INSERT INTO conversations(id) VALUES(?)")
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
        sqlx::query("INSERT INTO conversation_members(conversation_id,user_id) VALUES(?,?),(?,?)")
            .bind(&id)
            .bind(&user.id)
            .bind(&id)
            .bind(&body.user_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
        tx.commit().await.map_err(|_| ApiError::internal())?;
        id
    };
    conversation_row(&state.db, &id, &user.id).await.map(Json)
}

#[derive(Deserialize)]
pub(crate) struct GroupConversationRequest {
    pub(crate) name: String,
    pub(crate) member_ids: Vec<String>,
}

pub(crate) async fn create_group_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GroupConversationRequest>,
) -> Result<Json<ConversationRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let title = body.name.trim();
    if title.is_empty() || title.chars().count() > 50 || title.chars().any(char::is_control) {
        return Err(ApiError::bad(
            "Group names must contain 1 to 50 characters.",
        ));
    }

    let mut member_ids = body.member_ids;
    if member_ids.len() > 16 {
        return Err(ApiError::bad("Choose 2 to 15 friends for a group."));
    }
    member_ids.retain(|id| id != &user.id);
    // Keep the given order (it is the founders' join order); drop repeats.
    let mut seen = std::collections::HashSet::new();
    member_ids.retain(|id| seen.insert(id.clone()));
    if !(2..=15).contains(&member_ids.len()) {
        return Err(ApiError::bad("Choose 2 to 15 friends for a group."));
    }

    for member_id in &member_ids {
        let (low, high) = ordered(&user.id, member_id);
        let is_friend = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM friendships WHERE user_low=? AND user_high=?",
        )
        .bind(low)
        .bind(high)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        if is_friend == 0 {
            return Err(ApiError::forbidden());
        }
    }

    let id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("INSERT INTO conversations(id,kind,title) VALUES(?,'group',?)")
        .bind(&id)
        .bind(title)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    sqlx::query(
        "INSERT INTO conversation_members(conversation_id,user_id,role) VALUES(?,?,'owner')",
    )
    .bind(&id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    for member_id in &member_ids {
        sqlx::query("INSERT INTO conversation_members(conversation_id,user_id) VALUES(?,?)")
            .bind(&id)
            .bind(member_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;

    let recipients = std::iter::once(user.id.clone())
        .chain(member_ids.into_iter())
        .collect::<Vec<_>>();
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"conversation_created","conversation_id":id,"created_by":user.id}),
    });
    conversation_row(&state.db, &id, &user.id).await.map(Json)
}

pub(crate) async fn conversation_row(
    db: &SqlitePool,
    id: &str,
    user_id: &str,
) -> Result<ConversationRow, ApiError> {
    sqlx::query_as::<_, ConversationRow>(
        "SELECT c.id,c.kind,c.title,\
         COALESCE((SELECT u.id FROM conversation_members b JOIN users u ON u.id=b.user_id WHERE b.conversation_id=c.id AND b.user_id<>? LIMIT 1),'') AS peer_id,\
         COALESCE((SELECT u.username FROM conversation_members b JOIN users u ON u.id=b.user_id WHERE b.conversation_id=c.id AND b.user_id<>? LIMIT 1),'') AS username,\
         CASE WHEN c.kind='group' THEN c.title ELSE (SELECT u.display_name FROM conversation_members b JOIN users u ON u.id=b.user_id WHERE b.conversation_id=c.id AND b.user_id<>? LIMIT 1) END AS display_name,\
         CASE WHEN c.kind='group' THEN NULL ELSE (SELECT u.avatar_data FROM conversation_members b JOIN users u ON u.id=b.user_id WHERE b.conversation_id=c.id AND b.user_id<>? LIMIT 1) END AS avatar_data,\
         (SELECT count(*) FROM conversation_members cm WHERE cm.conversation_id=c.id) AS member_count,\
         (SELECT group_concat(u.display_name, ', ') FROM conversation_members cm JOIN users u ON u.id=cm.user_id WHERE cm.conversation_id=c.id AND cm.user_id<>?) AS member_names,\
         (SELECT content FROM messages m WHERE m.conversation_id=c.id ORDER BY m.rowid DESC LIMIT 1) AS last_message,\
         (SELECT created_at FROM messages m WHERE m.conversation_id=c.id ORDER BY m.rowid DESC LIMIT 1) AS last_at,\
         (SELECT count(*) FROM messages m WHERE m.conversation_id=c.id AND m.sender_id<>? AND m.rowid>COALESCE((SELECT cr.last_read_sequence FROM conversation_reads cr WHERE cr.conversation_id=c.id AND cr.user_id=?),0)) AS unread_count,\
         CASE WHEN c.kind='direct' THEN (SELECT max(cr.last_read_sequence) FROM conversation_reads cr JOIN conversation_members cm ON cm.conversation_id=cr.conversation_id AND cm.user_id=cr.user_id WHERE cr.conversation_id=c.id AND cm.user_id<>?) ELSE NULL END AS peer_read_sequence \
         FROM conversations c WHERE c.id=? AND EXISTS(SELECT 1 FROM conversation_members cm WHERE cm.conversation_id=c.id AND cm.user_id=?)",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|error| {
        tracing::error!(%error, conversation_id = id, "could not load conversation summary");
        ApiError::internal()
    })?
    .ok_or_else(ApiError::not_found)
}

pub(crate) async fn list_conversations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ConversationRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT conversation_id FROM conversation_members WHERE user_id=? ORDER BY conversation_id",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let mut rows = Vec::new();
    for id in ids {
        if let Ok(row) = conversation_row(&state.db, &id, &user.id).await {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| b.last_at.cmp(&a.last_at));
    Ok(Json(rows))
}

#[derive(Serialize, FromRow)]
pub(crate) struct GroupMemberRow {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) username: String,
    pub(crate) avatar_data: Option<String>,
    pub(crate) role: String,
    /// Join order (lower joined earlier), for deterministic client fallbacks.
    pub(crate) joined_seq: Option<i64>,
}

pub(crate) async fn group_recipients(
    db: &SqlitePool,
    conversation_id: &str,
) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
    .map_err(|_| ApiError::internal())
}

pub(crate) async fn list_group_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<GroupMemberRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let members = sqlx::query_as::<_, GroupMemberRow>(
        "SELECT u.id,u.display_name,u.username,u.avatar_data,cm.role,cm.joined_seq FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id JOIN users u ON u.id=cm.user_id WHERE cm.conversation_id=? AND c.kind='group' AND EXISTS(SELECT 1 FROM conversation_members viewer WHERE viewer.conversation_id=c.id AND viewer.user_id=?) ORDER BY CASE cm.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END,u.display_name COLLATE NOCASE",
    )
    .bind(id)
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if members.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(Json(members))
}

#[derive(Deserialize)]
pub(crate) struct RenameGroupRequest {
    pub(crate) name: String,
}

pub(crate) async fn rename_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RenameGroupRequest>,
) -> Result<Json<ConversationRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if role != "owner" {
        return Err(ApiError::forbidden());
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 50 || name.chars().any(char::is_control) {
        return Err(ApiError::bad(
            "Group names must contain 1 to 50 characters.",
        ));
    }
    let result = sqlx::query("UPDATE conversations SET title=? WHERE id=? AND kind='group'")
        .bind(name)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let recipients = group_recipients(&state.db, &id).await?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    conversation_row(&state.db, &id, &user.id).await.map(Json)
}

pub(crate) async fn require_group_manager(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), ApiError> {
    let role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if role == "owner" || role == "admin" {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

pub(crate) async fn is_member(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversation_members WHERE conversation_id=? AND user_id=?",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_one(db)
    .await
    .map(|n| n > 0)
    .map_err(|_| ApiError::internal())
}

pub(crate) async fn ensure_direct_not_blocked(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), ApiError> {
    let peer_id = sqlx::query_scalar::<_, String>(
        "SELECT member.user_id FROM conversations c JOIN conversation_members member ON member.conversation_id=c.id WHERE c.id=? AND c.kind='direct' AND member.user_id<>? LIMIT 1",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|_| ApiError::internal())?;
    if let Some(peer_id) = peer_id {
        if is_blocked_pair(db, user_id, &peer_id).await? {
            return Err(ApiError::forbidden());
        }
    }
    Ok(())
}
