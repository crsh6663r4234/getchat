use std::{
    collections::{HashMap, VecDeque},
    env,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use tokio::sync::broadcast;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer};
use uuid::Uuid;

const MESSAGE_LIMIT: usize = 4_000;
const PAGE_SIZE: i64 = 50;
const TURN_CREDENTIAL_TTL_SECONDS: u64 = 12 * 60 * 60;

#[derive(Clone)]
pub struct AppState {
    db: SqlitePool,
    events: broadcast::Sender<ServerEvent>,
    limiter: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    voice: VoiceServerConfig,
}

#[derive(Clone)]
struct VoiceServerConfig {
    stun_urls: Vec<String>,
    turn_urls: Vec<String>,
    turn_shared_secret: Option<String>,
}

#[derive(Clone, Serialize)]
struct ServerEvent {
    recipients: Vec<String>,
    event: serde_json::Value,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: &'static str,
}

impl ApiError {
    fn new(status: StatusCode, message: &'static str) -> Self {
        Self { status, message }
    }
    fn bad(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Invalid or expired session.")
    }
    fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "You do not have access to this resource.",
        )
    }
    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "The requested item was not found.")
    }
    fn conflict(message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }
    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The server could not complete this request.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"error": self.message})),
        )
            .into_response()
    }
}

pub async fn open_database(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    Ok(pool)
}

pub async fn build_state(db: SqlitePool) -> AppState {
    let (events, _) = broadcast::channel(256);
    let stun_urls = env::var("GHOSTCHAT_STUN_URLS")
        .ok()
        .map(|value| parse_server_urls(&value, &["stun:", "stuns:"]))
        .filter(|urls| !urls.is_empty())
        .unwrap_or_else(|| vec!["stun:stun.l.google.com:19302".into()]);
    let turn_urls = env::var("GHOSTCHAT_TURN_URLS")
        .ok()
        .map(|value| parse_server_urls(&value, &["turn:", "turns:"]))
        .unwrap_or_default();
    let turn_shared_secret = env::var("GHOSTCHAT_TURN_SHARED_SECRET")
        .ok()
        .filter(|secret| !secret.trim().is_empty());
    AppState {
        db,
        events,
        limiter: Arc::new(Mutex::new(HashMap::new())),
        voice: VoiceServerConfig {
            stun_urls,
            turn_urls,
            turn_shared_secret,
        },
    }
}

fn parse_server_urls(value: &str, allowed_schemes: &[&str]) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|url| allowed_schemes.iter().any(|scheme| url.starts_with(scheme)))
        .map(str::to_owned)
        .collect()
}

pub fn router(state: AppState) -> Router {
    let origins = [
        HeaderValue::from_static("tauri://localhost"),
        HeaderValue::from_static("http://tauri.localhost"),
        HeaderValue::from_static("http://localhost:1420"),
    ];
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok","service":"ghostchat"})) }),
        )
        .route("/v1/accounts", post(create_account))
        .route("/v1/sessions/recover", post(recover_account))
        .route("/v1/sessions", delete(logout))
        .route("/v1/me", get(me))
        .route("/v1/data/export", get(export_data))
        .route("/v1/me/account", delete(delete_account))
        .route("/v1/users/search", get(search_users))
        .route("/v1/friends", get(list_friends).post(send_friend_request))
        .route("/v1/friends/requests", get(pending_requests))
        .route(
            "/v1/friends/requests/:id/accept",
            post(accept_friend_request),
        )
        .route(
            "/v1/friends/requests/:id/reject",
            post(reject_friend_request),
        )
        .route("/v1/friends/:id", delete(remove_friend))
        .route("/v1/conversations", get(list_conversations))
        .route("/v1/conversations/direct", post(create_direct_conversation))
        .route(
            "/v1/conversations/:id/messages",
            get(list_messages).post(send_message),
        )
        .route(
            "/v1/messages/:id",
            patch(edit_message).delete(delete_message),
        )
        .route("/v1/messages/search", get(search_messages))
        .route("/v1/voice/ice", get(voice_ice_servers))
        .route("/v1/ws", get(websocket))
        .fallback(|| async { ApiError::not_found() })
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(cors)
        .with_state(state)
}

pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
}

fn session_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn recovery_lookup(secret: &str) -> String {
    session_hash(secret)
}

fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn recovery_verifier(secret: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ApiError::internal())
}

fn verify_recovery(secret: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(secret.as_bytes(), &parsed)
            .is_ok()
    })
}

async fn add_session(db: &SqlitePool, user_id: &str) -> Result<String, ApiError> {
    let token = new_secret();
    sqlx::query("INSERT INTO sessions(token_hash,user_id,expires_at) VALUES(?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now','+30 days'))")
        .bind(session_hash(&token)).bind(user_id).execute(db).await.map_err(|_| ApiError::internal())?;
    Ok(token)
}

#[derive(Serialize, FromRow, Clone)]
struct PublicUser {
    id: String,
    username: String,
    display_name: String,
}

#[derive(Serialize)]
struct SessionReply {
    user: PublicUser,
    session_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_token: Option<String>,
}

#[derive(Deserialize)]
struct CreateAccount {
    username: Option<String>,
    display_name: String,
}

async fn create_account(
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
    let id = Uuid::new_v4().to_string();
    let recovery = new_secret();
    let verifier = recovery_verifier(&recovery)?;
    let result = sqlx::query("INSERT INTO users(id,username,display_name,recovery_lookup,recovery_verifier) VALUES(?,?,?,?,?)")
        .bind(&id).bind(&username).bind(display_name).bind(recovery_lookup(&recovery)).bind(verifier).execute(&state.db).await;
    if result.is_err() {
        return Err(ApiError::conflict("That username is already in use."));
    }
    let session_token = add_session(&state.db, &id).await?;
    Ok(Json(SessionReply {
        user: PublicUser {
            id,
            username,
            display_name: display_name.to_owned(),
        },
        session_token,
        recovery_token: Some(recovery),
    }))
}

#[derive(Deserialize)]
struct RecoverAccount {
    recovery_token: String,
}

async fn recover_account(
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
    let row = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT id,username,display_name,recovery_verifier FROM users WHERE recovery_lookup=?",
    )
    .bind(recovery_lookup(&request.recovery_token))
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::unauthorized)?;
    let (id, username, display_name, verifier) = row;
    if !verify_recovery(&request.recovery_token, &verifier) {
        return Err(ApiError::unauthorized());
    }
    let user = PublicUser {
        id,
        username,
        display_name,
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

async fn authenticate(headers: &HeaderMap, state: &AppState) -> Result<PublicUser, ApiError> {
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
    sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
        .bind(session_hash(token)).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?.ok_or_else(ApiError::unauthorized)
}

#[derive(Serialize)]
struct IceServer {
    urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<String>,
}

async fn voice_ice_servers(
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

    Ok(Json(
        serde_json::json!({"ice_servers": ice_servers, "turn_available": turn_available}),
    ))
}

fn allow_rate(state: &AppState, key: &str, count: usize, period: Duration) -> bool {
    let now = Instant::now();
    let mut limiter = match state.limiter.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    };
    let entries = limiter.entry(key.to_owned()).or_default();
    while entries
        .front()
        .is_some_and(|instant| now.duration_since(*instant) > period)
    {
        entries.pop_front();
    }
    if entries.len() >= count {
        return false;
    }
    entries.push_back(now);
    true
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    sqlx::query("DELETE FROM sessions WHERE token_hash=? AND user_id=?")
        .bind(session_hash(token))
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id],
        event: serde_json::json!({"type":"session_revoked"}),
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PublicUser>, ApiError> {
    Ok(Json(authenticate(&headers, &state).await?))
}

#[derive(Serialize)]
struct ExportConversation {
    conversation: ConversationRow,
    messages: Vec<MessageRow>,
}

#[derive(Serialize)]
struct DataExport {
    exported_at: String,
    account: PublicUser,
    friends: Vec<PublicUser>,
    conversations: Vec<ExportConversation>,
}

async fn export_data(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DataExport>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let friends = sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name FROM friendships f JOIN users u ON u.id=CASE WHEN f.user_low=? THEN f.user_high ELSE f.user_low END WHERE f.user_low=? OR f.user_high=? ORDER BY u.display_name COLLATE NOCASE")
        .bind(&user.id).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
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
        let messages = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? ORDER BY m.rowid")
            .bind(&id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
        conversations.push(ExportConversation {
            conversation,
            messages,
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
        conversations,
    }))
}

async fn delete_account(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM conversations WHERE id IN (SELECT conversation_id FROM conversation_members WHERE user_id=?)")
        .bind(&user.id).execute(&mut *tx).await.map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM users WHERE id=?")
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent {
        recipients: vec![user.id],
        event: serde_json::json!({"type":"session_revoked"}),
    });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<PublicUser>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let q = query.q.trim().trim_start_matches('@');
    if q.len() < 2 || q.len() > 64 {
        return Ok(Json(vec![]));
    }
    let rows = sqlx::query_as::<_, PublicUser>("SELECT id,username,display_name FROM users WHERE id LIKE ? OR username LIKE ? ORDER BY username LIMIT 20")
        .bind(format!("{q}%")).bind(format!("{q}%")).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(
        rows.into_iter().filter(|row| row.id != user.id).collect(),
    ))
}

#[derive(Serialize, FromRow)]
struct FriendRequestRow {
    id: String,
    created_at: String,
    user_id: String,
    username: String,
    display_name: String,
}

async fn send_friend_request(
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
    sqlx::query("INSERT INTO friend_requests(id,sender_id,recipient_id) VALUES(?,?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(user.id)
        .bind(target)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::conflict("A request is already pending."))?;
    Ok(StatusCode::NO_CONTENT)
}

fn ordered(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

async fn pending_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<FriendRequestRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, FriendRequestRow>("SELECT r.id,r.created_at,u.id AS user_id,u.username,u.display_name FROM friend_requests r JOIN users u ON u.id=r.sender_id WHERE r.recipient_id=? ORDER BY r.created_at DESC")
        .bind(user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

async fn accept_friend(state: AppState, user_id: &str, request_id: &str) -> Result<(), ApiError> {
    let sender = sqlx::query_scalar::<_, String>(
        "SELECT sender_id FROM friend_requests WHERE id=? AND recipient_id=?",
    )
    .bind(request_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
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
    Ok(())
}

async fn accept_friend_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    accept_friend(state, &user.id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reject_friend_request(
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

async fn list_friends(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PublicUser>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let rows = sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name FROM friendships f JOIN users u ON u.id=CASE WHEN f.user_low=? THEN f.user_high ELSE f.user_low END WHERE f.user_low=? OR f.user_high=? ORDER BY u.display_name COLLATE NOCASE")
        .bind(&user.id).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

async fn remove_friend(
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

#[derive(Deserialize)]
struct DirectConversationRequest {
    user_id: String,
}

#[derive(Serialize, FromRow)]
struct ConversationRow {
    id: String,
    peer_id: String,
    username: String,
    display_name: String,
    last_message: Option<String>,
    last_at: Option<String>,
}

async fn create_direct_conversation(
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
    let existing = sqlx::query_scalar::<_, String>("SELECT a.conversation_id FROM conversation_members a JOIN conversation_members b ON b.conversation_id=a.conversation_id WHERE a.user_id=? AND b.user_id=? LIMIT 1")
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

async fn conversation_row(
    db: &SqlitePool,
    id: &str,
    user_id: &str,
) -> Result<ConversationRow, ApiError> {
    sqlx::query_as::<_, ConversationRow>("SELECT c.id,u.id AS peer_id,u.username,u.display_name,(SELECT content FROM messages m WHERE m.conversation_id=c.id ORDER BY m.rowid DESC LIMIT 1) AS last_message,(SELECT created_at FROM messages m WHERE m.conversation_id=c.id ORDER BY m.rowid DESC LIMIT 1) AS last_at FROM conversations c JOIN conversation_members a ON a.conversation_id=c.id JOIN conversation_members b ON b.conversation_id=c.id JOIN users u ON u.id=b.user_id WHERE c.id=? AND a.user_id=? AND b.user_id<>? LIMIT 1")
        .bind(id).bind(user_id).bind(user_id).fetch_optional(db).await.map_err(|_| ApiError::internal())?.ok_or_else(ApiError::not_found)
}

async fn list_conversations(
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

#[derive(Serialize, FromRow, Clone)]
struct MessageRow {
    sequence: i64,
    id: String,
    conversation_id: String,
    sender_id: String,
    sender_name: String,
    client_message_id: String,
    content: String,
    created_at: String,
    edited_at: Option<String>,
}

async fn is_member(
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

async fn list_messages(
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
        sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.rowid<? ORDER BY m.rowid DESC LIMIT ?")
            .bind(&id).bind(before).bind(limit).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?
    } else {
        sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? ORDER BY m.rowid DESC LIMIT ?")
            .bind(&id).bind(limit).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?
    };
    Ok(Json(rows.into_iter().rev().collect()))
}

#[derive(Deserialize)]
struct SendMessage {
    client_message_id: String,
    content: String,
}

async fn send_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<SendMessage>,
) -> Result<Json<MessageRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
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
    if body.client_message_id.len() > 64 || body.client_message_id.is_empty() {
        return Err(ApiError::bad("Invalid message ID."));
    }
    let id = Uuid::new_v4().to_string();
    let inserted = sqlx::query("INSERT OR IGNORE INTO messages(id,conversation_id,sender_id,client_message_id,content) VALUES(?,?,?,?,?)")
        .bind(&id).bind(&conversation_id).bind(&user.id).bind(&body.client_message_id).bind(content).execute(&state.db).await.map_err(|_| ApiError::internal())?;
    let message = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.sender_id=? AND m.client_message_id=?")
        .bind(&conversation_id).bind(&user.id).bind(&body.client_message_id).fetch_one(&state.db).await.map_err(|_| ApiError::internal())?;
    if inserted.rows_affected() > 0 {
        publish_message(&state, &conversation_id, &message).await?;
    }
    Ok(Json(message))
}

async fn publish_message(
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
struct EditMessage {
    content: String,
}

async fn edit_message(
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
    let msg = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.id=?")
        .bind(&id).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?.ok_or_else(ApiError::not_found)?;
    if msg.sender_id != user.id {
        return Err(ApiError::forbidden());
    }
    sqlx::query(
        "UPDATE messages SET content=?,edited_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?",
    )
    .bind(content)
    .bind(&id)
    .execute(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let updated = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.id=?")
        .bind(&id).fetch_one(&state.db).await.map_err(|_| ApiError::internal())?;
    publish_message(&state, &updated.conversation_id, &updated).await?;
    Ok(Json(updated))
}

async fn delete_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let msg = sqlx::query_as::<_, (String, String)>(
        "SELECT conversation_id,sender_id FROM messages WHERE id=?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if msg.1 != user.id {
        return Err(ApiError::forbidden());
    }
    sqlx::query("DELETE FROM messages WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
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

async fn search_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<MessageRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let q = query.get("q").map(|v| v.trim()).unwrap_or("");
    if q.len() < 2 || q.len() > 128 {
        return Ok(Json(vec![]));
    }
    let rows = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,m.client_message_id,m.content,m.created_at,m.edited_at FROM messages m JOIN users u ON u.id=m.sender_id JOIN conversation_members c ON c.conversation_id=m.conversation_id WHERE c.user_id=? AND m.content LIKE ? ORDER BY m.rowid DESC LIMIT 50")
        .bind(user.id).bind(format!("%{q}%")).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct WsAuth {
    token: String,
}

async fn websocket(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(32 * 1024)
        .max_frame_size(32 * 1024)
        .on_upgrade(move |socket| websocket_session(socket, state))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WsVoiceSignal {
    #[serde(rename = "type")]
    message_type: String,
    conversation_id: String,
    to_user_id: String,
    kind: String,
    payload: serde_json::Value,
}

async fn relay_voice_signal(state: &AppState, sender: &PublicUser, text: &str) {
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
        "hangup" => signal.payload.is_null(),
        _ => false,
    };
    if !valid_payload {
        return;
    }

    let peer_id = match sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? AND user_id<>? LIMIT 1",
    )
    .bind(&signal.conversation_id)
    .bind(&sender.id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(peer_id)) if peer_id == signal.to_user_id => peer_id,
        _ => return,
    };

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

async fn websocket_session(mut socket: WebSocket, state: AppState) {
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
    let user = match sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
        .bind(session_hash(&token)).fetch_optional(&state.db).await { Ok(Some(user)) => user, _ => { let _ = socket.close().await; return; } };
    if socket
        .send(WsMessage::Text(
            serde_json::json!({"type":"ready"}).to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let mut events = state.events.subscribe();
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(WsMessage::Close(_))) => break,
                None | Some(Err(_)) => break,
                Some(Ok(WsMessage::Ping(bytes))) => if socket.send(WsMessage::Pong(bytes)).await.is_err() { break; },
                Some(Ok(WsMessage::Text(text))) => relay_voice_signal(&state, &user, &text).await,
                _ => {}
            },
            event = events.recv() => match event {
                Ok(event) if event.recipients.iter().any(|id| id == &user.id) && event.event["type"] == "session_revoked" => {
                    let _ = socket.send(WsMessage::Close(None)).await;
                    break;
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
}

pub async fn bind_default(
    addr: SocketAddr,
    database_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db = open_database(database_path).await?;
    let state = build_state(db).await;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(address = %listener.local_addr()?, "GhostChat server listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_secrets_are_salted_and_verified() {
        let secret = new_secret();
        let verifier_one = recovery_verifier(&secret).unwrap();
        let verifier_two = recovery_verifier(&secret).unwrap();
        assert_ne!(
            verifier_one, verifier_two,
            "password hashes must use independent salts"
        );
        assert!(verify_recovery(&secret, &verifier_one));
        assert!(!verify_recovery("not-the-secret", &verifier_one));
        assert!(!verifier_one.contains(&secret));
    }

    #[test]
    fn session_lookup_is_stable_one_way_and_fixed_length() {
        let token = new_secret();
        let hash = session_hash(&token);
        assert_eq!(hash, session_hash(&token));
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, token);
    }

    #[test]
    fn friendship_keys_are_ordered_independently_of_request_direction() {
        assert_eq!(
            ordered("z-user", "a-user"),
            ("a-user".into(), "z-user".into())
        );
        assert_eq!(
            ordered("a-user", "z-user"),
            ("a-user".into(), "z-user".into())
        );
    }
}
