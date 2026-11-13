//! Accounts and sessions: sign-up, recovery, session tokens and authentication.

use super::*;

pub(crate) fn session_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

pub(crate) fn recovery_lookup(secret: &str) -> String {
    session_hash(secret)
}

pub(crate) fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn recovery_verifier(secret: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ApiError::internal())
}

pub(crate) fn verify_recovery(secret: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(secret.as_bytes(), &parsed)
            .is_ok()
    })
}

pub(crate) async fn add_session(db: &SqlitePool, user_id: &str) -> Result<String, ApiError> {
    let token = new_secret();
    sqlx::query("INSERT INTO sessions(token_hash,user_id,expires_at) VALUES(?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now','+30 days'))")
        .bind(session_hash(&token)).bind(user_id).execute(db).await.map_err(|_| ApiError::internal())?;
    Ok(token)
}

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct PublicUser {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) avatar_data: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SessionReply {
    pub(crate) user: PublicUser,
    pub(crate) session_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recovery_token: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct CreateAccount {
    pub(crate) username: Option<String>,
    pub(crate) display_name: String,
    pub(crate) avatar_data: Option<String>,
}

pub(crate) async fn create_account(
    State(state): State<AppState>,
    Json(request): Json<CreateAccount>,
) -> Result<Json<SessionReply>, ApiError> {
    if !allow_rate(&state, "account-creation", 20, Duration::from_secs(60)) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Account creation is temporarily limited. Please try again later.",
        ));
    }
    let display_name = request.display_name.trim();
    if !(2..=32).contains(&display_name.chars().count()) {
        return Err(ApiError::bad("Display name must be 2 to 32 characters."));
    }
    let raw_username = request
        .username
        .as_deref()
        .unwrap_or("")
        .trim()
        .trim_start_matches('@')
        .to_lowercase();
    let username = if raw_username.is_empty() {
        format!(
            "ghost_{}",
            Uuid::new_v4().simple().to_string()[..10].to_owned()
        )
    } else {
        raw_username
    };
    if username.len() < 3
        || username.len() > 24
        || !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(ApiError::bad(
            "Username must be 3 to 24 letters, numbers, or underscores.",
        ));
    }
    let avatar_data = validate_avatar(request.avatar_data)?;
    let id = Uuid::new_v4().to_string();
    let recovery = new_secret();
    let verifier = recovery_verifier(&recovery)?;
    let result = sqlx::query("INSERT INTO users(id,username,display_name,recovery_lookup,recovery_verifier,avatar_data) VALUES(?,?,?,?,?,?)")
        .bind(&id).bind(&username).bind(display_name).bind(recovery_lookup(&recovery)).bind(verifier).bind(&avatar_data).execute(&state.db).await;
    if result.is_err() {
        return Err(ApiError::conflict("That username is already in use."));
    }
    let session_token = add_session(&state.db, &id).await?;
    Ok(Json(SessionReply {
        user: PublicUser {
            id,
            username,
            display_name: display_name.to_owned(),
            avatar_data,
        },
        session_token,
        recovery_token: Some(recovery),
    }))
}

#[derive(Deserialize)]
pub(crate) struct RecoverAccount {
    pub(crate) recovery_token: String,
}

pub(crate) async fn recover_account(
    State(state): State<AppState>,
    Json(request): Json<RecoverAccount>,
) -> Result<Json<SessionReply>, ApiError> {
    if !allow_rate(&state, "account-recovery", 10, Duration::from_secs(60)) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Recovery is temporarily limited. Please try again later.",
        ));
    }
    if request.recovery_token.len() < 32 || request.recovery_token.len() > 128 {
        return Err(ApiError::unauthorized());
    }
    let row = sqlx::query_as::<_, (String, String, String, Option<String>, String)>(
        "SELECT id,username,display_name,avatar_data,recovery_verifier FROM users WHERE recovery_lookup=?",
    )
    .bind(recovery_lookup(&request.recovery_token))
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::unauthorized)?;
    let (id, username, display_name, avatar_data, verifier) = row;
    if !verify_recovery(&request.recovery_token, &verifier) {
        return Err(ApiError::unauthorized());
    }
    let user = PublicUser {
        id,
        username,
        display_name,
        avatar_data,
    };
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM sessions WHERE user_id=?")
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    let token = new_secret();
    sqlx::query("INSERT INTO sessions(token_hash,user_id,expires_at) VALUES(?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now','+30 days'))")
        .bind(session_hash(&token)).bind(&user.id).execute(&mut *tx).await.map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id.clone()],
        event: serde_json::json!({"type":"session_revoked"}),
    });
    Ok(Json(SessionReply {
        user,
        session_token: token,
        recovery_token: None,
    }))
}

pub(crate) async fn authenticate(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<PublicUser, ApiError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(ApiError::unauthorized)?;
    if token.len() < 32 || token.len() > 128 {
        return Err(ApiError::unauthorized());
    }
    sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
        .bind(session_hash(token)).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?.ok_or_else(ApiError::unauthorized)
}

pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let logged_out_session_hash = session_hash(token);
    sqlx::query("DELETE FROM sessions WHERE token_hash=? AND user_id=?")
        .bind(&logged_out_session_hash)
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id],
        event: serde_json::json!({
            "type": "session_revoked",
            "session_hash": logged_out_session_hash,
        }),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn revoke_other_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let current_session_hash = session_hash(token);
    sqlx::query("DELETE FROM sessions WHERE user_id=? AND token_hash<>?")
        .bind(&user.id)
        .bind(&current_session_hash)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id],
        event: serde_json::json!({
            "type": "session_revoked",
            "except_session_hash": current_session_hash,
        }),
    });
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PublicUser>, ApiError> {
    Ok(Json(authenticate(&headers, &state).await?))
}

#[derive(Serialize)]
pub(crate) struct RecoveryKeyReply {
    pub(crate) recovery_token: String,
}

pub(crate) async fn rotate_recovery_key(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RecoveryKeyReply>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !allow_rate(
        &state,
        &format!("recovery-key:{}", user.id),
        5,
        Duration::from_secs(60 * 60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Recovery key changes are temporarily limited. Please try again later.",
        ));
    }
    let recovery_token = new_secret();
    let verifier = recovery_verifier(&recovery_token)?;
    sqlx::query("UPDATE users SET recovery_lookup=?,recovery_verifier=? WHERE id=?")
        .bind(recovery_lookup(&recovery_token))
        .bind(verifier)
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(Json(RecoveryKeyReply { recovery_token }))
}
