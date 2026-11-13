//! Users: presence, profiles and user search.

use super::*;

#[derive(Serialize, FromRow)]
pub(crate) struct PresenceRow {
    pub(crate) user_id: String,
    pub(crate) mode: String,
}

#[derive(Deserialize)]
pub(crate) struct UpdatePresence {
    pub(crate) mode: String,
}

pub(crate) async fn list_presence(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PresenceRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, PresenceRow>(
        "SELECT u.id AS user_id, COALESCE(p.mode, 'online') AS mode FROM users u LEFT JOIN user_presence p ON p.user_id=u.id WHERE u.id=? OR EXISTS(SELECT 1 FROM friendships f WHERE (f.user_low=? AND f.user_high=u.id) OR (f.user_high=? AND f.user_low=u.id))",
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let connected = state
        .online_users
        .lock()
        .map(|users| users.clone())
        .unwrap_or_default();
    let rows = rows
        .into_iter()
        .map(|mut row| {
            // A remembered account session is not an online application. Friends
            // see offline until at least one authenticated WebSocket is alive.
            // Invisible must look exactly like offline to everyone else.
            if row.user_id != user.id
                && (connected.get(&row.user_id).copied().unwrap_or(0) == 0
                    || row.mode == "invisible")
            {
                row.mode = "offline".into();
            }
            row
        })
        .collect();
    Ok(Json(rows))
}

pub(crate) fn register_online_connection(state: &AppState, user_id: &str) -> bool {
    let Ok(mut users) = state.online_users.lock() else {
        return false;
    };
    let count = users.entry(user_id.to_owned()).or_insert(0);
    *count += 1;
    *count == 1
}

pub(crate) fn unregister_online_connection(state: &AppState, user_id: &str) -> bool {
    let Ok(mut users) = state.online_users.lock() else {
        return false;
    };
    let Some(count) = users.get_mut(user_id) else {
        return false;
    };
    *count = count.saturating_sub(1);
    if *count == 0 {
        users.remove(user_id);
        true
    } else {
        false
    }
}

/// Connect/disconnect transition. For an invisible account friends already
/// see offline, so they get no event (its timing would reveal the transition).
pub(crate) async fn broadcast_current_presence(state: &AppState, user_id: &str) {
    broadcast_presence(state, user_id, false).await;
}

pub(crate) async fn broadcast_presence(
    state: &AppState,
    user_id: &str,
    explicit_mode_change: bool,
) {
    let saved_mode =
        sqlx::query_scalar::<_, String>("SELECT mode FROM user_presence WHERE user_id=?")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "online".into());
    let connected = state
        .online_users
        .lock()
        .ok()
        .and_then(|users| users.get(user_id).copied())
        .unwrap_or(0)
        > 0;
    let notify_friends = explicit_mode_change || saved_mode != "invisible";
    let mode = if connected {
        saved_mode
    } else {
        "offline".into()
    };
    let friends = sqlx::query_scalar::<_, String>("SELECT CASE WHEN user_low=? THEN user_high ELSE user_low END FROM friendships WHERE user_low=? OR user_high=?")
        .bind(user_id).bind(user_id).bind(user_id).fetch_all(&state.db).await.unwrap_or_default();
    // Invisible must look exactly like offline to friends; only the account
    // itself learns its saved mode.
    let friend_mode = if mode == "invisible" {
        "offline".to_owned()
    } else {
        mode.clone()
    };
    if notify_friends {
        let _ = state.events.send(ServerEvent {
            recipients: friends,
            event: serde_json::json!({"type":"presence_updated","user_id":user_id,"mode":friend_mode}),
        });
    }
    let _ = state.events.send(ServerEvent {
        recipients: vec![user_id.to_owned()],
        event: serde_json::json!({"type":"presence_updated","user_id":user_id,"mode":mode}),
    });
}

pub(crate) async fn update_presence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdatePresence>,
) -> Result<Json<PresenceRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mode = request.mode.trim().to_ascii_lowercase();
    if !matches!(mode.as_str(), "online" | "invisible" | "dnd") {
        return Err(ApiError::bad("Presence must be online, invisible, or dnd."));
    }
    sqlx::query("INSERT INTO user_presence(user_id,mode,updated_at) VALUES(?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now')) ON CONFLICT(user_id) DO UPDATE SET mode=excluded.mode,updated_at=excluded.updated_at")
        .bind(&user.id)
        .bind(&mode)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    broadcast_presence(&state, &user.id, true).await;
    Ok(Json(PresenceRow {
        user_id: user.id,
        mode,
    }))
}

#[derive(Deserialize)]
pub(crate) struct UpdateProfile {
    pub(crate) display_name: String,
    pub(crate) username: Option<String>,
    pub(crate) avatar_data: Option<String>,
}

pub(crate) fn validate_avatar(value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else { return Ok(None) };
    let (encoded, kind) = if let Some(encoded) = value.strip_prefix("data:image/jpeg;base64,") {
        (encoded, "jpeg")
    } else if let Some(encoded) = value.strip_prefix("data:image/gif;base64,") {
        (encoded, "gif")
    } else if let Some(encoded) = value.strip_prefix("data:image/webp;base64,") {
        (encoded, "webp")
    } else {
        return Err(ApiError::bad(
            "Profile photo must be a JPEG, GIF, or WebP image.",
        ));
    };
    if value.len() > 120_000 {
        return Err(ApiError::bad("Profile photo must be under 80 KB."));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::bad("Profile photo is not valid."))?;
    let valid = match kind {
        "jpeg" => {
            bytes.len() <= 32_000
                && bytes.starts_with(&[0xff, 0xd8, 0xff])
                && bytes.ends_with(&[0xff, 0xd9])
        }
        "gif" => {
            bytes.len() <= 80_000 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
        }
        "webp" => {
            bytes.len() <= 80_000
                && bytes.len() >= 12
                && &bytes[0..4] == b"RIFF"
                && &bytes[8..12] == b"WEBP"
        }
        _ => false,
    };
    if !valid {
        return Err(ApiError::bad(
            "Profile photo is invalid or exceeds the 80 KB limit.",
        ));
    }
    Ok(Some(value))
}

pub(crate) async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateProfile>,
) -> Result<Json<PublicUser>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let display_name = request.display_name.trim();
    if !(2..=32).contains(&display_name.chars().count()) {
        return Err(ApiError::bad("Display name must be 2 to 32 characters."));
    }
    let username = request
        .username
        .as_deref()
        .map(|value| value.trim().trim_start_matches('@').to_lowercase())
        .unwrap_or_else(|| user.username.clone());
    if username.len() < 3
        || username.len() > 24
        || !username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(ApiError::bad(
            "Username must be 3 to 24 letters, numbers, or underscores.",
        ));
    }
    let avatar_data = validate_avatar(request.avatar_data)?;
    sqlx::query("UPDATE users SET username=?,display_name=?,avatar_data=? WHERE id=?")
        .bind(&username)
        .bind(display_name)
        .bind(&avatar_data)
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .is_some_and(|database_error| database_error.is_unique_violation())
            {
                ApiError::conflict("That username is already in use.")
            } else {
                ApiError::internal()
            }
        })?;
    let friends = sqlx::query_scalar::<_, String>(
        "SELECT CASE WHEN user_low=? THEN user_high ELSE user_low END FROM friendships WHERE user_low=? OR user_high=?",
    )
    .bind(&user.id).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    if !friends.is_empty() {
        let _ = state.events.send(ServerEvent {
            recipients: friends,
            event: serde_json::json!({"type":"profile_updated","user_id":user.id}),
        });
    }
    Ok(Json(PublicUser {
        username,
        display_name: display_name.to_owned(),
        avatar_data,
        ..user
    }))
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    pub(crate) q: String,
}

pub(crate) async fn search_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<PublicUser>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let q = query.q.trim().trim_start_matches('@');
    if q.len() < 2 || q.len() > 64 {
        return Ok(Json(vec![]));
    }
    // Escape LIKE wildcards so `%` and `_` match only themselves.
    let pattern = format!(
        "{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows = sqlx::query_as::<_, PublicUser>("SELECT id,username,display_name,avatar_data FROM users WHERE (id LIKE ? ESCAPE '\\' OR username LIKE ? ESCAPE '\\') AND NOT EXISTS (SELECT 1 FROM user_blocks b WHERE (b.blocker_id=? AND b.blocked_id=users.id) OR (b.blocker_id=users.id AND b.blocked_id=?)) ORDER BY username LIMIT 20")
        .bind(&pattern).bind(&pattern).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(
        rows.into_iter().filter(|row| row.id != user.id).collect(),
    ))
}
