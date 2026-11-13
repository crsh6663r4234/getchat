//! Friends, blocks and user reports.

use super::*;

pub(crate) async fn is_blocked_pair(
    db: &SqlitePool,
    first: &str,
    second: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM user_blocks WHERE (blocker_id=? AND blocked_id=?) OR (blocker_id=? AND blocked_id=?))",
    )
    .bind(first)
    .bind(second)
    .bind(second)
    .bind(first)
    .fetch_one(db)
    .await
    .map(|exists| exists != 0)
    .map_err(|_| ApiError::internal())
}

#[derive(Serialize, FromRow)]
pub(crate) struct BlockedUserRow {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) avatar_data: Option<String>,
    pub(crate) created_at: String,
}

pub(crate) async fn list_blocked_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<BlockedUserRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, BlockedUserRow>(
        "SELECT u.id,u.username,u.display_name,u.avatar_data,b.created_at FROM user_blocks b JOIN users u ON u.id=b.blocked_id WHERE b.blocker_id=? ORDER BY b.created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

pub(crate) async fn block_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if target_id == user.id {
        return Err(ApiError::bad("You cannot block your own account."));
    }
    if !allow_rate(
        &state,
        &format!("block:{}", user.id),
        60,
        Duration::from_secs(3600),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before changing block settings.",
        ));
    }
    let exists = sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM users WHERE id=?)")
        .bind(&target_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    if exists == 0 {
        return Err(ApiError::not_found());
    }
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("INSERT OR IGNORE INTO user_blocks(blocker_id,blocked_id) VALUES(?,?)")
        .bind(&user.id)
        .bind(&target_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    let (low, high) = ordered(&user.id, &target_id);
    sqlx::query("DELETE FROM friendships WHERE user_low=? AND user_high=?")
        .bind(low)
        .bind(high)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM friend_requests WHERE (sender_id=? AND recipient_id=?) OR (sender_id=? AND recipient_id=?)")
        .bind(&user.id)
        .bind(&target_id)
        .bind(&target_id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    state.media_rooms.authorization_changed();
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id],
        event: serde_json::json!({"type":"user_block_updated","user_id":target_id}),
    });
    let _ = state.events.send(ServerEvent {
        recipients: vec![target_id],
        event: serde_json::json!({"type":"contacts_updated"}),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn unblock_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    // Same budget as block_user: SECURITY.md limits all blocking changes.
    if !allow_rate(
        &state,
        &format!("block:{}", user.id),
        60,
        Duration::from_secs(3600),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before changing block settings.",
        ));
    }
    sqlx::query("DELETE FROM user_blocks WHERE blocker_id=? AND blocked_id=?")
        .bind(&user.id)
        .bind(&target_id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id],
        event: serde_json::json!({"type":"user_block_updated","user_id":target_id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct ReportUserRequest {
    pub(crate) category: String,
    #[serde(default)]
    pub(crate) details: String,
    pub(crate) conversation_id: Option<String>,
}

pub(crate) async fn report_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    Json(body): Json<ReportUserRequest>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if target_id == user.id {
        return Err(ApiError::bad("You cannot report your own account."));
    }
    if !allow_rate(
        &state,
        &format!("report:{}", user.id),
        5,
        Duration::from_secs(3600),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before submitting another report.",
        ));
    }
    if !matches!(
        body.category.as_str(),
        "harassment" | "spam" | "threats" | "other"
    ) {
        return Err(ApiError::bad("Choose a valid report reason."));
    }
    let details = body.details.trim();
    if details.chars().count() > 1000
        || details
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ApiError::bad(
            "Report details must be 1,000 characters or fewer.",
        ));
    }
    let target_exists =
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM users WHERE id=?)")
            .bind(&target_id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
    if target_exists == 0 {
        return Err(ApiError::not_found());
    }
    if let Some(conversation_id) = body.conversation_id.as_deref() {
        let valid_conversation = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM conversations c JOIN conversation_members reporter ON reporter.conversation_id=c.id AND reporter.user_id=? JOIN conversation_members target ON target.conversation_id=c.id AND target.user_id=? WHERE c.id=? AND c.kind='direct')",
        )
        .bind(&user.id)
        .bind(&target_id)
        .bind(conversation_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        if valid_conversation == 0 {
            return Err(ApiError::bad(
                "The selected conversation cannot be used for this report.",
            ));
        }
    }
    sqlx::query("INSERT INTO user_reports(id,reporter_id,target_id,conversation_id,category,details) VALUES(?,?,?,?,?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(user.id)
        .bind(target_id)
        .bind(body.conversation_id)
        .bind(body.category)
        .bind(details)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, FromRow)]
pub(crate) struct FriendRequestRow {
    pub(crate) id: String,
    pub(crate) created_at: String,
    pub(crate) user_id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) avatar_data: Option<String>,
}

pub(crate) async fn send_friend_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !allow_rate(&state, &user.id, 20, Duration::from_secs(60)) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please slow down and try again.",
        ));
    }
    let target = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad("A user ID is required."))?;
    if target == user.id {
        return Err(ApiError::bad("You cannot add yourself."));
    }
    if is_blocked_pair(&state.db, &user.id, target).await? {
        return Err(ApiError::forbidden());
    }
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE id=?")
        .bind(target)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        == 0
    {
        return Err(ApiError::not_found());
    }
    let (low, high) = ordered(&user.id, target);
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM friendships WHERE user_low=? AND user_high=?",
    )
    .bind(low)
    .bind(high)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
        > 0
    {
        return Err(ApiError::conflict("You are already friends."));
    }
    let reverse = sqlx::query_scalar::<_, String>(
        "SELECT id FROM friend_requests WHERE sender_id=? AND recipient_id=?",
    )
    .bind(target)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if let Some(id) = reverse {
        accept_friend(state, &user.id, &id).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let request_id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO friend_requests(id,sender_id,recipient_id) VALUES(?,?,?)")
        .bind(&request_id)
        .bind(&user.id)
        .bind(target)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::conflict("A request is already pending."))?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![target.to_owned()],
        event: serde_json::json!({
            "type": "friend_request",
            "request": {
                "id": request_id,
                "user_id": user.id,
                "username": user.username,
                "display_name": user.display_name,
                "avatar_data": user.avatar_data,
            }
        }),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn ordered(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

pub(crate) async fn pending_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<FriendRequestRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, FriendRequestRow>("SELECT r.id,r.created_at,u.id AS user_id,u.username,u.display_name,u.avatar_data FROM friend_requests r JOIN users u ON u.id=r.sender_id WHERE r.recipient_id=? ORDER BY r.created_at DESC")
        .bind(user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

pub(crate) async fn accept_friend(
    state: AppState,
    user_id: &str,
    request_id: &str,
) -> Result<(), ApiError> {
    let sender = sqlx::query_scalar::<_, String>(
        "SELECT sender_id FROM friend_requests WHERE id=? AND recipient_id=?",
    )
    .bind(request_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if is_blocked_pair(&state.db, user_id, &sender).await? {
        return Err(ApiError::forbidden());
    }
    let (low, high) = ordered(user_id, &sender);
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("INSERT OR IGNORE INTO friendships(user_low,user_high) VALUES(?,?)")
        .bind(low)
        .bind(high)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM friend_requests WHERE id=?")
        .bind(request_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![sender.clone(), user_id.to_owned()],
        event: serde_json::json!({
            "type": "friend_request_accepted",
            "by_user_id": user_id,
            "requester_id": sender,
        }),
    });
    Ok(())
}

pub(crate) async fn accept_friend_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    accept_friend(state, &user.id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn reject_friend_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let result = sqlx::query("DELETE FROM friend_requests WHERE id=? AND recipient_id=?")
        .bind(id)
        .bind(user.id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_friends(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PublicUser>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM friendships f JOIN users u ON u.id=CASE WHEN f.user_low=? THEN f.user_high ELSE f.user_low END WHERE f.user_low=? OR f.user_high=? ORDER BY u.display_name COLLATE NOCASE")
        .bind(&user.id).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

pub(crate) async fn remove_friend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let (low, high) = ordered(&user.id, &id);
    sqlx::query("DELETE FROM friendships WHERE user_low=? AND user_high=?")
        .bind(low)
        .bind(high)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}
