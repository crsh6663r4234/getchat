use std::{
    collections::{HashMap, VecDeque},
    env,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, Query, State,
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
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    sync::broadcast,
};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

const MESSAGE_LIMIT: usize = 4_000;
const MAX_CRYPTO_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const ENCRYPTED_ATTACHMENT_CHUNK_BYTES: u64 = 1024 * 1024;
const ATTACHMENT_DOWNLOAD_TTL_SECONDS: u64 = 30 * 60;
const ATTACHMENT_MARKER: &str = "file:";
const PAGE_SIZE: i64 = 50;
const TURN_CREDENTIAL_TTL_SECONDS: u64 = 12 * 60 * 60;

#[derive(Clone)]
pub struct AppState {
    db: SqlitePool,
    attachment_dir: PathBuf,
    attachment_ticket_key: [u8; 32],
    events: broadcast::Sender<ServerEvent>,
    limiter: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    voice: VoiceServerConfig,
}

#[derive(Clone)]
struct VoiceServerConfig {
    stun_urls: Vec<String>,
    turn_urls: Vec<String>,
    turn_shared_secret: Option<String>,
    livekit_url: Option<String>,
    livekit_api_key: Option<String>,
    livekit_api_secret: Option<String>,
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
    fn too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Files must be 5 GB or smaller.",
        )
    }
    fn crypto_payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Cryptographic payloads must be 64 KB or smaller.",
        )
    }
    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The server could not complete this request.",
        )
    }
    fn unavailable(message: &'static str) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
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

fn validate_crypto_payload(payload: &str) -> Result<(), ApiError> {
    if payload.is_empty() || payload.len() > MAX_CRYPTO_PAYLOAD_BYTES * 4 / 3 + 8 {
        return Err(ApiError::crypto_payload_too_large());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|_| ApiError::bad("Encrypted events must use URL-safe base64."))?;
    if decoded.is_empty() || decoded.len() > MAX_CRYPTO_PAYLOAD_BYTES {
        return Err(ApiError::crypto_payload_too_large());
    }
    Ok(())
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
    let attachment_dir = env::var_os("GHOSTCHAT_ATTACHMENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ghostchat-attachments"));
    build_state_with_attachment_dir(db, attachment_dir).await
}

pub async fn build_state_with_attachment_dir(db: SqlitePool, attachment_dir: PathBuf) -> AppState {
    let (events, _) = broadcast::channel(256);
    let mut attachment_ticket_key = [0u8; 32];
    OsRng.fill_bytes(&mut attachment_ticket_key);
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
    let livekit_url = env::var("GHOSTCHAT_LIVEKIT_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| {
            url.starts_with("wss://")
                || url.starts_with("ws://localhost:")
                || url.starts_with("ws://127.0.0.1:")
        });
    let livekit_api_key = env::var("GHOSTCHAT_LIVEKIT_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty());
    let livekit_api_secret = env::var("GHOSTCHAT_LIVEKIT_API_SECRET")
        .ok()
        .filter(|secret| !secret.trim().is_empty());
    AppState {
        db,
        attachment_dir,
        attachment_ticket_key,
        events,
        limiter: Arc::new(Mutex::new(HashMap::new())),
        voice: VoiceServerConfig {
            stun_urls,
            turn_urls,
            turn_shared_secret,
            livekit_url,
            livekit_api_key,
            livekit_api_secret,
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
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::RANGE])
        .expose_headers([
            header::CONTENT_DISPOSITION,
            header::CONTENT_RANGE,
            header::ACCEPT_RANGES,
        ]);

    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok","service":"ghostchat"})) }),
        )
        .route("/v1/accounts", post(create_account))
        .route("/v1/sessions/recover", post(recover_account))
        .route("/v1/sessions", delete(logout))
        .route("/v1/me/sessions/others", delete(revoke_other_sessions))
        .route("/v1/me", get(me))
        .route("/v1/me/profile", patch(update_profile))
        .route("/v1/me/recovery-key", post(rotate_recovery_key))
        .route(
            "/v1/me/crypto/devices",
            get(list_crypto_devices).post(register_crypto_device),
        )
        .route("/v1/me/crypto/devices/:id", delete(revoke_crypto_device))
        .route(
            "/v1/me/crypto/devices/:id/key-packages",
            post(upload_crypto_key_package),
        )
        .route(
            "/v1/users/:id/crypto/key-package",
            post(claim_crypto_key_package),
        )
        .route("/v1/data/export", get(export_data))
        .route("/v1/me/account", delete(delete_account))
        .route("/v1/me/blocks", get(list_blocked_users))
        .route("/v1/users/:id/block", post(block_user).delete(unblock_user))
        .route("/v1/users/:id/report", post(report_user))
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
        .route("/v1/conversations/groups", post(create_group_conversation))
        .route(
            "/v1/conversations/:id/members",
            get(list_group_members).post(add_group_members),
        )
        .route(
            "/v1/conversations/:id/members/:user_id",
            patch(change_group_role).delete(remove_group_member),
        )
        .route(
            "/v1/conversations/:id/crypto-membership/remove/:user_id",
            post(remove_encrypted_group_member),
        )
        .route(
            "/v1/conversations/:id/crypto-membership/leave",
            post(request_encrypted_group_leave),
        )
        .route(
            "/v1/conversations/:id/crypto-membership/leave/commit/:user_id",
            post(commit_encrypted_group_leave),
        )
        .route("/v1/conversations/:id/group", patch(rename_group))
        .route(
            "/v1/conversations/:id/crypto-lock",
            get(get_group_crypto_lock).post(lock_group_crypto_roster),
        )
        .route(
            "/v1/conversations/:id/crypto-lock/complete",
            post(complete_group_crypto_setup),
        )
        .route(
            "/v1/conversations/:id/invites",
            get(list_group_invites).post(create_group_invite),
        )
        .route(
            "/v1/conversations/:id/invites/:invite_id",
            delete(revoke_group_invite),
        )
        .route("/v1/group-invites/join", post(join_group_invite))
        .route(
            "/v1/conversations/:id/reactions",
            get(list_conversation_reactions),
        )
        .route("/v1/conversations/:id/read", post(mark_conversation_read))
        .route("/v1/conversations/:id/attachments", post(upload_attachment))
        .route(
            "/v1/conversations/:id/encrypted-attachments",
            post(upload_encrypted_attachment),
        )
        .route("/v1/attachments/:id/link", post(create_attachment_link))
        .route(
            "/v1/attachments/:id/content",
            get(download_attachment_ticket),
        )
        .route("/v1/attachments/:id", get(download_attachment))
        .route(
            "/v1/conversations/:id/messages",
            get(list_messages).post(send_message),
        )
        .route(
            "/v1/conversations/:id/crypto-events",
            get(list_crypto_events).post(send_crypto_event),
        )
        .route(
            "/v1/messages/:id",
            patch(edit_message).delete(delete_message),
        )
        .route("/v1/messages/:id/reactions", post(toggle_message_reaction))
        .route("/v1/messages/:id/pin", post(toggle_message_pin))
        .route("/v1/messages/search", get(search_messages))
        .route("/v1/voice/ice", get(voice_ice_servers))
        .route("/v1/conversations/:id/voice-token", post(group_voice_token))
        .route("/v1/ws", get(websocket))
        .fallback(|| async { ApiError::not_found() })
        .layer(DefaultBodyLimit::max(96 * 1024))
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
    avatar_data: Option<String>,
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
    avatar_data: Option<String>,
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
    sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
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

    Ok(Json(serde_json::json!({
        "ice_servers": ice_servers,
        "turn_available": turn_available,
        "group_voice_available": state.voice.livekit_url.is_some()
            && state.voice.livekit_api_key.is_some()
            && state.voice.livekit_api_secret.is_some()
    })))
}

async fn group_voice_token(
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
        "exp": now.saturating_add(10 * 60),
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
        "expires_at": now.saturating_add(10 * 60)
    })))
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

async fn revoke_other_sessions(
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

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PublicUser>, ApiError> {
    Ok(Json(authenticate(&headers, &state).await?))
}

#[derive(Deserialize)]
struct UpdateProfile {
    display_name: String,
    username: Option<String>,
    avatar_data: Option<String>,
}

fn validate_avatar(value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > 48_000 || !value.starts_with("data:image/jpeg;base64,") {
        return Err(ApiError::bad("Profile photo must be a small JPEG image."));
    }
    let encoded = value
        .strip_prefix("data:image/jpeg;base64,")
        .ok_or_else(|| ApiError::bad("Profile photo must be a small JPEG image."))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::bad("Profile photo is not valid."))?;
    if bytes.len() > 32_000
        || !bytes.starts_with(&[0xff, 0xd8, 0xff])
        || !bytes.ends_with(&[0xff, 0xd9])
    {
        return Err(ApiError::bad(
            "Profile photo must be a valid JPEG under 32 KB.",
        ));
    }
    Ok(Some(value))
}

async fn update_profile(
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

#[derive(Serialize)]
struct RecoveryKeyReply {
    recovery_token: String,
}

async fn rotate_recovery_key(
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

#[derive(Serialize, FromRow)]
struct CryptoDeviceRow {
    id: String,
    label: String,
    created_at: String,
    last_seen_at: String,
    revoked_at: Option<String>,
}

#[derive(Deserialize)]
struct RegisterCryptoDevice {
    id: String,
    label: String,
}

fn normalize_device_id(id: &str) -> Result<String, ApiError> {
    Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Device IDs must be UUIDs."))
}

fn validate_device_label(label: &str) -> Result<String, ApiError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 32 || label.chars().any(char::is_control) {
        return Err(ApiError::bad(
            "Device names must contain 1 to 32 characters.",
        ));
    }
    Ok(label.to_owned())
}

async fn list_crypto_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CryptoDeviceRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let devices = sqlx::query_as::<_, CryptoDeviceRow>(
        "SELECT id,label,created_at,last_seen_at,revoked_at FROM crypto_devices WHERE user_id=? ORDER BY created_at",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(devices))
}

async fn register_crypto_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterCryptoDevice>,
) -> Result<Json<CryptoDeviceRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let id = normalize_device_id(&body.id)?;
    let label = validate_device_label(&body.label)?;
    let other_active_devices = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM crypto_devices WHERE user_id=? AND revoked_at IS NULL AND id<>?",
    )
    .bind(&user.id)
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if other_active_devices >= 10 {
        return Err(ApiError::conflict(
            "An account can have at most 10 active encryption devices.",
        ));
    }
    let result = sqlx::query(
        "INSERT INTO crypto_devices(id,user_id,label) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET label=excluded.label,last_seen_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE crypto_devices.user_id=excluded.user_id AND crypto_devices.revoked_at IS NULL",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&label)
    .execute(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if result.rows_affected() == 0 {
        return Err(ApiError::conflict(
            "This device ID is already registered or has been revoked.",
        ));
    }
    let device = sqlx::query_as::<_, CryptoDeviceRow>(
        "SELECT id,label,created_at,last_seen_at,revoked_at FROM crypto_devices WHERE id=? AND user_id=?",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(device))
}

async fn revoke_crypto_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let id = normalize_device_id(&raw_id)?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let result = sqlx::query(
        "UPDATE crypto_devices SET revoked_at=COALESCE(revoked_at,strftime('%Y-%m-%dT%H:%M:%fZ','now')) WHERE id=? AND user_id=?",
    )
    .bind(&id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    sqlx::query("DELETE FROM crypto_key_packages WHERE device_id=? AND claimed_at IS NULL")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UploadCryptoKeyPackage {
    package_ref: String,
    payload: String,
}

#[derive(Serialize, FromRow)]
struct CryptoKeyPackageAck {
    package_ref: String,
    created_at: String,
}

async fn upload_crypto_key_package(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_device_id): Path<String>,
    Json(body): Json<UploadCryptoKeyPackage>,
) -> Result<Json<CryptoKeyPackageAck>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let device_id = normalize_device_id(&raw_device_id)?;
    if !allow_rate(
        &state,
        &format!("crypto-package-upload:{}:{device_id}", user.id),
        200,
        Duration::from_secs(60 * 60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Key package uploads are temporarily limited.",
        ));
    }
    if body.package_ref.is_empty()
        || body.package_ref.len() > 128
        || !body
            .package_ref
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApiError::bad("Invalid key package reference."));
    }
    if body.payload.is_empty() || body.payload.len() > MAX_CRYPTO_PAYLOAD_BYTES * 4 / 3 + 8 {
        return Err(ApiError::crypto_payload_too_large());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(body.payload.as_bytes())
        .map_err(|_| ApiError::bad("Key packages must use URL-safe base64."))?;
    if decoded.is_empty() || decoded.len() > MAX_CRYPTO_PAYLOAD_BYTES {
        return Err(ApiError::crypto_payload_too_large());
    }
    let owns_device = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM crypto_devices WHERE id=? AND user_id=? AND revoked_at IS NULL",
    )
    .bind(&device_id)
    .bind(&user.id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if owns_device == 0 {
        return Err(ApiError::not_found());
    }

    if let Some((existing_device, existing_payload, created_at)) =
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT device_id,payload,created_at FROM crypto_key_packages WHERE package_ref=?",
        )
        .bind(&body.package_ref)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
    {
        if existing_device == device_id && existing_payload == body.payload {
            return Ok(Json(CryptoKeyPackageAck {
                package_ref: body.package_ref,
                created_at,
            }));
        }
        return Err(ApiError::conflict(
            "This key package reference is already in use.",
        ));
    }

    sqlx::query("DELETE FROM crypto_key_packages WHERE device_id=? AND datetime(expires_at)<=datetime('now')")
        .bind(&device_id)
        .execute(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let available = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM crypto_key_packages WHERE device_id=? AND claimed_at IS NULL AND datetime(expires_at)>datetime('now')",
    )
    .bind(&device_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if available >= 100 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "This device already has enough unused key packages.",
        ));
    }

    sqlx::query(
        "INSERT OR IGNORE INTO crypto_key_packages(package_ref,device_id,payload) VALUES(?,?,?)",
    )
    .bind(&body.package_ref)
    .bind(&device_id)
    .bind(&body.payload)
    .execute(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    let existing = sqlx::query_as::<_, (String, String)>(
        "SELECT device_id,payload FROM crypto_key_packages WHERE package_ref=?",
    )
    .bind(&body.package_ref)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if existing.0 != device_id || existing.1 != body.payload {
        return Err(ApiError::conflict(
            "This key package reference is already in use.",
        ));
    }
    let ack = sqlx::query_as::<_, CryptoKeyPackageAck>(
        "SELECT package_ref,created_at FROM crypto_key_packages WHERE package_ref=?",
    )
    .bind(&body.package_ref)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(ack))
}

#[derive(Serialize, FromRow)]
struct ClaimedCryptoKeyPackage {
    package_ref: String,
    device_id: String,
    payload: String,
    created_at: String,
}

#[derive(Deserialize)]
struct ClaimCryptoKeyPackage {
    client_claim_id: String,
}

async fn claim_crypto_key_package(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_target_id): Path<String>,
    Json(body): Json<ClaimCryptoKeyPackage>,
) -> Result<Json<ClaimedCryptoKeyPackage>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let target_id = normalize_device_id(raw_target_id.trim())?;
    if body.client_claim_id.is_empty()
        || body.client_claim_id.len() > 64
        || !body
            .client_claim_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApiError::bad("Invalid key package claim ID."));
    }
    if target_id != user.id {
        let (low, high) = ordered(&user.id, &target_id);
        let can_share_key = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM friendships WHERE user_low=? AND user_high=?) OR EXISTS(SELECT 1 FROM conversation_members a JOIN conversation_members b ON b.conversation_id=a.conversation_id WHERE a.user_id=? AND b.user_id=?)",
        )
        .bind(low)
        .bind(high)
        .bind(&user.id)
        .bind(&target_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        if can_share_key == 0 {
            return Err(ApiError::not_found());
        }
    }

    let previous_target = sqlx::query_scalar::<_, String>(
        "SELECT target_user_id FROM crypto_key_package_claims WHERE requester_id=? AND client_claim_id=?",
    )
    .bind(&user.id)
    .bind(&body.client_claim_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if let Some(previous_target) = previous_target {
        if previous_target != target_id {
            return Err(ApiError::conflict(
                "This claim ID was already used for another account.",
            ));
        }
        let previous = sqlx::query_as::<_, ClaimedCryptoKeyPackage>(
            "SELECT p.package_ref,p.device_id,p.payload,p.created_at FROM crypto_key_package_claims c JOIN crypto_key_packages p ON p.package_ref=c.package_ref WHERE c.requester_id=? AND c.client_claim_id=?",
        )
        .bind(&user.id)
        .bind(&body.client_claim_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
        return Ok(Json(previous));
    }
    if !allow_rate(
        &state,
        &format!("crypto-package-claim:{}:{target_id}", user.id),
        20,
        Duration::from_secs(60 * 60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Key package claims are temporarily limited.",
        ));
    }

    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let package = sqlx::query_as::<_, ClaimedCryptoKeyPackage>(
        "UPDATE crypto_key_packages SET claimed_by=?,claimed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE package_ref=(SELECT k.package_ref FROM crypto_key_packages k JOIN crypto_devices d ON d.id=k.device_id WHERE d.user_id=? AND d.revoked_at IS NULL AND k.claimed_at IS NULL AND datetime(k.expires_at)>datetime('now') ORDER BY k.created_at LIMIT 1) AND claimed_at IS NULL AND datetime(expires_at)>datetime('now') RETURNING package_ref,device_id,payload,created_at",
    )
    .bind(&user.id)
    .bind(&target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    let Some(package) = package else {
        tx.rollback().await.map_err(|_| ApiError::internal())?;
        return Err(ApiError::conflict(
            "No fresh key packages are available for this account.",
        ));
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO crypto_key_package_claims(requester_id,client_claim_id,target_user_id,package_ref) VALUES(?,?,?,?)",
    )
    .bind(&user.id)
    .bind(&body.client_claim_id)
    .bind(&target_id)
    .bind(&package.package_ref)
    .execute(&mut *tx)
    .await
    {
        let _ = tx.rollback().await;
        if error
            .as_database_error()
            .is_some_and(|database_error| database_error.is_unique_violation())
        {
            let previous_target = sqlx::query_scalar::<_, String>(
                "SELECT target_user_id FROM crypto_key_package_claims WHERE requester_id=? AND client_claim_id=?",
            )
            .bind(&user.id)
            .bind(&body.client_claim_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
            if let Some(previous_target) = previous_target {
                if previous_target != target_id {
                    return Err(ApiError::conflict(
                        "This claim ID was already used for another account.",
                    ));
                }
                let previous = sqlx::query_as::<_, ClaimedCryptoKeyPackage>(
                    "SELECT p.package_ref,p.device_id,p.payload,p.created_at FROM crypto_key_package_claims c JOIN crypto_key_packages p ON p.package_ref=c.package_ref WHERE c.requester_id=? AND c.client_claim_id=?",
                )
                .bind(&user.id)
                .bind(&body.client_claim_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|_| ApiError::internal())?
                .ok_or_else(ApiError::not_found)?;
                return Ok(Json(previous));
            }
        }
        return Err(ApiError::internal());
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    Ok(Json(package))
}

#[derive(Serialize)]
struct ExportConversation {
    conversation: ConversationRow,
    messages: Vec<MessageRow>,
    reactions: Vec<MessageReactionExport>,
    crypto_events: Vec<CryptoEventRow>,
}

#[derive(Serialize, FromRow)]
struct MessageReactionExport {
    message_id: String,
    user_id: String,
    emoji: String,
    created_at: String,
}

#[derive(Serialize)]
struct DataExport {
    exported_at: String,
    account: PublicUser,
    friends: Vec<PublicUser>,
    blocked_users: Vec<PublicUser>,
    submitted_reports: Vec<UserReportExport>,
    conversations: Vec<ExportConversation>,
}

#[derive(Serialize, FromRow)]
struct UserReportExport {
    target_id: String,
    conversation_id: Option<String>,
    category: String,
    details: String,
    created_at: String,
}

async fn export_data(
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

async fn delete_account(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let attachment_ids = sqlx::query_scalar::<_, String>("SELECT id FROM attachments WHERE stored_on_disk=1 AND (uploaded_by=? OR conversation_id IN (SELECT conversation_id FROM conversation_members WHERE user_id=?))")
        .bind(&user.id)
        .bind(&user.id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM conversations WHERE id IN (SELECT conversation_id FROM conversation_members WHERE user_id=?)")
        .bind(&user.id).execute(&mut *tx).await.map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM users WHERE id=?")
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    for attachment_id in attachment_ids {
        let path = state.attachment_dir.join(format!("{attachment_id}.blob"));
        let _ = tokio::fs::remove_file(path).await;
    }
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
    let rows = sqlx::query_as::<_, PublicUser>("SELECT id,username,display_name,avatar_data FROM users WHERE (id LIKE ? OR username LIKE ?) AND NOT EXISTS (SELECT 1 FROM user_blocks b WHERE (b.blocker_id=? AND b.blocked_id=users.id) OR (b.blocker_id=users.id AND b.blocked_id=?)) ORDER BY username LIMIT 20")
        .bind(format!("{q}%")).bind(format!("{q}%")).bind(&user.id).bind(&user.id).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?;
    Ok(Json(
        rows.into_iter().filter(|row| row.id != user.id).collect(),
    ))
}

async fn is_blocked_pair(db: &SqlitePool, first: &str, second: &str) -> Result<bool, ApiError> {
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
struct BlockedUserRow {
    id: String,
    username: String,
    display_name: String,
    avatar_data: Option<String>,
    created_at: String,
}

async fn list_blocked_users(
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

async fn block_user(
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

async fn unblock_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
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
struct ReportUserRequest {
    category: String,
    #[serde(default)]
    details: String,
    conversation_id: Option<String>,
}

async fn report_user(
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
struct FriendRequestRow {
    id: String,
    created_at: String,
    user_id: String,
    username: String,
    display_name: String,
    avatar_data: Option<String>,
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
    let rows = sqlx::query_as::<_, FriendRequestRow>("SELECT r.id,r.created_at,u.id AS user_id,u.username,u.display_name,u.avatar_data FROM friend_requests r JOIN users u ON u.id=r.sender_id WHERE r.recipient_id=? ORDER BY r.created_at DESC")
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
    let rows = sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM friendships f JOIN users u ON u.id=CASE WHEN f.user_low=? THEN f.user_high ELSE f.user_low END WHERE f.user_low=? OR f.user_high=? ORDER BY u.display_name COLLATE NOCASE")
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
    kind: String,
    title: Option<String>,
    peer_id: String,
    username: String,
    display_name: String,
    avatar_data: Option<String>,
    member_count: i64,
    member_names: Option<String>,
    last_message: Option<String>,
    last_at: Option<String>,
    unread_count: i64,
    peer_read_sequence: Option<i64>,
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
struct GroupConversationRequest {
    name: String,
    member_ids: Vec<String>,
}

async fn create_group_conversation(
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
    member_ids.sort();
    member_ids.dedup();
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

async fn conversation_row(
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

#[derive(Serialize, FromRow)]
struct GroupMemberRow {
    id: String,
    display_name: String,
    username: String,
    avatar_data: Option<String>,
    role: String,
}

async fn group_recipients(db: &SqlitePool, conversation_id: &str) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
    .map_err(|_| ApiError::internal())
}

async fn list_group_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<GroupMemberRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let members = sqlx::query_as::<_, GroupMemberRow>(
        "SELECT u.id,u.display_name,u.username,u.avatar_data,cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id JOIN users u ON u.id=cm.user_id WHERE cm.conversation_id=? AND c.kind='group' AND EXISTS(SELECT 1 FROM conversation_members viewer WHERE viewer.conversation_id=c.id AND viewer.user_id=?) ORDER BY CASE cm.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END,u.display_name COLLATE NOCASE",
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
struct RenameGroupRequest {
    name: String,
}

async fn rename_group(
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

#[derive(Deserialize, Serialize, Clone)]
struct EncryptedGroupMembershipUpdate {
    operation_id: String,
    expected_member_ids: Vec<String>,
    commit_payload: String,
    #[serde(default)]
    welcome_payload: String,
}

#[derive(Deserialize)]
struct EncryptedGroupLeaveRequest {
    operation_id: String,
    expected_member_ids: Vec<String>,
    proposal_payload: String,
}

#[derive(Deserialize)]
struct AddGroupMembersRequest {
    member_ids: Vec<String>,
    crypto_update: Option<EncryptedGroupMembershipUpdate>,
}

async fn add_group_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AddGroupMembersRequest>,
) -> Result<Json<Vec<GroupMemberRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if body.member_ids.is_empty() || body.member_ids.len() > 15 {
        return Err(ApiError::bad("Choose 1 to 15 friends to add."));
    }
    let mut member_ids = body
        .member_ids
        .iter()
        .map(|member_id| {
            Uuid::parse_str(member_id)
                .map(|id| id.to_string())
                .map_err(|_| ApiError::bad("Invalid group member ID."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let requested_count = member_ids.len();
    member_ids.sort();
    member_ids.dedup();
    if member_ids.len() != requested_count {
        return Err(ApiError::bad("A friend can only be selected once."));
    }

    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let actor_role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    let mut crypto_update = body.crypto_update;
    if crypto_locked != 0 {
        let setup_complete = sqlx::query_scalar::<_, i64>(
            "SELECT setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
        )
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?
        .unwrap_or(0);
        if setup_complete == 0 {
            return Err(ApiError::conflict(
                "Finish the encrypted group setup before changing its members.",
            ));
        }
        if crypto_update.is_none() {
            return Err(ApiError::conflict(
                "Encrypted group members must be added with an MLS Commit and Welcome.",
            ));
        }
    } else if crypto_update.is_some() {
        return Err(ApiError::conflict(
            "MLS membership updates require an encrypted group.",
        ));
    }

    if let Some(update) = crypto_update.as_mut() {
        update.operation_id = Uuid::parse_str(&update.operation_id)
            .map(|id| id.to_string())
            .map_err(|_| ApiError::bad("Invalid encrypted membership operation ID."))?;
        update.expected_member_ids = update
            .expected_member_ids
            .iter()
            .map(|member_id| {
                Uuid::parse_str(member_id)
                    .map(|id| id.to_string())
                    .map_err(|_| ApiError::bad("Invalid expected group member ID."))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected_count = update.expected_member_ids.len();
        update.expected_member_ids.sort();
        update.expected_member_ids.dedup();
        if update.expected_member_ids.len() != expected_count {
            return Err(ApiError::bad(
                "The expected group roster contains duplicates.",
            ));
        }
        validate_crypto_payload(&update.commit_payload)?;
        validate_crypto_payload(&update.welcome_payload)?;
    }

    let request_hash = if let Some(update) = crypto_update.as_ref() {
        let serialized = serde_json::to_vec(&(member_ids.as_slice(), update))
            .map_err(|_| ApiError::internal())?;
        Some(format!("{:x}", Sha256::digest(serialized)))
    } else {
        None
    };

    if let (Some(update), Some(request_hash)) = (crypto_update.as_ref(), request_hash.as_deref()) {
        let existing_hash = sqlx::query_scalar::<_, String>(
            "SELECT request_hash FROM crypto_membership_operations WHERE conversation_id=? AND actor_id=? AND operation_id=?",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&update.operation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if let Some(existing_hash) = existing_hash {
            if existing_hash != request_hash {
                return Err(ApiError::conflict(
                    "This encrypted membership operation ID was already used for different data.",
                ));
            }
            tx.rollback().await.map_err(|_| ApiError::internal())?;
            return list_group_members(State(state), headers, Path(id)).await;
        }
    }
    if actor_role != "owner" && actor_role != "admin" {
        return Err(ApiError::forbidden());
    }

    if crypto_locked != 0 {
        let pending_leave_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM crypto_group_leave_requests WHERE conversation_id=? AND resolved_at IS NULL",
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if pending_leave_count != 0 {
            return Err(ApiError::conflict(
                "Resolve the pending encrypted leave request before changing group members.",
            ));
        }
    }

    let current_member_ids = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if let Some(update) = crypto_update.as_ref() {
        if update.expected_member_ids != current_member_ids {
            return Err(ApiError::conflict(
                "The encrypted group membership changed. Refresh the group and try again.",
            ));
        }
        let target_set = member_ids.iter().collect::<std::collections::HashSet<_>>();
        if current_member_ids
            .iter()
            .any(|current| target_set.contains(current))
        {
            return Err(ApiError::conflict(
                "A selected friend is already in this encrypted group.",
            ));
        }
    }
    let group_exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversations WHERE id=? AND kind='group'",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if group_exists == 0 {
        return Err(ApiError::not_found());
    }
    let current_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversation_members WHERE conversation_id=?",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if current_count + member_ids.len() as i64 > 16 {
        return Err(ApiError::bad("Groups can have up to 16 members."));
    }
    for member_id in &member_ids {
        let (low, high) = ordered(&user.id, member_id);
        let is_friend = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM friendships WHERE user_low=? AND user_high=?",
        )
        .bind(low)
        .bind(high)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if is_friend == 0 {
            return Err(ApiError::forbidden());
        }
        let already_member = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_members WHERE conversation_id=? AND user_id=?",
        )
        .bind(&id)
        .bind(member_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if already_member > 0 {
            return Err(ApiError::conflict("That person is already in this group."));
        }
    }
    for member_id in &member_ids {
        sqlx::query(
            "INSERT INTO conversation_members(conversation_id,user_id,role) VALUES(?,?,'member')",
        )
        .bind(&id)
        .bind(member_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    }

    let mut crypto_events = Vec::new();
    if let (Some(update), Some(request_hash)) = (crypto_update.as_ref(), request_hash.as_deref()) {
        for (index, recipient_id) in current_member_ids
            .iter()
            .filter(|member_id| *member_id != &user.id)
            .enumerate()
        {
            let client_event_id = format!("{}-commit-{index}", update.operation_id);
            let event_id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload) VALUES(?,?,?,?,?,?)",
            )
            .bind(&event_id)
            .bind(&id)
            .bind(&user.id)
            .bind(&client_event_id)
            .bind(recipient_id)
            .bind(&update.commit_payload)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            let event = sqlx::query_as::<_, CryptoEventRow>(
                "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sender_id=? AND client_event_id=?",
            )
            .bind(&id)
            .bind(&user.id)
            .bind(&client_event_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            crypto_events.push(event);
        }
        for (index, recipient_id) in member_ids.iter().enumerate() {
            let client_event_id = format!("{}-welcome-{index}", update.operation_id);
            let event_id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload) VALUES(?,?,?,?,?,?)",
            )
            .bind(&event_id)
            .bind(&id)
            .bind(&user.id)
            .bind(&client_event_id)
            .bind(recipient_id)
            .bind(&update.welcome_payload)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            let event = sqlx::query_as::<_, CryptoEventRow>(
                "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sender_id=? AND client_event_id=?",
            )
            .bind(&id)
            .bind(&user.id)
            .bind(&client_event_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            crypto_events.push(event);
        }
        sqlx::query(
            "INSERT INTO crypto_membership_operations(conversation_id,actor_id,operation_id,request_hash) VALUES(?,?,?,?)",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&update.operation_id)
        .bind(request_hash)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    for event in &crypto_events {
        publish_crypto_event(&state, event).await?;
    }
    let recipients = group_recipients(&state.db, &id).await?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    list_group_members(State(state), headers, Path(id)).await
}

#[derive(Deserialize)]
struct ChangeGroupRoleRequest {
    role: String,
}

async fn change_group_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, target_id)): Path<(String, String)>,
    Json(body): Json<ChangeGroupRoleRequest>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !matches!(body.role.as_str(), "owner" | "admin" | "member") {
        return Err(ApiError::bad("Choose owner, admin, or member."));
    }
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let actor_role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if actor_role != "owner" && actor_role != "admin" {
        return Err(ApiError::forbidden());
    }
    let has_pending_encrypted_leave = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?) AND EXISTS(SELECT 1 FROM crypto_group_leave_requests WHERE conversation_id=? AND resolved_at IS NULL)",
    )
    .bind(&id)
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if has_pending_encrypted_leave != 0 {
        return Err(ApiError::conflict(
            "Resolve the pending encrypted leave request before changing group roles.",
        ));
    }
    let target_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM conversation_members WHERE conversation_id=? AND user_id=?",
    )
    .bind(&id)
    .bind(&target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if actor_role != "owner"
        && (target_role != "member" || !matches!(body.role.as_str(), "admin" | "member"))
    {
        return Err(ApiError::forbidden());
    }
    if body.role == "owner" {
        if actor_role != "owner" || target_id == user.id || target_role == "owner" {
            return Err(ApiError::forbidden());
        }
        sqlx::query("UPDATE conversation_members SET role='admin' WHERE conversation_id=? AND user_id=? AND role='owner'")
            .bind(&id).bind(&user.id).execute(&mut *tx).await.map_err(|_| ApiError::internal())?;
        sqlx::query(
            "UPDATE conversation_members SET role='owner' WHERE conversation_id=? AND user_id=?",
        )
        .bind(&id)
        .bind(&target_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    } else {
        if target_role == "owner" || (target_id == user.id && body.role != "owner") {
            return Err(ApiError::bad(
                "Transfer group ownership before changing your own role.",
            ));
        }
        sqlx::query("UPDATE conversation_members SET role=? WHERE conversation_id=? AND user_id=?")
            .bind(&body.role)
            .bind(&id)
            .bind(&target_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let recipients = group_recipients(&state.db, &id).await?;
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn request_encrypted_group_leave(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    Json(mut body): Json<EncryptedGroupLeaveRequest>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let id = Uuid::parse_str(&raw_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid group ID."))?;
    body.operation_id = Uuid::parse_str(&body.operation_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid encrypted leave operation ID."))?;
    body.expected_member_ids = body
        .expected_member_ids
        .iter()
        .map(|member_id| {
            Uuid::parse_str(member_id)
                .map(|id| id.to_string())
                .map_err(|_| ApiError::bad("Invalid expected group member ID."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected_count = body.expected_member_ids.len();
    body.expected_member_ids.sort();
    body.expected_member_ids.dedup();
    if body.expected_member_ids.len() != expected_count || !(2..=16).contains(&expected_count) {
        return Err(ApiError::bad(
            "The expected encrypted group roster is invalid.",
        ));
    }
    if !body.expected_member_ids.contains(&user.id) {
        return Err(ApiError::forbidden());
    }
    validate_crypto_payload(&body.proposal_payload)?;
    let request_hash = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(body.expected_member_ids.as_slice(), &body.proposal_payload))
                .map_err(|_| ApiError::internal())?,
        )
    );

    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    if let Some((existing_hash, resolved_at)) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT request_hash,resolved_at FROM crypto_group_leave_requests WHERE conversation_id=? AND leaver_id=? AND operation_id=?",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&body.operation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    {
        if existing_hash != request_hash {
            return Err(ApiError::conflict(
                "This encrypted leave operation ID was already used for different data.",
            ));
        }
        tx.rollback().await.map_err(|_| ApiError::internal())?;
        if resolved_at.is_none() {
            let event = sqlx::query_as::<_, CryptoEventRow>(
                "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE id=?",
            )
            .bind(&body.operation_id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
            publish_crypto_event(&state, &event).await?;
        }
        return Ok(StatusCode::NO_CONTENT);
    }

    let role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if role == "owner" {
        return Err(ApiError::conflict(
            "Transfer group ownership before requesting to leave.",
        ));
    }
    let setup_complete = sqlx::query_scalar::<_, i64>(
        "SELECT setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .unwrap_or(0);
    if setup_complete == 0 {
        return Err(ApiError::conflict(
            "Finish encrypted group setup before requesting to leave.",
        ));
    }
    let current_member_ids = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if body.expected_member_ids != current_member_ids {
        return Err(ApiError::conflict(
            "The encrypted group membership changed. Refresh the group and try again.",
        ));
    }
    let pending_leave_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM crypto_group_leave_requests WHERE conversation_id=? AND resolved_at IS NULL",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if pending_leave_count != 0 {
        return Err(ApiError::conflict(
            "Another encrypted group leave request is already waiting for the owner.",
        ));
    }

    sqlx::query(
        "INSERT INTO crypto_group_leave_requests(conversation_id,leaver_id,operation_id,request_hash) VALUES(?,?,?,?)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&body.operation_id)
    .bind(&request_hash)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    sqlx::query(
        "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload) VALUES(?,?,?,?,NULL,?)",
    )
    .bind(&body.operation_id)
    .bind(&id)
    .bind(&user.id)
    .bind(&body.operation_id)
    .bind(&body.proposal_payload)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;

    let event = sqlx::query_as::<_, CryptoEventRow>(
        "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE id=?",
    )
    .bind(&body.operation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    publish_crypto_event(&state, &event).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn commit_encrypted_group_leave(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((raw_id, raw_target_id)): Path<(String, String)>,
    Json(mut update): Json<EncryptedGroupMembershipUpdate>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let id = Uuid::parse_str(&raw_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid group ID."))?;
    let target_id = Uuid::parse_str(&raw_target_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid group member ID."))?;
    update.operation_id = Uuid::parse_str(&update.operation_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid encrypted leave operation ID."))?;
    update.expected_member_ids = update
        .expected_member_ids
        .iter()
        .map(|member_id| {
            Uuid::parse_str(member_id)
                .map(|id| id.to_string())
                .map_err(|_| ApiError::bad("Invalid expected group member ID."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected_count = update.expected_member_ids.len();
    update.expected_member_ids.sort();
    update.expected_member_ids.dedup();
    if update.expected_member_ids.len() != expected_count
        || !(2..=16).contains(&expected_count)
        || !update.expected_member_ids.contains(&user.id)
        || !update.expected_member_ids.contains(&target_id)
        || target_id == user.id
    {
        return Err(ApiError::bad(
            "The encrypted group leave roster is invalid.",
        ));
    }
    if !update.welcome_payload.is_empty() {
        return Err(ApiError::bad(
            "An MLS leave commit must not contain a Welcome message.",
        ));
    }
    validate_crypto_payload(&update.commit_payload)?;
    let serialized = serde_json::to_vec(&(
        target_id.as_str(),
        update.expected_member_ids.as_slice(),
        &update.commit_payload,
    ))
    .map_err(|_| ApiError::internal())?;
    let request_hash = format!("{:x}", Sha256::digest(serialized));
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;

    if let Some(existing_hash) = sqlx::query_scalar::<_, String>(
        "SELECT request_hash FROM crypto_membership_operations WHERE conversation_id=? AND actor_id=? AND operation_id=?",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&update.operation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    {
        if existing_hash != request_hash {
            return Err(ApiError::conflict(
                "This encrypted leave operation ID was already used for different data.",
            ));
        }
        tx.rollback().await.map_err(|_| ApiError::internal())?;
        let event_prefix = format!("{}-leave-%", update.operation_id);
        let events = sqlx::query_as::<_, CryptoEventRow>(
            "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sender_id=? AND client_event_id LIKE ? ORDER BY sequence ASC",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(event_prefix)
        .fetch_all(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        for event in &events {
            publish_crypto_event(&state, event).await?;
        }
        let _ = state.events.send(ServerEvent {
            recipients: update.expected_member_ids,
            event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
        });
        return Ok(StatusCode::NO_CONTENT);
    }

    let actor_role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if actor_role != "owner" {
        return Err(ApiError::forbidden());
    }
    let owner_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversation_members WHERE conversation_id=? AND role='owner'",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if owner_count != 1 {
        return Err(ApiError::conflict(
            "The encrypted group must have exactly one owner to approve a leave request.",
        ));
    }
    let target_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM conversation_members WHERE conversation_id=? AND user_id=?",
    )
    .bind(&id)
    .bind(&target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if target_role == "owner" {
        return Err(ApiError::conflict(
            "Transfer group ownership before leaving the encrypted group.",
        ));
    }
    let setup_complete = sqlx::query_scalar::<_, i64>(
        "SELECT setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .unwrap_or(0);
    if setup_complete == 0 {
        return Err(ApiError::conflict(
            "Finish encrypted group setup before changing its members.",
        ));
    }
    let current_member_ids = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if update.expected_member_ids != current_member_ids {
        return Err(ApiError::conflict(
            "The encrypted group membership changed. Refresh the group and try again.",
        ));
    }
    let leave_request_exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM crypto_group_leave_requests WHERE conversation_id=? AND leaver_id=? AND operation_id=? AND resolved_at IS NULL",
    )
    .bind(&id)
    .bind(&target_id)
    .bind(&update.operation_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if leave_request_exists == 0 {
        return Err(ApiError::conflict(
            "The encrypted leave request is missing or has already been handled.",
        ));
    }

    sqlx::query("DELETE FROM conversation_members WHERE conversation_id=? AND user_id=?")
        .bind(&id)
        .bind(&target_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut crypto_events = Vec::new();
    for (index, recipient_id) in current_member_ids
        .iter()
        .filter(|member_id| *member_id != &user.id && *member_id != &target_id)
        .enumerate()
    {
        let client_event_id = format!("{}-leave-{index}", update.operation_id);
        let event_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload) VALUES(?,?,?,?,?,?)",
        )
        .bind(&event_id)
        .bind(&id)
        .bind(&user.id)
        .bind(&client_event_id)
        .bind(recipient_id)
        .bind(&update.commit_payload)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        let event = sqlx::query_as::<_, CryptoEventRow>(
            "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sender_id=? AND client_event_id=?",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&client_event_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        crypto_events.push(event);
    }
    sqlx::query(
        "INSERT INTO crypto_membership_operations(conversation_id,actor_id,operation_id,request_hash) VALUES(?,?,?,?)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&update.operation_id)
    .bind(&request_hash)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    sqlx::query(
        "UPDATE crypto_group_leave_requests SET resolved_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE conversation_id=? AND leaver_id=? AND operation_id=? AND resolved_at IS NULL",
    )
    .bind(&id)
    .bind(&target_id)
    .bind(&update.operation_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;

    if let Err(error) = remove_livekit_participant(&state, &id, &target_id).await {
        tracing::warn!(conversation_id = %id, account_id = %target_id, %error, "could not remove group member from LiveKit");
    }
    for event in &crypto_events {
        publish_crypto_event(&state, event).await?;
    }
    let _ = state.events.send(ServerEvent {
        recipients: current_member_ids,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_encrypted_group_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((raw_id, raw_target_id)): Path<(String, String)>,
    Json(mut update): Json<EncryptedGroupMembershipUpdate>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let id = Uuid::parse_str(&raw_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid group ID."))?;
    let target_id = Uuid::parse_str(&raw_target_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid group member ID."))?;
    update.operation_id = Uuid::parse_str(&update.operation_id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Invalid encrypted membership operation ID."))?;
    update.expected_member_ids = update
        .expected_member_ids
        .iter()
        .map(|member_id| {
            Uuid::parse_str(member_id)
                .map(|id| id.to_string())
                .map_err(|_| ApiError::bad("Invalid expected group member ID."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected_count = update.expected_member_ids.len();
    update.expected_member_ids.sort();
    update.expected_member_ids.dedup();
    if update.expected_member_ids.len() != expected_count {
        return Err(ApiError::bad(
            "The expected group roster contains duplicates.",
        ));
    }
    if !update.welcome_payload.is_empty() {
        return Err(ApiError::bad(
            "An MLS removal must not contain a Welcome message.",
        ));
    }
    validate_crypto_payload(&update.commit_payload)?;
    let serialized =
        serde_json::to_vec(&(target_id.as_str(), &update)).map_err(|_| ApiError::internal())?;
    let request_hash = format!("{:x}", Sha256::digest(serialized));
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;

    if let Some(existing_hash) = sqlx::query_scalar::<_, String>(
        "SELECT request_hash FROM crypto_membership_operations WHERE conversation_id=? AND actor_id=? AND operation_id=?",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&update.operation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    {
        if existing_hash != request_hash {
            return Err(ApiError::conflict(
                "This encrypted membership operation ID was already used for different data.",
            ));
        }
        tx.rollback().await.map_err(|_| ApiError::internal())?;
        return Ok(StatusCode::NO_CONTENT);
    }

    let actor_role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if actor_role != "owner" && actor_role != "admin" {
        return Err(ApiError::forbidden());
    }
    let pending_leave_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM crypto_group_leave_requests WHERE conversation_id=? AND resolved_at IS NULL",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if pending_leave_count != 0 {
        return Err(ApiError::conflict(
            "Resolve the pending encrypted leave request before removing another member.",
        ));
    }
    if target_id == user.id {
        return Err(ApiError::bad(
            "Encrypted group self-leave is not supported by this operation.",
        ));
    }
    let target_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM conversation_members WHERE conversation_id=? AND user_id=?",
    )
    .bind(&id)
    .bind(&target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if target_role == "owner" || (actor_role == "admin" && target_role != "member") {
        return Err(ApiError::forbidden());
    }
    let setup_complete = sqlx::query_scalar::<_, i64>(
        "SELECT setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .unwrap_or(0);
    if setup_complete == 0 {
        return Err(ApiError::conflict(
            "Finish encrypted group setup before changing its members.",
        ));
    }
    let current_member_ids = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if update.expected_member_ids != current_member_ids {
        return Err(ApiError::conflict(
            "The encrypted group membership changed. Refresh the group and try again.",
        ));
    }

    sqlx::query("DELETE FROM conversation_members WHERE conversation_id=? AND user_id=?")
        .bind(&id)
        .bind(&target_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut crypto_events = Vec::new();
    for (index, recipient_id) in current_member_ids
        .iter()
        .filter(|member_id| *member_id != &user.id && *member_id != &target_id)
        .enumerate()
    {
        let client_event_id = format!("{}-remove-{index}", update.operation_id);
        let event_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload) VALUES(?,?,?,?,?,?)",
        )
        .bind(&event_id)
        .bind(&id)
        .bind(&user.id)
        .bind(&client_event_id)
        .bind(recipient_id)
        .bind(&update.commit_payload)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        let event = sqlx::query_as::<_, CryptoEventRow>(
            "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sender_id=? AND client_event_id=?",
        )
        .bind(&id)
        .bind(&user.id)
        .bind(&client_event_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        crypto_events.push(event);
    }
    sqlx::query(
        "INSERT INTO crypto_membership_operations(conversation_id,actor_id,operation_id,request_hash) VALUES(?,?,?,?)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&update.operation_id)
    .bind(&request_hash)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;

    if let Err(error) = remove_livekit_participant(&state, &id, &target_id).await {
        tracing::warn!(conversation_id = %id, account_id = %target_id, %error, "could not remove group member from LiveKit");
    }
    for event in &crypto_events {
        publish_crypto_event(&state, event).await?;
    }
    let _ = state.events.send(ServerEvent {
        recipients: current_member_ids,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_group_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, target_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let actor_role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked != 0 {
        return Err(ApiError::conflict(
            "Use the encrypted MLS membership removal operation for this group.",
        ));
    }
    let target_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM conversation_members WHERE conversation_id=? AND user_id=?",
    )
    .bind(&id)
    .bind(&target_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    let self_leave = target_id == user.id;
    if !self_leave && (actor_role == "member" || (actor_role == "admin" && target_role != "member"))
    {
        return Err(ApiError::forbidden());
    }
    if target_role == "owner" {
        return Err(ApiError::conflict(
            "Transfer group ownership before leaving or removing the owner.",
        ));
    }
    let old_recipients = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=?",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id=? AND user_id=?")
        .bind(&id)
        .bind(&target_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    if let Err(error) = remove_livekit_participant(&state, &id, &target_id).await {
        tracing::warn!(conversation_id = %id, account_id = %target_id, %error, "could not remove group member from LiveKit");
    }
    let _ = state.events.send(ServerEvent {
        recipients: old_recipients,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_livekit_participant(
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

fn livekit_room_admin_token(
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

#[derive(Deserialize)]
struct LockGroupCryptoRosterRequest {
    member_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupCryptoLockStatus {
    locked: bool,
    locked_by: Option<String>,
    setup_complete: bool,
}

async fn get_group_crypto_lock(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<GroupCryptoLockStatus>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    let lock = sqlx::query_as::<_, (String, i64)>(
        "SELECT locked_by,setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
    )
    .bind(conversation_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(GroupCryptoLockStatus {
        locked: lock.is_some(),
        locked_by: lock.as_ref().map(|(locked_by, _)| locked_by.clone()),
        setup_complete: lock.is_some_and(|(_, complete)| complete != 0),
    }))
}

async fn lock_group_crypto_roster(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<LockGroupCryptoRosterRequest>,
) -> Result<Json<GroupCryptoLockStatus>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let mut requested_ids = body.member_ids;
    requested_ids.sort();
    requested_ids.dedup();
    if requested_ids.len() < 2 || requested_ids.len() > 16 {
        return Err(ApiError::bad(
            "An encrypted group must have 2 to 16 members.",
        ));
    }

    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    let conversation_kind = sqlx::query_scalar::<_, String>(
        "SELECT c.kind FROM conversations c JOIN conversation_members cm ON cm.conversation_id=c.id WHERE c.id=? AND cm.user_id=?",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if conversation_kind == "direct" {
        if requested_ids.len() != 2 {
            return Err(ApiError::bad(
                "An encrypted direct conversation must have exactly two members.",
            ));
        }
        let current_ids = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY user_id",
        )
        .bind(&conversation_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if current_ids != requested_ids {
            return Err(ApiError::conflict(
                "The direct conversation changed while encryption was being prepared. Refresh and try again.",
            ));
        }
        let existing_lock = sqlx::query_as::<_, (String, i64)>(
            "SELECT locked_by,setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
        )
        .bind(&conversation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        if let Some((locked_by, setup_complete)) = existing_lock {
            if locked_by != user.id {
                return Err(ApiError::conflict(
                    "The other participant has already started encryption setup. Wait for them to finish.",
                ));
            }
            if setup_complete != 0 {
                return Err(ApiError::conflict(
                    "This direct conversation's MLS encryption setup is already complete.",
                ));
            }
        }
        sqlx::query(
            "INSERT OR IGNORE INTO conversation_crypto_locks(conversation_id,locked_by) VALUES(?,?)",
        )
        .bind(&conversation_id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
        tx.commit().await.map_err(|_| ApiError::internal())?;
        return Ok(Json(GroupCryptoLockStatus {
            locked: true,
            locked_by: Some(user.id),
            setup_complete: false,
        }));
    }
    let actor_role = sqlx::query_scalar::<_, String>(
        "SELECT cm.role FROM conversation_members cm JOIN conversations c ON c.id=cm.conversation_id WHERE cm.conversation_id=? AND cm.user_id=? AND c.kind='group'",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    if actor_role != "owner" && actor_role != "admin" {
        return Err(ApiError::forbidden());
    }

    let current_ids = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(&conversation_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if current_ids != requested_ids {
        return Err(ApiError::conflict(
            "The group roster changed while encryption was being prepared. Refresh the group and try again.",
        ));
    }

    let existing_lock = sqlx::query_as::<_, (String, i64)>(
        "SELECT locked_by,setup_complete FROM conversation_crypto_locks WHERE conversation_id=?",
    )
    .bind(&conversation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if let Some((locked_by, setup_complete)) = existing_lock {
        if locked_by != user.id {
            return Err(ApiError::conflict(
                "Another group admin started encryption setup. Wait for them to finish it.",
            ));
        }
        if setup_complete != 0 {
            return Err(ApiError::conflict(
                "This group's MLS encryption setup is already complete.",
            ));
        }
    }

    sqlx::query(
        "INSERT OR IGNORE INTO conversation_crypto_locks(conversation_id,locked_by) VALUES(?,?)",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    sqlx::query(
        "UPDATE group_invites SET revoked_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE conversation_id=? AND revoked_at IS NULL",
    )
    .bind(&conversation_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    tx.commit().await.map_err(|_| ApiError::internal())?;
    Ok(Json(GroupCryptoLockStatus {
        locked: true,
        locked_by: Some(user.id),
        setup_complete: false,
    }))
}

async fn complete_group_crypto_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<GroupCryptoLockStatus>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let changed = sqlx::query(
        "UPDATE conversation_crypto_locks SET setup_complete=1 WHERE conversation_id=? AND locked_by=?",
    )
    .bind(&conversation_id)
    .bind(&user.id)
    .execute(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if changed.rows_affected() == 0 {
        let existing = sqlx::query_scalar::<_, i64>(
            "SELECT setup_complete FROM conversation_crypto_locks WHERE conversation_id=? AND locked_by=?",
        )
        .bind(&conversation_id)
        .bind(&user.id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
        if existing.is_none() {
            return Err(ApiError::forbidden());
        }
    }
    Ok(Json(GroupCryptoLockStatus {
        locked: true,
        locked_by: Some(user.id),
        setup_complete: true,
    }))
}

async fn require_group_manager(
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

#[derive(Deserialize)]
struct CreateGroupInviteRequest {
    max_uses: i64,
}

#[derive(Serialize, FromRow)]
struct GroupInviteRow {
    id: String,
    created_by: String,
    expires_at: String,
    max_uses: i64,
    use_count: i64,
    created_at: String,
}

#[derive(Serialize)]
struct GroupInviteCreated {
    id: String,
    code: String,
    expires_at: String,
    max_uses: i64,
}

async fn list_group_invites(
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

async fn create_group_invite(
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

async fn revoke_group_invite(
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
struct JoinGroupInviteRequest {
    code: String,
}

#[derive(FromRow)]
struct GroupInviteUse {
    conversation_id: String,
    created_by: String,
}

async fn join_group_invite(
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

#[derive(Serialize, FromRow, Clone)]
struct MessageRow {
    sequence: i64,
    id: String,
    conversation_id: String,
    sender_id: String,
    sender_name: String,
    sender_avatar: Option<String>,
    client_message_id: String,
    content: String,
    created_at: String,
    edited_at: Option<String>,
    pinned_at: Option<String>,
    reply_to_id: Option<String>,
    reply_to_content: Option<String>,
    reply_to_sender: Option<String>,
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

async fn ensure_direct_not_blocked(
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
        sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.rowid<? ORDER BY m.rowid DESC LIMIT ?")
            .bind(&id).bind(before).bind(limit).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?
    } else {
        sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? ORDER BY m.rowid DESC LIMIT ?")
            .bind(&id).bind(limit).fetch_all(&state.db).await.map_err(|_| ApiError::internal())?
    };
    Ok(Json(rows.into_iter().rev().collect()))
}

#[derive(Deserialize)]
struct ReadConversationRequest {
    sequence: i64,
}

#[derive(Serialize)]
struct ReadConversationReply {
    last_read_sequence: i64,
}

async fn mark_conversation_read(
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
struct MessageReactionSummary {
    message_id: String,
    emoji: String,
    count: i64,
    reacted: i64,
}

async fn list_conversation_reactions(
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
struct ToggleMessageReaction {
    emoji: String,
}

async fn toggle_message_reaction(
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

async fn toggle_message_pin(
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

#[derive(Deserialize)]
struct SendMessage {
    client_message_id: String,
    content: String,
    reply_to_id: Option<String>,
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
    let inserted = sqlx::query("INSERT OR IGNORE INTO messages(id,conversation_id,sender_id,client_message_id,content,reply_to_id) VALUES(?,?,?,?,?,?)")
        .bind(&id).bind(&conversation_id).bind(&user.id).bind(&body.client_message_id).bind(content).bind(body.reply_to_id.as_deref()).execute(&state.db).await.map_err(|error| {
            if error.as_database_error().is_some_and(|database_error| database_error.message().contains("encrypted conversation only accepts MLS content")) {
                ApiError::conflict("This encrypted conversation only accepts MLS-encrypted text messages.")
            } else {
                ApiError::internal()
            }
        })?;
    let message = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.sender_id=? AND m.client_message_id=?")
        .bind(&conversation_id).bind(&user.id).bind(&body.client_message_id).fetch_one(&state.db).await.map_err(|_| ApiError::internal())?;
    if inserted.rows_affected() > 0 {
        publish_message(&state, &conversation_id, &message).await?;
    }
    Ok(Json(message))
}

#[derive(Serialize, FromRow, Clone)]
struct CryptoEventRow {
    sequence: i64,
    id: String,
    conversation_id: String,
    sender_id: String,
    recipient_id: Option<String>,
    payload: String,
    created_at: String,
}

#[derive(Deserialize)]
struct SendCryptoEvent {
    client_event_id: String,
    recipient_user_id: Option<String>,
    payload: String,
}

async fn list_crypto_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Vec<CryptoEventRow>>, ApiError> {
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
    let events = sqlx::query_as::<_, CryptoEventRow>(
        "SELECT sequence,id,conversation_id,sender_id,recipient_id,payload,created_at FROM crypto_events WHERE conversation_id=? AND sequence>? AND (recipient_id IS NULL OR recipient_id=?) ORDER BY sequence ASC LIMIT ?",
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

async fn send_crypto_event(
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

#[derive(Deserialize)]
struct UploadAttachmentQuery {
    name: String,
    client_message_id: String,
}

#[derive(Deserialize)]
struct EncryptedAttachmentUploadQuery {
    attachment_id: String,
    plain_size: u64,
    chunk_size: u64,
    chunk_count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EncryptedAttachmentCreated {
    attachment_id: String,
}

async fn upload_encrypted_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<EncryptedAttachmentUploadQuery>,
    body: Body,
) -> Result<Json<EncryptedAttachmentCreated>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    ensure_direct_not_blocked(&state.db, &conversation_id, &user.id).await?;
    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked == 0 {
        return Err(ApiError::conflict(
            "Encrypted attachments require MLS encryption to be enabled for this conversation.",
        ));
    }
    let attachment_id = Uuid::parse_str(&query.attachment_id)
        .map_err(|_| ApiError::bad("Invalid encrypted attachment ID."))?
        .to_string();
    if query.plain_size > MAX_ATTACHMENT_BYTES {
        return Err(ApiError::too_large());
    }
    if query.chunk_size != ENCRYPTED_ATTACHMENT_CHUNK_BYTES {
        return Err(ApiError::bad(
            "Unsupported encrypted attachment chunk size.",
        ));
    }
    let expected_chunks = if query.plain_size == 0 {
        1
    } else {
        query
            .plain_size
            .checked_add(query.chunk_size - 1)
            .ok_or_else(ApiError::too_large)?
            / query.chunk_size
    };
    if query.chunk_count != expected_chunks {
        return Err(ApiError::bad(
            "Encrypted attachment chunk count is invalid.",
        ));
    }
    let expected_ciphertext_size = query
        .plain_size
        .checked_add(
            query
                .chunk_count
                .checked_mul(16)
                .ok_or_else(ApiError::too_large)?,
        )
        .filter(|size| *size <= MAX_ATTACHMENT_BYTES + 1024 * 1024)
        .ok_or_else(ApiError::too_large)?;
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length != expected_ciphertext_size)
    {
        return Err(ApiError::bad(
            "Encrypted attachment size does not match its manifest.",
        ));
    }
    if !allow_rate(
        &state,
        &format!("upload:{}", user.id),
        10,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before sending more files.",
        ));
    }
    if sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM attachments WHERE id=?)")
        .bind(&attachment_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        != 0
    {
        return Err(ApiError::conflict(
            "This encrypted attachment ID is already in use.",
        ));
    }

    let temp_path = state.attachment_dir.join(format!("{attachment_id}.upload"));
    let stored_path = state.attachment_dir.join(format!("{attachment_id}.blob"));
    tokio::fs::create_dir_all(&state.attachment_dir)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut stream = body.into_data_stream();
    let mut size = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(ApiError::bad("The upload was interrupted."));
            }
        };
        size = match size.checked_add(chunk.len() as u64) {
            Some(size) if size <= expected_ciphertext_size => size,
            _ => {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(ApiError::too_large());
            }
        };
        if file.write_all(&chunk).await.is_err() {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ApiError::internal());
        }
    }
    if size != expected_ciphertext_size {
        drop(file);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::bad(
            "Encrypted attachment size does not match its manifest.",
        ));
    }
    if file.flush().await.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::internal());
    }
    if file.sync_all().await.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::internal());
    }
    drop(file);
    if tokio::fs::rename(&temp_path, &stored_path).await.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::internal());
    }

    let saved = sqlx::query(
        "INSERT INTO attachments(id,conversation_id,uploaded_by,filename,content_type,size_bytes,file_data,stored_on_disk,encrypted) VALUES(?,?,?,?,?,?,?,1,1)",
    )
    .bind(&attachment_id)
    .bind(&conversation_id)
    .bind(&user.id)
    .bind("encrypted-attachment.bin")
    .bind("application/octet-stream")
    .bind(size as i64)
    .bind(Vec::<u8>::new())
    .execute(&state.db)
    .await;
    if saved.is_err() {
        let _ = tokio::fs::remove_file(&stored_path).await;
        return Err(ApiError::internal());
    }
    Ok(Json(EncryptedAttachmentCreated { attachment_id }))
}

fn safe_attachment_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let clean = name
        .chars()
        .take(120)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ' ') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let clean = clean.trim().trim_matches('.');
    if clean.is_empty() {
        "file.bin".into()
    } else {
        clean.into()
    }
}

async fn upload_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(query): Query<UploadAttachmentQuery>,
    body: Body,
) -> Result<Json<MessageRow>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !is_member(&state.db, &conversation_id, &user.id).await? {
        return Err(ApiError::not_found());
    }
    ensure_direct_not_blocked(&state.db, &conversation_id, &user.id).await?;
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_ATTACHMENT_BYTES)
    {
        return Err(ApiError::too_large());
    }
    if query.client_message_id.is_empty() || query.client_message_id.len() > 64 {
        return Err(ApiError::bad("Invalid message ID."));
    }
    if !allow_rate(
        &state,
        &format!("upload:{}", user.id),
        10,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before sending more files.",
        ));
    }

    if let Some(message) = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.conversation_id=? AND m.sender_id=? AND m.client_message_id=?")
        .bind(&conversation_id)
        .bind(&user.id)
        .bind(&query.client_message_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
    {
        return Ok(Json(message));
    }

    let crypto_locked = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(&conversation_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if crypto_locked != 0 {
        return Err(ApiError::conflict(
            "File sharing is disabled in this encrypted conversation until encrypted attachments are supported.",
        ));
    }

    let filename = safe_attachment_name(&query.name);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    if content_type.len() > 120
        || content_type.contains('\r')
        || content_type.contains('\n')
        || matches!(
            content_type.as_str(),
            "text/html" | "application/xhtml+xml" | "image/svg+xml" | "application/javascript"
        )
    {
        return Err(ApiError::bad("This file type cannot be shared."));
    }

    let attachment_id = Uuid::new_v4().to_string();
    let message_id = Uuid::new_v4().to_string();
    let content = format!("{ATTACHMENT_MARKER}{attachment_id}");
    let temp_path = state.attachment_dir.join(format!("{attachment_id}.upload"));
    let stored_path = state.attachment_dir.join(format!("{attachment_id}.blob"));
    tokio::fs::create_dir_all(&state.attachment_dir)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut stream = body.into_data_stream();
    let mut size = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(ApiError::bad("The upload was interrupted."));
            }
        };
        size = match size.checked_add(chunk.len() as u64) {
            Some(size) if size <= MAX_ATTACHMENT_BYTES => size,
            _ => {
                drop(file);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(ApiError::too_large());
            }
        };
        if file.write_all(&chunk).await.is_err() {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ApiError::internal());
        }
    }
    if size == 0 {
        drop(file);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::bad("The file is empty."));
    }
    if file.flush().await.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::internal());
    }
    drop(file);
    if tokio::fs::rename(&temp_path, &stored_path).await.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::internal());
    }

    let save_result: Result<(), ApiError> = async {
        let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
        sqlx::query("INSERT INTO attachments(id,conversation_id,uploaded_by,filename,content_type,size_bytes,file_data,stored_on_disk) VALUES(?,?,?,?,?,?,?,1)")
            .bind(&attachment_id)
            .bind(&conversation_id)
            .bind(&user.id)
            .bind(filename)
            .bind(content_type)
            .bind(size as i64)
            .bind(Vec::<u8>::new())
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
        sqlx::query("INSERT INTO messages(id,conversation_id,sender_id,client_message_id,content) VALUES(?,?,?,?,?)")
            .bind(&message_id)
            .bind(&conversation_id)
            .bind(&user.id)
            .bind(&query.client_message_id)
            .bind(&content)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                if error.as_database_error().is_some_and(|database_error| database_error.message().contains("encrypted conversation only accepts MLS content")) {
                    ApiError::conflict("File sharing is disabled in this encrypted conversation until encrypted attachments are supported.")
                } else {
                    ApiError::internal()
                }
            })?;
        tx.commit().await.map_err(|_| ApiError::internal())?;
        Ok(())
    }
    .await;
    if let Err(error) = save_result {
        let _ = tokio::fs::remove_file(&stored_path).await;
        return Err(error);
    }

    let message = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.id=?")
        .bind(&message_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| ApiError::internal())?;
    publish_message(&state, &conversation_id, &message).await?;
    Ok(Json(message))
}

async fn download_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let row = attachment_row(&state, &id).await?;
    if !is_member(&state.db, &row.0, &user.id).await? {
        return Err(ApiError::not_found());
    }
    attachment_response(&state, &id, row, headers.get(header::RANGE)).await
}

#[derive(Serialize)]
struct AttachmentLink {
    url: String,
    filename: String,
    content_type: String,
}

async fn create_attachment_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<AttachmentLink>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    let row = attachment_row(&state, &id).await?;
    if !is_member(&state.db, &row.0, &user.id).await? {
        return Err(ApiError::not_found());
    }
    let ticket = make_attachment_ticket(&state, &id, &user.id);
    Ok(Json(AttachmentLink {
        url: format!("/v1/attachments/{id}/content?ticket={ticket}"),
        filename: row.1,
        content_type: row.2,
    }))
}

#[derive(Deserialize)]
struct AttachmentTicketQuery {
    ticket: String,
}

async fn download_attachment_ticket(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AttachmentTicketQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = verify_attachment_ticket(&state, &id, &query.ticket)?;
    let row = attachment_row(&state, &id).await?;
    if !is_member(&state.db, &row.0, &user_id).await? {
        return Err(ApiError::not_found());
    }
    attachment_response(&state, &id, row, headers.get(header::RANGE)).await
}

type AttachmentRow = (String, String, String, i64, Vec<u8>, i64);

async fn attachment_row(state: &AppState, id: &str) -> Result<AttachmentRow, ApiError> {
    sqlx::query_as::<_, AttachmentRow>("SELECT conversation_id,filename,content_type,size_bytes,file_data,stored_on_disk FROM attachments WHERE id=?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)
}

fn make_attachment_ticket(state: &AppState, attachment_id: &str, user_id: &str) -> String {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + ATTACHMENT_DOWNLOAD_TTL_SECONDS;
    let payload = URL_SAFE_NO_PAD.encode(format!("{attachment_id}\n{user_id}\n{expires}"));
    let mut mac = Hmac::<Sha256>::new_from_slice(&state.attachment_ticket_key)
        .expect("HMAC accepts keys of any size");
    mac.update(payload.as_bytes());
    format!(
        "{payload}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn verify_attachment_ticket(
    state: &AppState,
    attachment_id: &str,
    ticket: &str,
) -> Result<String, ApiError> {
    let (payload, signature) = ticket.split_once('.').ok_or_else(ApiError::unauthorized)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| ApiError::unauthorized())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&state.attachment_ticket_key)
        .expect("HMAC accepts keys of any size");
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| ApiError::unauthorized())?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ApiError::unauthorized())?;
    let decoded = std::str::from_utf8(&decoded).map_err(|_| ApiError::unauthorized())?;
    let mut fields = decoded.split('\n');
    let ticket_attachment_id = fields.next().ok_or_else(ApiError::unauthorized)?;
    let user_id = fields.next().ok_or_else(ApiError::unauthorized)?;
    let expires = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(ApiError::unauthorized)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if fields.next().is_some() || ticket_attachment_id != attachment_id || expires < now {
        return Err(ApiError::unauthorized());
    }
    Ok(user_id.to_owned())
}

fn parse_attachment_range(
    value: Option<&HeaderValue>,
    size: u64,
) -> Result<(u64, u64, bool), ApiError> {
    if size == 0 {
        return Err(ApiError::not_found());
    }
    let Some(value) = value else {
        return Ok((0, size - 1, false));
    };
    let value = value
        .to_str()
        .map_err(|_| ApiError::new(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range."))?;
    let range = value
        .strip_prefix("bytes=")
        .filter(|range| !range.contains(','))
        .ok_or_else(|| ApiError::new(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range."))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| ApiError::new(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range."))?;
    let (start, end) = if start.is_empty() {
        let suffix = end
            .parse::<u64>()
            .ok()
            .filter(|suffix| *suffix > 0)
            .ok_or_else(|| {
                ApiError::new(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range.")
            })?;
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start = start
            .parse::<u64>()
            .map_err(|_| ApiError::new(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range."))?;
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>()
                .map_err(|_| {
                    ApiError::new(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range.")
                })?
                .min(size - 1)
        };
        if start >= size || end < start {
            return Err(ApiError::new(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "Invalid byte range.",
            ));
        }
        (start, end)
    };
    Ok((start, end, true))
}

async fn attachment_response(
    state: &AppState,
    id: &str,
    row: AttachmentRow,
    range_header: Option<&HeaderValue>,
) -> Result<Response, ApiError> {
    let size = u64::try_from(row.3).map_err(|_| ApiError::internal())?;
    let (start, end, partial) = parse_attachment_range(range_header, size)?;
    let length = end - start + 1;
    let filename = safe_attachment_name(&row.1);
    let inline = row.2.starts_with("image/") || row.2.starts_with("video/");
    let disposition = if inline { "inline" } else { "attachment" };
    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, row.2)
        .header(
            header::CONTENT_DISPOSITION,
            format!("{disposition}; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header("x-content-type-options", "nosniff")
        .header(header::CACHE_CONTROL, "private, no-store");
    if partial {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }

    let body = if row.5 == 1 {
        let path = state.attachment_dir.join(format!("{id}.blob"));
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|_| ApiError::not_found())?;
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|_| ApiError::internal())?;
        let stream =
            futures_util::stream::try_unfold((file, length), |(mut file, remaining)| async move {
                if remaining == 0 {
                    return Ok::<_, std::io::Error>(None);
                }
                let mut buffer = vec![0u8; remaining.min(64 * 1024) as usize];
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Attachment file ended early",
                    ));
                }
                buffer.truncate(read);
                Ok(Some((Bytes::from(buffer), (file, remaining - read as u64))))
            });
        Body::from_stream(stream)
    } else {
        let start = usize::try_from(start).map_err(|_| ApiError::internal())?;
        let end = usize::try_from(end + 1).map_err(|_| ApiError::internal())?;
        let data = row.4.get(start..end).ok_or_else(ApiError::not_found)?;
        Body::from(Bytes::copy_from_slice(data))
    };
    builder.body(body).map_err(|_| ApiError::internal())
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

async fn publish_crypto_event(state: &AppState, event: &CryptoEventRow) -> Result<(), ApiError> {
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
    let msg = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id WHERE m.id=?")
        .bind(&id).fetch_optional(&state.db).await.map_err(|_| ApiError::internal())?.ok_or_else(ApiError::not_found)?;
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

async fn delete_message(
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
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
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
    let rows = sqlx::query_as::<_, MessageRow>("SELECT m.rowid AS sequence,m.id,m.conversation_id,m.sender_id,u.display_name AS sender_name,u.avatar_data AS sender_avatar,m.client_message_id,m.content,m.created_at,m.edited_at,m.pinned_at,m.reply_to_id,(SELECT r.content FROM messages r WHERE r.id=m.reply_to_id) AS reply_to_content,(SELECT u.display_name FROM messages r JOIN users u ON u.id=r.sender_id WHERE r.id=m.reply_to_id) AS reply_to_sender FROM messages m JOIN users u ON u.id=m.sender_id JOIN conversation_members c ON c.conversation_id=m.conversation_id WHERE c.user_id=? AND m.content LIKE ? ORDER BY m.rowid DESC LIMIT 50")
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WsTypingSignal {
    #[serde(rename = "type")]
    message_type: String,
    conversation_id: String,
    active: bool,
}

async fn relay_typing_signal(state: &AppState, sender: &PublicUser, text: &str) {
    let Ok(signal) = serde_json::from_str::<WsTypingSignal>(text) else {
        return;
    };
    if signal.message_type != "typing" || signal.conversation_id.len() > 64 {
        return;
    }
    let rate_key = format!("typing:{}", sender.id);
    if !allow_rate(state, &rate_key, 16, Duration::from_secs(10))
        || !is_member(&state.db, &signal.conversation_id, &sender.id)
            .await
            .unwrap_or(false)
    {
        return;
    }
    let recipients = match sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM conversation_members WHERE conversation_id=? AND user_id<>?",
    )
    .bind(&signal.conversation_id)
    .bind(&sender.id)
    .fetch_all(&state.db)
    .await
    {
        Ok(recipients) => recipients,
        Err(_) => return,
    };
    let _ = state.events.send(ServerEvent {
        recipients,
        event: serde_json::json!({
            "type": "typing",
            "conversation_id": signal.conversation_id,
            "user_id": sender.id,
            "display_name": sender.display_name,
            "active": signal.active,
        }),
    });
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
        "SELECT member.user_id FROM conversation_members member JOIN conversations c ON c.id=member.conversation_id WHERE member.conversation_id=? AND member.user_id<>? AND c.kind='direct' LIMIT 1",
    )
    .bind(&signal.conversation_id)
    .bind(&sender.id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(peer_id)) if peer_id == signal.to_user_id => peer_id,
        _ => return,
    };
    let blocked = is_blocked_pair(&state.db, &sender.id, &peer_id)
        .await
        .unwrap_or(true);
    if blocked && signal.kind != "hangup" {
        return;
    }

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
    let current_session_hash = session_hash(&token);
    let user = match sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
        .bind(&current_session_hash).fetch_optional(&state.db).await { Ok(Some(user)) => user, _ => { let _ = socket.close().await; return; } };
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
    let mut session_check = tokio::time::interval(Duration::from_secs(15));
    session_check.tick().await;
    loop {
        tokio::select! {
            _ = session_check.tick() => {
                let active = sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE token_hash=? AND datetime(expires_at)>datetime('now'))",
                )
                .bind(&current_session_hash)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
                if active == 0 {
                    let _ = socket.send(WsMessage::Text(serde_json::json!({"type":"session_revoked"}).to_string())).await;
                    let _ = socket.send(WsMessage::Close(None)).await;
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(WsMessage::Close(_))) => break,
                None | Some(Err(_)) => break,
                Some(Ok(WsMessage::Ping(bytes))) => if socket.send(WsMessage::Pong(bytes)).await.is_err() { break; },
                Some(Ok(WsMessage::Text(text))) => {
                    relay_typing_signal(&state, &user, &text).await;
                    relay_voice_signal(&state, &user, &text).await;
                },
                _ => {}
            },
            event = events.recv() => match event {
                Ok(event) if event.recipients.iter().any(|id| id == &user.id) && event.event["type"] == "session_revoked" => {
                    if event.event["except_session_hash"].as_str()
                        != Some(current_session_hash.as_str())
                    {
                        let _ = socket.send(WsMessage::Text(serde_json::json!({"type":"session_revoked"}).to_string())).await;
                        let _ = socket.send(WsMessage::Close(None)).await;
                        break;
                    }
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
    let attachment_dir = env::var_os("GHOSTCHAT_ATTACHMENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(database_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("ghostchat-attachments")
        });
    let state = build_state_with_attachment_dir(db, attachment_dir).await;
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

    #[tokio::test]
    async fn direct_conversation_row_loads_after_creation() {
        let path = std::env::temp_dir().join(format!("ghostchat-conversation-{}.sqlite", Uuid::new_v4()));
        let path_string = path.to_string_lossy().into_owned();
        let db = open_database(&path_string).await.unwrap();

        for (id, username, display_name) in [("a", "alice", "Alice"), ("b", "bob", "Bob")] {
            sqlx::query("INSERT INTO users(id,username,display_name,recovery_lookup,recovery_verifier) VALUES(?,?,?,?,?)")
                .bind(id)
                .bind(username)
                .bind(display_name)
                .bind(format!("lookup-{id}"))
                .bind("verifier")
                .execute(&db)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO conversations(id) VALUES('direct-1')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversation_members(conversation_id,user_id) VALUES('direct-1','a'),('direct-1','b')")
            .execute(&db)
            .await
            .unwrap();

        let row = conversation_row(&db, "direct-1", "a").await.unwrap();
        assert_eq!(row.id, "direct-1");
        assert_eq!(row.peer_id, "b");
        assert_eq!(row.display_name, "Bob");
        assert_eq!(row.member_names.as_deref(), Some("Bob"));

        db.close().await;
        let _ = std::fs::remove_file(path);
    }
}
