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
pub mod device_proof;
mod media_revocation;
pub mod ratelimit;

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
    online_users: Arc<Mutex<HashMap<String, usize>>>,
    voice: VoiceServerConfig,
    distributed_limiter: Result<Option<ratelimit::RateLimiter>, ratelimit::LimitError>,
    /// GHOSTCHAT_REQUIRE_REDIS=1, read once at startup.
    require_redis: bool,
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
    /// A stable identifier for refusals a client must tell apart from other
    /// errors with the same status (sent as `code` next to `error`).
    code: Option<&'static str>,
}

impl ApiError {
    fn new(status: StatusCode, message: &'static str) -> Self {
        Self {
            status,
            message,
            code: None,
        }
    }
    fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
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
        let mut body = serde_json::json!({"error": self.message});
        if let Some(code) = self.code {
            body["code"] = code.into();
        }
        (self.status, Json(body)).into_response()
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
    let migrator = sqlx::migrate!("./migrations");
    backup_before_upgrade(&pool, path, &migrator).await?;
    migrator
        .run(&pool)
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    Ok(pool)
}

/// `open_database` stopped because the pre-upgrade backup could not be
/// written; nothing was migrated. Carried in `sqlx::Error::Configuration`.
#[derive(Debug)]
pub struct PreUpgradeBackupError(String);

impl std::fmt::Display for PreUpgradeBackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PreUpgradeBackupError {}

/// Whether `open_database` failed because its pre-upgrade backup could not
/// be written (and so nothing was migrated).
pub fn is_pre_upgrade_backup_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Configuration(inner) if inner.is::<PreUpgradeBackupError>())
}

/// Migrations from this version on cannot be run by an older server binary
/// and have no reverse migration, so a backup is the only way back.
const FIRST_ONE_WAY_MIGRATION: i64 = 25;

/// Writes `<database>.pre-<first pending version>.bak` with `VACUUM INTO`
/// before an upgrade that applies a one-way migration to a database that
/// already holds data. Skips in-memory and brand-new databases and never
/// overwrites an existing backup. A failure stops startup before migrating.
async fn backup_before_upgrade(
    pool: &SqlitePool,
    path: &str,
    migrator: &sqlx::migrate::Migrator,
) -> Result<(), sqlx::Error> {
    if path.is_empty() || path == ":memory:" || path.contains("mode=memory") {
        return Ok(());
    }
    let has_history = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await?
        > 0;
    if !has_history {
        return Ok(());
    }
    let applied: std::collections::HashSet<i64> =
        sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    if applied.is_empty() {
        return Ok(());
    }
    let pending: Vec<i64> = migrator
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .filter(|version| !applied.contains(version))
        .collect();
    if !pending
        .iter()
        .any(|version| *version >= FIRST_ONE_WAY_MIGRATION)
    {
        return Ok(());
    }
    let first = pending.iter().copied().min().unwrap_or_default();
    let backup = PathBuf::from(format!("{path}.pre-{first:04}.bak"));
    if backup.exists() {
        tracing::info!(backup = %backup.display(), "pre-upgrade backup already exists");
        return Ok(());
    }
    let failed = |reason: String| {
        sqlx::Error::Configuration(Box::new(PreUpgradeBackupError(format!(
            "could not write the pre-upgrade backup {}: {reason}; the database was not migrated",
            backup.display()
        ))))
    };
    // VACUUM INTO refuses an existing target, so a stale partial file from an
    // interrupted attempt is removed first; the rename makes the final name
    // appear only once the backup is complete.
    let partial = PathBuf::from(format!("{}.tmp", backup.display()));
    if partial.exists() {
        std::fs::remove_file(&partial).map_err(|e| failed(e.to_string()))?;
    }
    sqlx::query("VACUUM INTO ?")
        .bind(partial.to_string_lossy().into_owned())
        .execute(pool)
        .await
        .map_err(|e| failed(e.to_string()))?;
    std::fs::rename(&partial, &backup).map_err(|e| failed(e.to_string()))?;
    tracing::info!(backup = %backup.display(), "wrote the pre-upgrade database backup");
    rotate_upgrade_backups(&backup);
    Ok(())
}

/// How many `pre-<version>.bak` backups of one database are kept.
const KEPT_UPGRADE_BACKUPS: usize = 3;

/// After `written` was completed, deletes this database's older
/// `<database>.pre-<version>.bak` files so that `written` and the newest
/// others (by version) make KEPT_UPGRADE_BACKUPS. `written` is never deleted;
/// other files are left alone. A failed deletion is logged, not fatal.
fn rotate_upgrade_backups(written: &std::path::Path) {
    let (Some(directory), Some(written_name)) = (
        written.parent(),
        written.file_name().and_then(|name| name.to_str()),
    ) else {
        return;
    };
    let Some(prefix) = written_name
        .rsplit_once(".pre-")
        .map(|(database, _)| format!("{database}.pre-"))
    else {
        return;
    };
    let directory = if directory.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        directory
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut others: Vec<(u64, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix(&prefix)?.strip_suffix(".bak")?;
            if name == written_name
                || version.len() < 4
                || !version.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            Some((version.parse().ok()?, entry.path()))
        })
        .collect();
    others.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in others.into_iter().skip(KEPT_UPGRADE_BACKUPS - 1) {
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(backup = %path.display(), "removed an older pre-upgrade backup")
            }
            Err(error) => {
                tracing::warn!(backup = %path.display(), %error, "could not remove an older pre-upgrade backup")
            }
        }
    }
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
        online_users: Arc::new(Mutex::new(HashMap::new())),
        voice: VoiceServerConfig {
            stun_urls,
            turn_urls,
            turn_shared_secret,
            livekit_url,
            livekit_api_key,
            livekit_api_secret,
        },
        distributed_limiter: ratelimit::RateLimiter::from_env(),
        require_redis: env::var("GHOSTCHAT_REQUIRE_REDIS").as_deref() == Ok("1"),
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
            axum::http::Method::PUT,
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
        .route("/v1/me/presence", get(list_presence).put(update_presence))
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
            "/v1/conversations/:id/planner/polls",
            get(list_group_polls).post(create_group_poll),
        )
        .route("/v1/group-polls/:id/vote", post(vote_group_poll))
        .route(
            "/v1/conversations/:id/planner/events",
            get(list_group_events).post(create_group_event),
        )
        .route("/v1/group-events/:id/rsvp", post(rsvp_group_event))
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
        .route("/v1/messages/:id/save", post(toggle_saved_message))
        .route("/v1/messages/search", get(search_messages))
        .route("/v1/me/saved-messages", get(list_saved_messages))
        .route("/v1/voice/ice", get(voice_ice_servers))
        .route("/v1/conversations/:id/voice-token", post(group_voice_token))
        .route("/v1/conversations/:id/share-token", post(ghost_share_token))
        .route("/v1/ws", get(websocket))
        .fallback(|| async { ApiError::not_found() })
        .layer(DefaultBodyLimit::max(96 * 1024))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            distributed_admission,
        ))
        .with_state(state)
}

async fn distributed_admission(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    match &state.distributed_limiter {
        Err(_) => {
            return ApiError::unavailable("Distributed admission is misconfigured.").into_response()
        }
        Ok(None) if state.require_redis => {
            return ApiError::unavailable("Distributed admission is required but unavailable.")
                .into_response()
        }
        Ok(None) => {} // Compatibility mode, explicitly not distributed security.
        Ok(Some(limiter)) => {
            let subject = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .filter(|v| v.len() <= 256)
                .map(session_hash)
                .unwrap_or_else(|| "anonymous".into());
            for (scope, identity, count, window) in [
                ("http-global", "global", 200, Duration::from_secs(1)),
                (
                    "http-session",
                    subject.as_str(),
                    600,
                    Duration::from_secs(60),
                ),
            ] {
                match limiter.check(scope, identity, count, window).await {
                    Ok(decision) if decision.allowed => {}
                    Ok(_) => {
                        return ApiError::new(
                            StatusCode::TOO_MANY_REQUESTS,
                            "Please slow down and try again.",
                        )
                        .into_response()
                    }
                    Err(_) => {
                        return ApiError::unavailable(
                            "Distributed admission could not be verified.",
                        )
                        .into_response()
                    }
                }
            }
        }
    }
    next.run(request).await
}

pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let reconciler = tokio::spawn(media_revocation::run(state.clone()));
    let upload_sweep = tokio::spawn(run_unclaimed_upload_sweep(state.clone()));
    let result = axum::serve(listener, router(state)).await;
    reconciler.abort();
    upload_sweep.abort();
    result
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
    media_revocation::admit(&state, &conversation_id).await?;
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
        "exp": now.saturating_add(media_revocation::JOIN_TTL),
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
        "expires_at": now.saturating_add(media_revocation::JOIN_TTL)
    })))
}

// GhostShare uses a separate LiveKit room for one-to-one screen sharing. Audio
// remains on the existing direct-call path; separating the screen publisher lets
// the SFU adapt layers without renegotiating or destabilising the call itself.
async fn ghost_share_token(
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
    if participant.1 != "direct" {
        return Err(ApiError::not_found());
    }
    // A block forbids GhostShare like it forbids call signaling.
    ensure_direct_not_blocked(&state.db, &conversation_id, &user.id).await?;
    media_revocation::admit(&state, &conversation_id).await?;
    let api_key = state.voice.livekit_api_key.as_deref().ok_or_else(|| {
        ApiError::unavailable("GhostShare needs a configured LiveKit media server.")
    })?;
    let api_secret = state.voice.livekit_api_secret.as_deref().ok_or_else(|| {
        ApiError::unavailable("GhostShare needs a configured LiveKit media server.")
    })?;
    let server_url = state.voice.livekit_url.as_deref().ok_or_else(|| {
        ApiError::unavailable("GhostShare needs a configured LiveKit media server.")
    })?;
    if !allow_rate(
        &state,
        &format!("ghostshare-token:{}", user.id),
        12,
        Duration::from_secs(60),
    ) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Please wait before requesting another GhostShare token.",
        ));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let room = format!("ghostshare-{conversation_id}");
    let identity = format!("{}-{}", user.id, Uuid::new_v4().simple());
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let metadata = serde_json::json!({"account_id": user.id, "purpose": "ghostshare"}).to_string();
    let claims = serde_json::json!({
        "iss": api_key, "sub": identity, "name": participant.0, "metadata": metadata,
        "iat": now, "nbf": now.saturating_sub(5), "exp": now.saturating_add(media_revocation::JOIN_TTL),
        "video": {"roomJoin": true, "room": room, "canPublish": true,
          "canPublishSources": ["screen_share", "screen_share_audio"], "canSubscribe": true, "canPublishData": false}
    });
    let payload =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| ApiError::internal())?);
    let signing_input = format!("{header}.{payload}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(api_secret.as_bytes()).map_err(|_| ApiError::internal())?;
    mac.update(signing_input.as_bytes());
    let token = format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    Ok(Json(
        serde_json::json!({"url": server_url, "token": token, "room": room, "expires_at": now.saturating_add(media_revocation::JOIN_TTL)}),
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

#[derive(Serialize, FromRow)]
struct PresenceRow {
    user_id: String,
    mode: String,
}

#[derive(Deserialize)]
struct UpdatePresence {
    mode: String,
}

async fn list_presence(
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

fn register_online_connection(state: &AppState, user_id: &str) -> bool {
    let Ok(mut users) = state.online_users.lock() else {
        return false;
    };
    let count = users.entry(user_id.to_owned()).or_insert(0);
    *count += 1;
    *count == 1
}

fn unregister_online_connection(state: &AppState, user_id: &str) -> bool {
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
async fn broadcast_current_presence(state: &AppState, user_id: &str) {
    broadcast_presence(state, user_id, false).await;
}

async fn broadcast_presence(state: &AppState, user_id: &str, explicit_mode_change: bool) {
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

async fn update_presence(
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
struct UpdateProfile {
    display_name: String,
    username: Option<String>,
    avatar_data: Option<String>,
}

fn validate_avatar(value: Option<String>) -> Result<Option<String>, ApiError> {
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
    /// Join order (lower joined earlier), for deterministic client fallbacks.
    joined_seq: Option<i64>,
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
    /// Accounts the client says the commit adds or removes. Optional for
    /// older clients; when given for a removal it must be the route target.
    /// Not part of the idempotency hash (the route target already is).
    #[serde(default, skip_serializing)]
    member_ids: Vec<String>,
    /// The conversation's commit sequence this commit was built on. Not part
    /// of the idempotency hash, so a retry of an accepted operation is
    /// recognised whatever it carries here.
    #[serde(default, skip_serializing)]
    expected_commit_seq: Option<i64>,
    commit_payload: String,
    #[serde(default)]
    welcome_payload: String,
}

/// The server cannot read an MLS commit, so it cannot see which group state
/// (epoch) a commit was built on. `conversation_crypto_locks.commit_seq`
/// counts the commits accepted for a conversation; a membership commit must
/// name the value it was built on. Two different commits built on the same
/// state can therefore never both be accepted, which would fork the group.
fn stale_crypto_commit(missing: bool) -> ApiError {
    ApiError::conflict(if missing {
        "This encrypted membership change does not say which group state it was built on. Update GhostChat and try again."
    } else {
        "The encrypted group changed while this update was being prepared. Sync the group and try again."
    })
    .with_code("stale_commit_seq")
}

async fn require_crypto_commit_seq(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation_id: &str,
    expected: Option<i64>,
) -> Result<i64, ApiError> {
    let expected = expected.ok_or_else(|| stale_crypto_commit(true))?;
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT commit_seq FROM conversation_crypto_locks WHERE conversation_id=?",
    )
    .bind(conversation_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if current != Some(expected) {
        return Err(stale_crypto_commit(false));
    }
    Ok(expected)
}

/// Counts an accepted commit, in the transaction that relays it. The update
/// is conditional, so of two requests racing from the same value only one
/// can be counted.
async fn advance_crypto_commit_seq(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation_id: &str,
    expected: i64,
) -> Result<(), ApiError> {
    let advanced = sqlx::query(
        "UPDATE conversation_crypto_locks SET commit_seq=commit_seq+1 WHERE conversation_id=? AND commit_seq=?",
    )
    .bind(conversation_id)
    .bind(expected)
    .execute(&mut **tx)
    .await
    .map_err(|_| ApiError::internal())?;
    if advanced.rows_affected() != 1 {
        return Err(stale_crypto_commit(false));
    }
    Ok(())
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
    let built_on_commit_seq = match crypto_update.as_ref() {
        Some(update) => {
            Some(require_crypto_commit_seq(&mut tx, &id, update.expected_commit_seq).await?)
        }
        None => None,
    };

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
        if let Some(built_on) = built_on_commit_seq {
            advance_crypto_commit_seq(&mut tx, &id, built_on).await?;
        }
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
    let built_on_commit_seq =
        require_crypto_commit_seq(&mut tx, &id, update.expected_commit_seq).await?;
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
    media_revocation::enqueue(&mut tx, &id, &target_id).await?;
    advance_crypto_commit_seq(&mut tx, &id, built_on_commit_seq).await?;
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
    // The server cannot read the commit (MLS ciphertext). The client states
    // its target, and a statement that differs from the route is refused.
    if !update.member_ids.is_empty()
        && (update.member_ids.len() != 1
            || Uuid::parse_str(&update.member_ids[0])
                .map(|id| id.to_string())
                .ok()
                != Some(target_id.clone()))
    {
        return Err(ApiError::bad(
            "The MLS removal declares a different member than this operation.",
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
    .map_err(|_| ApiError::internal())?;
    // A deleted account is no longer a member, but its MLS leaf stays in the
    // group until one removal commit for it is accepted here.
    let deleted_target = target_role.is_none()
        && sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM crypto_group_deleted_members WHERE conversation_id=? AND user_id=? AND removal_committed_at IS NULL)",
        )
        .bind(&id)
        .bind(&target_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?
            != 0;
    let target_role = match target_role {
        Some(role) => role,
        None if deleted_target => "member".to_owned(),
        None => {
            // Another owner/admin's removal of this deleted leaf was already
            // accepted: tell the client its own staged removal is moot.
            let already_removed = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM crypto_group_deleted_members WHERE conversation_id=? AND user_id=? AND removal_committed_at IS NOT NULL)",
            )
            .bind(&id)
            .bind(&target_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            if already_removed != 0 {
                return Err(ApiError::new(
                    StatusCode::GONE,
                    "This deleted account was already removed from the encrypted group.",
                ));
            }
            return Err(ApiError::not_found());
        }
    };
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
    let built_on_commit_seq =
        require_crypto_commit_seq(&mut tx, &id, update.expected_commit_seq).await?;
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

    if deleted_target {
        sqlx::query(
            "UPDATE crypto_group_deleted_members SET removal_committed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE conversation_id=? AND user_id=?",
        )
        .bind(&id)
        .bind(&target_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal())?;
    } else {
        sqlx::query("DELETE FROM conversation_members WHERE conversation_id=? AND user_id=?")
            .bind(&id)
            .bind(&target_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
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
        "INSERT INTO crypto_membership_operations(conversation_id,actor_id,operation_id,request_hash,target_id) VALUES(?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&update.operation_id)
    .bind(&request_hash)
    .bind(&target_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal())?;
    // A deleted account's media access was revoked when it was deleted.
    if !deleted_target {
        media_revocation::enqueue(&mut tx, &id, &target_id).await?;
    }
    advance_crypto_commit_seq(&mut tx, &id, built_on_commit_seq).await?;
    tx.commit().await.map_err(|_| ApiError::internal())?;

    if !deleted_target {
        if let Err(error) = remove_livekit_participant(&state, &id, &target_id).await {
            tracing::warn!(conversation_id = %id, account_id = %target_id, %error, "could not remove group member from LiveKit");
        }
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
    media_revocation::enqueue(&mut tx, &id, &target_id).await?;
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
    /// Accounts deleted while members of this MLS-locked group, whose MLS
    /// leaves remaining clients may tolerate until they are removed.
    deleted_member_ids: Vec<String>,
    /// The same accounts with when they were deleted and when the removal
    /// commit for their leaf was accepted (null while it is still pending).
    deleted_members: Vec<DeletedCryptoMember>,
    /// The unfinished setup was started by an account that no longer exists;
    /// the group owner (or the oldest admin when there is no owner) may take
    /// it over through the lock route.
    locked_by_deleted_account: bool,
    /// Counts takeovers of an unfinished setup. A takeover is a reset, so a
    /// device holding the group of an older generation must drop it.
    setup_generation: i64,
    /// Counts the accepted membership commits (and setup takeovers). A
    /// membership commit names the value it was built on.
    commit_seq: i64,
    /// The server's clock, in the format of `deletedAt`. A client measures
    /// how long a deleted account's leaf has been waiting as the difference
    /// of the two, so its own clock does not matter.
    server_now: String,
}

async fn server_now(db: &SqlitePool) -> Result<String, ApiError> {
    sqlx::query_scalar::<_, String>("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .fetch_one(db)
        .await
        .map_err(|_| ApiError::internal())
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct DeletedCryptoMember {
    user_id: String,
    deleted_at: String,
    removal_committed_at: Option<String>,
}

async fn deleted_crypto_members(
    db: &SqlitePool,
    conversation_id: &str,
) -> Result<Vec<DeletedCryptoMember>, ApiError> {
    sqlx::query_as::<_, DeletedCryptoMember>(
        "SELECT user_id,deleted_at,removal_committed_at FROM crypto_group_deleted_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
    .map_err(|_| ApiError::internal())
}

/// The lock status of a group conversation, with its deleted-member record.
async fn group_crypto_lock_status(
    db: &SqlitePool,
    conversation_id: &str,
) -> Result<GroupCryptoLockStatus, ApiError> {
    let lock = sqlx::query_as::<_, (String, i64, bool, i64, i64)>(
        "SELECT l.locked_by,l.setup_complete,NOT EXISTS(SELECT 1 FROM users u WHERE u.id=l.locked_by),l.setup_generation,l.commit_seq FROM conversation_crypto_locks l WHERE l.conversation_id=?",
    )
    .bind(conversation_id)
    .fetch_optional(db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(GroupCryptoLockStatus {
        locked: lock.is_some(),
        locked_by: lock.as_ref().map(|(locked_by, ..)| locked_by.clone()),
        setup_complete: lock
            .as_ref()
            .is_some_and(|(_, complete, ..)| *complete != 0),
        deleted_member_ids: deleted_crypto_member_ids(db, conversation_id).await?,
        deleted_members: deleted_crypto_members(db, conversation_id).await?,
        locked_by_deleted_account: lock
            .as_ref()
            .is_some_and(|(_, complete, deleted, ..)| *complete == 0 && *deleted),
        setup_generation: lock.as_ref().map_or(0, |(.., generation, _)| *generation),
        commit_seq: lock.as_ref().map_or(0, |(.., commit_seq)| *commit_seq),
        server_now: server_now(db).await?,
    })
}

async fn deleted_crypto_member_ids(
    db: &SqlitePool,
    conversation_id: &str,
) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM crypto_group_deleted_members WHERE conversation_id=? ORDER BY user_id",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
    .map_err(|_| ApiError::internal())
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
    Ok(Json(
        group_crypto_lock_status(&state.db, &conversation_id).await?,
    ))
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
            // A direct conversation is deleted with either account.
            deleted_member_ids: Vec::new(),
            deleted_members: Vec::new(),
            locked_by_deleted_account: false,
            // Only a group setup can be taken over, and a direct
            // conversation has no membership commits.
            setup_generation: 0,
            commit_seq: 0,
            server_now: server_now(&state.db).await?,
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
        if locked_by != user.id && setup_complete == 0 {
            // An unfinished setup whose starter deleted their account can
            // never be completed by them. The owner takes it over, or the
            // oldest admin when the group has no owner.
            let locker_deleted =
                sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM users WHERE id=?)")
                    .bind(&locked_by)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|_| ApiError::internal())?
                    == 0;
            let successor = sqlx::query_scalar::<_, String>(
                "SELECT user_id FROM conversation_members WHERE conversation_id=? AND role IN ('owner','admin') ORDER BY CASE role WHEN 'owner' THEN 0 ELSE 1 END, joined_seq, user_id LIMIT 1",
            )
            .bind(&conversation_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
            if locker_deleted && successor.as_deref() == Some(user.id.as_str()) {
                // The takeover is a reset, never an adoption: nobody can
                // know which members received the old Welcome. A new setup
                // generation starts, and the old setup's relay events and
                // deleted-leaf records go with it, so no device can still
                // join the old group or tolerate its leaves. The commit
                // sequence moves on too, so nothing built on the old group
                // is accepted for the new one.
                sqlx::query(
                    "UPDATE conversation_crypto_locks SET locked_by=?,setup_generation=setup_generation+1,commit_seq=commit_seq+1 WHERE conversation_id=? AND locked_by=? AND setup_complete=0",
                )
                .bind(&user.id)
                .bind(&conversation_id)
                .bind(&locked_by)
                .execute(&mut *tx)
                .await
                .map_err(|_| ApiError::internal())?;
                for table in ["crypto_events", "crypto_group_deleted_members"] {
                    sqlx::query(&format!("DELETE FROM {table} WHERE conversation_id=?"))
                        .bind(&conversation_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|_| ApiError::internal())?;
                }
            } else if locker_deleted {
                return Err(ApiError::forbidden());
            } else {
                return Err(ApiError::conflict(
                    "Another group admin started encryption setup. Wait for them to finish it.",
                ));
            }
        } else if locked_by != user.id {
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
    Ok(Json(
        group_crypto_lock_status(&state.db, &conversation_id).await?,
    ))
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
    Ok(Json(
        group_crypto_lock_status(&state.db, &conversation_id).await?,
    ))
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

#[derive(Serialize)]
struct SavedMessageState {
    saved: bool,
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

async fn toggle_saved_message(
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

async fn list_saved_messages(
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

async fn ensure_group_planner_member(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), ApiError> {
    let allowed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversations c JOIN conversation_members cm ON cm.conversation_id=c.id WHERE c.id=? AND c.kind='group' AND cm.user_id=?",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_one(db)
    .await
    .map_err(|_| ApiError::internal())?;
    if allowed == 0 {
        Err(ApiError::not_found())
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
struct CreateGroupPollRequest {
    question: String,
    options: Vec<String>,
    closes_at: Option<String>,
}

#[derive(Serialize, FromRow)]
struct GroupPollOption {
    id: String,
    label: String,
    position: i64,
    votes: i64,
}

#[derive(Serialize, FromRow)]
struct GroupPollRow {
    id: String,
    conversation_id: String,
    /// `None` once the creator deleted their account.
    creator_id: Option<String>,
    question: String,
    closes_at: Option<String>,
    closed_at: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct GroupPollDetail {
    #[serde(flatten)]
    poll: GroupPollRow,
    options: Vec<GroupPollOption>,
    my_option_id: Option<String>,
}

async fn group_poll_details(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<Vec<GroupPollDetail>, ApiError> {
    let polls = sqlx::query_as::<_, GroupPollRow>(
        "SELECT id,conversation_id,creator_id,question,closes_at,closed_at,created_at FROM group_polls WHERE conversation_id=? ORDER BY created_at DESC LIMIT 100",
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await
    .map_err(|_| ApiError::internal())?;
    let mut details = Vec::with_capacity(polls.len());
    for poll in polls {
        let options = sqlx::query_as::<_, GroupPollOption>(
            "SELECT o.id,o.label,o.position,(SELECT count(*) FROM group_poll_votes v WHERE v.option_id=o.id) AS votes FROM group_poll_options o WHERE o.poll_id=? ORDER BY o.position",
        )
        .bind(&poll.id)
        .fetch_all(db)
        .await
        .map_err(|_| ApiError::internal())?;
        let my_option_id = sqlx::query_scalar::<_, String>(
            "SELECT option_id FROM group_poll_votes WHERE poll_id=? AND user_id=?",
        )
        .bind(&poll.id)
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(|_| ApiError::internal())?;
        details.push(GroupPollDetail {
            poll,
            options,
            my_option_id,
        });
    }
    Ok(details)
}

async fn list_group_polls(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<Vec<GroupPollDetail>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    Ok(Json(
        group_poll_details(&state.db, &conversation_id, &user.id).await?,
    ))
}

async fn create_group_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<CreateGroupPollRequest>,
) -> Result<Json<Vec<GroupPollDetail>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    let question = body.question.trim();
    let options: Vec<_> = body
        .options
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    if question.is_empty()
        || question.len() > 240
        || !(2..=8).contains(&options.len())
        || options.iter().any(|item| item.len() > 100)
    {
        return Err(ApiError::bad(
            "A poll needs a question and 2 to 8 short options.",
        ));
    }
    if body
        .closes_at
        .as_ref()
        .is_some_and(|value| value.len() > 64)
    {
        return Err(ApiError::bad("The poll closing time is not valid."));
    }
    if let Some(closes_at) = body.closes_at.as_deref() {
        let parsed = sqlx::query_scalar::<_, Option<f64>>("SELECT julianday(?)")
            .bind(closes_at)
            .fetch_one(&state.db)
            .await
            .map_err(|_| ApiError::internal())?;
        if parsed.is_none() {
            return Err(ApiError::bad("The poll closing time is not valid."));
        }
    }
    let poll_id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await.map_err(|_| ApiError::internal())?;
    sqlx::query("INSERT INTO group_polls(id,conversation_id,creator_id,question,closes_at) VALUES(?,?,?,?,?)")
        .bind(&poll_id).bind(&conversation_id).bind(&user.id).bind(question).bind(body.closes_at.as_deref())
        .execute(&mut *tx).await.map_err(|_| ApiError::internal())?;
    for (position, option) in options.iter().enumerate() {
        sqlx::query("INSERT INTO group_poll_options(id,poll_id,label,position) VALUES(?,?,?,?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&poll_id)
            .bind(option)
            .bind(position as i64)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    tx.commit().await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
    Ok(Json(
        group_poll_details(&state.db, &conversation_id, &user.id).await?,
    ))
}

#[derive(Deserialize)]
struct VoteGroupPollRequest {
    option_id: String,
}

async fn vote_group_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(poll_id): Path<String>,
    Json(body): Json<VoteGroupPollRequest>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    // A closing time stored before it was validated may not be a time:
    // julianday() is then NULL and so is the comparison. Such a poll has no
    // deadline that can be enforced and stays open; COALESCE says so in SQL
    // instead of leaving it to how the driver reads a NULL boolean.
    let (conversation_id, expired) = sqlx::query_as::<_, (String, bool)>(
        "SELECT conversation_id,COALESCE(julianday(closes_at)<=julianday('now'),0) FROM group_polls WHERE id=? AND closed_at IS NULL",
    )
    .bind(&poll_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    if expired {
        return Err(ApiError::conflict("This poll is closed."));
    }
    let valid = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM group_poll_options WHERE id=? AND poll_id=?",
    )
    .bind(&body.option_id)
    .bind(&poll_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if valid == 0 {
        return Err(ApiError::bad("Choose an option from this poll."));
    }
    sqlx::query("INSERT INTO group_poll_votes(poll_id,user_id,option_id) VALUES(?,?,?) ON CONFLICT(poll_id,user_id) DO UPDATE SET option_id=excluded.option_id,created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .bind(&poll_id).bind(&user.id).bind(&body.option_id).execute(&state.db).await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CreateGroupEventRequest {
    title: String,
    details: String,
    scheduled_at: String,
}

#[derive(Deserialize)]
struct GroupEventRsvpRequest {
    status: String,
}

#[derive(Serialize, FromRow)]
struct GroupEventRow {
    id: String,
    conversation_id: String,
    /// `None` once the creator deleted their account.
    creator_id: Option<String>,
    title: String,
    details: String,
    scheduled_at: String,
    cancelled_at: Option<String>,
    created_at: String,
    going: i64,
    maybe: i64,
    declined: i64,
    my_status: Option<String>,
}

async fn group_event_details(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<Vec<GroupEventRow>, ApiError> {
    sqlx::query_as::<_, GroupEventRow>(
        "SELECT e.id,e.conversation_id,e.creator_id,e.title,e.details,e.scheduled_at,e.cancelled_at,e.created_at,(SELECT count(*) FROM group_event_rsvps r WHERE r.event_id=e.id AND r.status='going') AS going,(SELECT count(*) FROM group_event_rsvps r WHERE r.event_id=e.id AND r.status='maybe') AS maybe,(SELECT count(*) FROM group_event_rsvps r WHERE r.event_id=e.id AND r.status='declined') AS declined,(SELECT status FROM group_event_rsvps r WHERE r.event_id=e.id AND r.user_id=?) AS my_status FROM group_events e WHERE e.conversation_id=? ORDER BY e.scheduled_at ASC LIMIT 100",
    ).bind(user_id).bind(conversation_id).fetch_all(db).await.map_err(|_| ApiError::internal())
}

async fn list_group_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<Vec<GroupEventRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    Ok(Json(
        group_event_details(&state.db, &conversation_id, &user.id).await?,
    ))
}

async fn create_group_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<CreateGroupEventRequest>,
) -> Result<Json<Vec<GroupEventRow>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    let title = body.title.trim();
    let details = body.details.trim();
    let scheduled_at = body.scheduled_at.trim();
    if !(2..=140).contains(&title.len())
        || details.len() > 1_500
        || scheduled_at.is_empty()
        || scheduled_at.len() > 64
    {
        return Err(ApiError::bad(
            "Add a short title and a valid time for this event.",
        ));
    }
    sqlx::query("INSERT INTO group_events(id,conversation_id,creator_id,title,details,scheduled_at) VALUES(?,?,?,?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(&conversation_id).bind(&user.id).bind(title).bind(details).bind(scheduled_at)
        .execute(&state.db).await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
    Ok(Json(
        group_event_details(&state.db, &conversation_id, &user.id).await?,
    ))
}

async fn rsvp_group_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(event_id): Path<String>,
    Json(body): Json<GroupEventRsvpRequest>,
) -> Result<StatusCode, ApiError> {
    let user = authenticate(&headers, &state).await?;
    if !matches!(body.status.as_str(), "going" | "maybe" | "declined") {
        return Err(ApiError::bad("Choose going, maybe, or declined."));
    }
    let conversation_id = sqlx::query_scalar::<_, String>(
        "SELECT conversation_id FROM group_events WHERE id=? AND cancelled_at IS NULL",
    )
    .bind(&event_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::not_found)?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    sqlx::query("INSERT INTO group_event_rsvps(event_id,user_id,status) VALUES(?,?,?) ON CONFLICT(event_id,user_id) DO UPDATE SET status=excluded.status,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .bind(&event_id).bind(&user.id).bind(&body.status).execute(&state.db).await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SendMessage {
    client_message_id: String,
    content: String,
    reply_to_id: Option<String>,
    /// Encrypted attachments (uploaded by the sender to this conversation)
    /// whose keys this e2e1 message carries; they are deleted with it.
    #[serde(default)]
    attachment_ids: Vec<String>,
    /// The conversation's commit sequence the sender's device had applied
    /// when it encrypted this message. Optional (older clients, direct
    /// conversations); when given, a new message is refused once the
    /// conversation has moved on, because devices that applied the newer
    /// commit can no longer decrypt it.
    #[serde(default)]
    expected_commit_seq: Option<i64>,
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
        .ok_or_else(|| ApiError::conflict("This message ID is already used in this conversation."));
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

/// A relay event as listed for a member, with the setup generation of its
/// conversation at the time of the listing.
#[derive(Serialize, FromRow)]
struct CryptoEventListRow {
    #[serde(flatten)]
    #[sqlx(flatten)]
    event: CryptoEventRow,
    setup_generation: i64,
    commit_seq: i64,
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

/// An encrypted upload no message has claimed within this time is abandoned.
const UNCLAIMED_UPLOAD_TTL_SECS: i64 = 24 * 60 * 60;
const UNCLAIMED_UPLOAD_SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// Deletes encrypted uploads that no message claimed (row and blob), once
/// they are older than a day as seen from `now` (Unix seconds). Only uploads
/// made after migration 0026 qualify: before it an upload could not be
/// linked, and an encrypted message the server cannot read may still point
/// to it. `installed_on` has whole seconds, so a one-second margin keeps an
/// upload from the second the migration ran out of the sweep. Without a
/// record for 0026 the comparison is NULL and nothing is deleted.
async fn sweep_unclaimed_encrypted_uploads(state: &AppState, now: i64) -> Result<u64, sqlx::Error> {
    let candidates = sqlx::query_scalar::<_, String>(
        "SELECT id FROM attachments WHERE encrypted=1 AND message_id IS NULL \
         AND julianday(created_at) < julianday(?,'unixepoch') \
         AND julianday(created_at) > (SELECT julianday(installed_on,'+1 second') FROM _sqlx_migrations WHERE version=26) \
         LIMIT 500",
    )
    .bind(now - UNCLAIMED_UPLOAD_TTL_SECS)
    .fetch_all(&state.db)
    .await?;
    let mut removed = 0;
    for id in candidates {
        // A message may have claimed it since the select.
        let deleted = sqlx::query(
            "DELETE FROM attachments WHERE id=? AND encrypted=1 AND message_id IS NULL",
        )
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected();
        if deleted == 1 {
            let path = state.attachment_dir.join(format!("{id}.blob"));
            let _ = tokio::fs::remove_file(path).await;
            removed += 1;
        }
    }
    Ok(removed)
}

async fn run_unclaimed_upload_sweep(state: AppState) {
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if sweep_unclaimed_encrypted_uploads(&state, now)
            .await
            .is_err()
        {
            tracing::warn!("Sweeping unclaimed encrypted uploads failed; retrying later");
        }
        tokio::time::sleep(UNCLAIMED_UPLOAD_SWEEP_EVERY).await;
    }
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
    // A blocked direct pair exchanges no typing indicators.
    if ensure_direct_not_blocked(&state.db, &signal.conversation_id, &sender.id)
        .await
        .is_err()
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

// Watch Party carries playback state, never media bytes. The server validates
// the shape and relays it through the same authenticated peer/member checks as
// call setup; it must never fetch the client-provided URL.
fn valid_watch_party_payload(payload: &serde_json::Value) -> bool {
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    let Some(object) = payload.as_object() else {
        return false;
    };
    if object.len() != 6 {
        return false;
    }
    let valid_source = payload["source"].as_str().is_some_and(|source| {
        source.is_empty()
            || (source.len() <= 2048
                && reqwest::Url::parse(source).is_ok_and(|url| {
                    url.scheme() == "https"
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                }))
    });
    valid_source
        && payload["position"]
            .as_f64()
            .is_some_and(|value| (0.0..=86_400.0).contains(&value))
        && payload["playing"].as_bool().is_some()
        && payload["revision"]
            .as_u64()
            .is_some_and(|value| value <= 1_000_000_000)
        && payload["sentAt"]
            .as_u64()
            .is_some_and(|value| value <= MAX_SAFE_INTEGER)
        && payload["action"].as_str().is_some_and(|value| {
            matches!(
                value,
                "source" | "play" | "pause" | "seek" | "request" | "state"
            )
        })
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
        "hangup" | "screen_stopped" | "ghostshare_started" => signal.payload.is_null(),
        "watch_party" => valid_watch_party_payload(&signal.payload),
        // Notes are call-room state only: relay them, but never persist them
        // in the chat history or account data.
        "call_note" => signal.payload["text"]
            .as_str()
            .is_some_and(|value| value.len() <= 1_200),
        _ => false,
    };
    if !valid_payload {
        return;
    }

    let peer_id = match sqlx::query_scalar::<_, String>(
        "SELECT member.user_id FROM conversation_members member JOIN conversations c ON c.id=member.conversation_id WHERE member.conversation_id=? AND member.user_id<>? AND c.kind='direct' AND EXISTS (SELECT 1 FROM conversation_members sender WHERE sender.conversation_id=c.id AND sender.user_id=?) LIMIT 1",
    )
    .bind(&signal.conversation_id)
    .bind(&sender.id)
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

fn session_revocation_applies(event: &serde_json::Value, current_session_hash: &str) -> bool {
    if let Some(only_session_hash) = event["session_hash"].as_str() {
        return only_session_hash == current_session_hash;
    }
    event["except_session_hash"].as_str() != Some(current_session_hash)
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
    // Subscribe before announcing "ready": an event published right after
    // the client sees "ready" (for example a logout) must not be missed.
    let mut events = state.events.subscribe();
    if socket
        .send(WsMessage::Text(
            serde_json::json!({"type":"ready"}).to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }
    if register_online_connection(&state, &user.id) {
        broadcast_current_presence(&state, &user.id).await;
    }
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
                    if session_revocation_applies(&event.event, &current_session_hash) {
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
    if unregister_online_connection(&state, &user.id) {
        broadcast_current_presence(&state, &user.id).await;
    }
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
    fn watch_party_accepts_bounded_state_and_all_protocol_actions() {
        for action in ["source", "play", "pause", "seek", "request", "state"] {
            let payload = serde_json::json!({
                "source": "https://media.example/movie.mp4", "position": 42.5,
                "playing": true, "revision": 3, "sentAt": 1_793_700_000_000_u64,
                "action": action
            });
            assert!(valid_watch_party_payload(&payload));
            let mut cleared = payload;
            cleared["source"] = serde_json::json!("");
            assert!(valid_watch_party_payload(&cleared));
        }
    }

    #[test]
    fn watch_party_rejects_unsafe_urls_and_unbounded_state() {
        let good = serde_json::json!({
            "source": "https://media.example/movie.mp4", "position": 0,
            "playing": false, "revision": 0, "sentAt": 0, "action": "state"
        });
        for source in [
            "http://media.example/a",
            "file:///C:/secret",
            "javascript:alert(1)",
            "https://user:secret@media.example/a",
            "https://",
        ] {
            let mut invalid = good.clone();
            invalid["source"] = serde_json::json!(source);
            assert!(!valid_watch_party_payload(&invalid), "accepted {source}");
        }
        for (key, value) in [
            ("position", serde_json::json!(-1)),
            ("position", serde_json::json!(86_401)),
            ("revision", serde_json::json!(0.5)),
            ("revision", serde_json::json!(-1)),
            ("sentAt", serde_json::json!(9_007_199_254_740_992_u64)),
            ("playing", serde_json::json!("true")),
            ("action", serde_json::json!("fetch")),
            (
                "source",
                serde_json::json!(format!("https://media.example/{}", "x".repeat(2048))),
            ),
        ] {
            let mut invalid = good.clone();
            invalid[key] = value;
            assert!(
                !valid_watch_party_payload(&invalid),
                "accepted invalid {key}"
            );
        }
        let mut missing = good;
        missing.as_object_mut().unwrap().remove("revision");
        assert!(!valid_watch_party_payload(&missing));
    }

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
        let path =
            std::env::temp_dir().join(format!("ghostchat-conversation-{}.sqlite", Uuid::new_v4()));
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

    // Route-level harness: a real router on a loopback port backed by a fresh
    // SQLite file with all migrations, driven over HTTP with reqwest.
    struct TestApp {
        base: String,
        state: AppState,
        client: reqwest::Client,
        dir: PathBuf,
    }

    struct TestUser {
        id: String,
        token: String,
    }

    impl Drop for TestApp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl TestApp {
        async fn new() -> Self {
            Self::configured(false).await
        }

        async fn configured(require_redis: bool) -> Self {
            let dir = std::env::temp_dir().join(format!("ghostchat-route-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let db = open_database(&dir.join("db.sqlite").to_string_lossy())
                .await
                .unwrap();
            let mut state = build_state_with_attachment_dir(db, dir.join("attachments")).await;
            // Do not inherit the developer's shell: no Redis admission, no
            // TURN and no LiveKit, whatever GHOSTCHAT_* variables are set.
            state.require_redis = require_redis;
            state.distributed_limiter = Ok(None);
            state.voice = VoiceServerConfig {
                stun_urls: vec!["stun:stun.l.google.com:19302".into()],
                turn_urls: Vec::new(),
                turn_shared_secret: None,
                livekit_url: None,
                livekit_api_key: None,
                livekit_api_secret: None,
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let app = router(state.clone());
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            Self {
                base,
                state,
                client,
                dir,
            }
        }

        fn request(
            &self,
            method: reqwest::Method,
            path: &str,
            user: &TestUser,
        ) -> reqwest::RequestBuilder {
            self.client
                .request(method, format!("{}{path}", self.base))
                .bearer_auth(&user.token)
        }

        async fn user(&self, username: &str) -> TestUser {
            let reply: serde_json::Value = self
                .client
                .post(format!("{}/v1/accounts", self.base))
                .json(&serde_json::json!({"username": username, "display_name": username}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            TestUser {
                id: reply["user"]["id"].as_str().unwrap().to_owned(),
                token: reply["session_token"].as_str().unwrap().to_owned(),
            }
        }

        async fn befriend(&self, first: &TestUser, second: &TestUser) {
            for (from, to) in [(first, second), (second, first)] {
                let status = self
                    .request(reqwest::Method::POST, "/v1/friends", from)
                    .json(&serde_json::json!({"user_id": to.id}))
                    .send()
                    .await
                    .unwrap()
                    .status();
                // 409 means the pair is already friends (helper reused).
                assert!(
                    matches!(
                        status,
                        reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::CONFLICT
                    ),
                    "friend request returned {status}"
                );
            }
        }

        async fn group(&self, owner: &TestUser, members: &[&TestUser]) -> String {
            for member in members {
                self.befriend(owner, member).await;
            }
            let reply: serde_json::Value = self
                .request(reqwest::Method::POST, "/v1/conversations/groups", owner)
                .json(&serde_json::json!({
                    "name": "Test group",
                    "member_ids": members.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            reply["id"].as_str().unwrap().to_owned()
        }

        async fn direct(&self, first: &TestUser, second: &TestUser) -> String {
            self.befriend(first, second).await;
            let reply: serde_json::Value = self
                .request(reqwest::Method::POST, "/v1/conversations/direct", first)
                .json(&serde_json::json!({"user_id": second.id}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            reply["id"].as_str().unwrap().to_owned()
        }

        async fn block(&self, blocker: &TestUser, target: &TestUser) {
            let status = self
                .request(
                    reqwest::Method::POST,
                    &format!("/v1/users/{}/block", target.id),
                    blocker,
                )
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
        }

        async fn public_user(&self, user: &TestUser) -> PublicUser {
            sqlx::query_as::<_, PublicUser>(
                "SELECT id,username,display_name,avatar_data FROM users WHERE id=?",
            )
            .bind(&user.id)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
        }

        async fn send_text(&self, user: &TestUser, conversation: &str, text: &str) -> String {
            let reply: serde_json::Value = self
                .request(
                    reqwest::Method::POST,
                    &format!("/v1/conversations/{conversation}/messages"),
                    user,
                )
                .json(&serde_json::json!({
                    "client_message_id": Uuid::new_v4().to_string(),
                    "content": text,
                }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            reply["id"].as_str().unwrap().to_owned()
        }

        async fn upload(
            &self,
            user: &TestUser,
            conversation: &str,
            bytes: Vec<u8>,
        ) -> reqwest::Response {
            self.request(
                reqwest::Method::POST,
                &format!(
                    "/v1/conversations/{conversation}/attachments?name=notes.txt&client_message_id={}",
                    Uuid::new_v4()
                ),
                user,
            )
            .header(header::CONTENT_TYPE, "text/plain")
            .body(bytes)
            .send()
            .await
            .unwrap()
        }
    }

    #[tokio::test]
    async fn require_redis_comes_from_state_not_process_env() {
        let required = TestApp::configured(true).await;
        let health = required
            .client
            .get(format!("{}/health", required.base))
            .send()
            .await
            .unwrap();
        assert!(health.status().is_success());
        let refused = required
            .client
            .post(format!("{}/v1/accounts", required.base))
            .json(&serde_json::json!({"username": "alice", "display_name": "alice"}))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let open = TestApp::new().await;
        let alice = open.user("alice").await;
        assert!(!alice.token.is_empty());
    }

    #[tokio::test]
    async fn message_routes_reject_non_member() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let mallory = app.user("mallory").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let message = app.send_text(&alice, &group, "hello").await;
        let upload: serde_json::Value = app
            .upload(&alice, &group, b"secret".to_vec())
            .await
            .json()
            .await
            .unwrap();
        let attachment = upload["content"]
            .as_str()
            .unwrap()
            .strip_prefix(ATTACHMENT_MARKER)
            .unwrap()
            .to_owned();
        let checks: Vec<(Method, String, Option<serde_json::Value>)> = vec![
            (
                Method::GET,
                format!("/v1/conversations/{group}/messages"),
                None,
            ),
            (
                Method::POST,
                format!("/v1/conversations/{group}/messages"),
                Some(serde_json::json!({"client_message_id":"x","content":"hi"})),
            ),
            (
                Method::PATCH,
                format!("/v1/messages/{message}"),
                Some(serde_json::json!({"content":"edited"})),
            ),
            (Method::POST, format!("/v1/messages/{message}/pin"), None),
            (Method::POST, format!("/v1/messages/{message}/save"), None),
            (
                Method::POST,
                format!("/v1/messages/{message}/reactions"),
                Some(serde_json::json!({"emoji":"👍"})),
            ),
            (
                Method::POST,
                format!("/v1/conversations/{group}/read"),
                Some(serde_json::json!({"sequence":0})),
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/reactions"),
                None,
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/members"),
                None,
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/crypto-events"),
                None,
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/crypto-lock"),
                None,
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/planner/polls"),
                None,
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/planner/events"),
                None,
            ),
            (
                Method::GET,
                format!("/v1/conversations/{group}/invites"),
                None,
            ),
            (Method::GET, format!("/v1/attachments/{attachment}"), None),
            (
                Method::POST,
                format!("/v1/attachments/{attachment}/link"),
                None,
            ),
        ];
        for (method, path, body) in checks {
            let mut request = app.request(method.clone(), &path, &mallory);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let status = request.send().await.unwrap().status();
            assert!(
                matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
                "{method} {path} returned {status} to a non-member"
            );
        }
        let status = app.upload(&mallory, &group, b"x".to_vec()).await.status();
        assert_eq!(status, StatusCode::NOT_FOUND);
        let saved: serde_json::Value = app
            .request(Method::GET, "/v1/me/saved-messages", &mallory)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(saved, serde_json::json!([]));
    }

    #[tokio::test]
    async fn attachment_upload_accepts_bodies_above_json_limit() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let response = app.upload(&alice, &group, vec![7u8; 512 * 1024]).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }

    #[tokio::test]
    async fn message_client_id_of_another_sender_is_refused() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let direct = app.direct(&alice, &bob).await;
        let elsewhere = app.direct(&bob, &carol).await;
        let client_id = Uuid::new_v4().to_string();
        let send = |user: &TestUser, conversation: &str, text: &str| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{conversation}/messages"),
                user,
            )
            .json(&serde_json::json!({"client_message_id": client_id, "content": text}))
            .send()
        };
        let count = || {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE client_message_id=?")
                .bind(&client_id)
                .fetch_one(&app.state.db)
        };

        let first = send(&alice, &direct, "first").await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value = first.json().await.unwrap();

        // Bob reuses Alice's client id in the same conversation: refused, so
        // no reader can mistake his message for hers.
        let status = send(&bob, &direct, "imposter").await.unwrap().status();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(count().await.unwrap(), 1);

        // Alice's retry still returns her existing row.
        let retry = send(&alice, &direct, "retried").await.unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        let retry: serde_json::Value = retry.json().await.unwrap();
        assert_eq!(retry["id"], first["id"]);
        assert_eq!(retry["content"], "first");
        assert_eq!(count().await.unwrap(), 1);

        // The same client id in another conversation is unrelated.
        let status = send(&bob, &elsewhere, "other").await.unwrap().status();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn a_message_encrypted_before_an_accepted_commit_is_refused() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete,commit_seq) VALUES(?,?,1,4)",
        )
        .bind(&group)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let send = |client_id: &str, seq: Option<i64>| {
            let mut body =
                serde_json::json!({"client_message_id": client_id, "content": "e2e1:AAAA"});
            if let Some(seq) = seq {
                body["expected_commit_seq"] = seq.into();
            }
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/messages"),
                &bob,
            )
            .json(&body)
            .send()
        };
        let count = || {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE conversation_id=?")
                .bind(&group)
                .fetch_one(&app.state.db)
        };
        // Encrypted on the group state the server is at: stored.
        let first = Uuid::new_v4().to_string();
        assert_eq!(
            send(&first, Some(4)).await.unwrap().status(),
            StatusCode::OK
        );
        // A membership commit is accepted. A message encrypted before it
        // cannot be read by devices that applied it, so it is not stored and
        // the sender is told to sync and encrypt again.
        sqlx::query("UPDATE conversation_crypto_locks SET commit_seq=5 WHERE conversation_id=?")
            .bind(&group)
            .execute(&app.state.db)
            .await
            .unwrap();
        let second = Uuid::new_v4().to_string();
        let refused = send(&second, Some(4)).await.unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "stale_commit_seq", "{body}");
        assert_eq!(count().await.unwrap(), 1);
        // A retry of the message the server already stored still succeeds.
        assert_eq!(
            send(&first, Some(4)).await.unwrap().status(),
            StatusCode::OK
        );
        // Encrypted again on the current state it is stored; a client that
        // names no state (an older one) is served as before.
        assert_eq!(
            send(&second, Some(5)).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            send(&Uuid::new_v4().to_string(), None)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn message_edit_rejects_attachment_marker() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let upload: serde_json::Value = app
            .upload(&bob, &group, b"bob's file".to_vec())
            .await
            .json()
            .await
            .unwrap();
        let attachment = upload["content"].as_str().unwrap().to_owned();
        let carol_message = app.send_text(&carol, &group, "innocent text").await;
        let status = app
            .request(
                Method::PATCH,
                &format!("/v1/messages/{carol_message}"),
                &carol,
            )
            .json(&serde_json::json!({"content": attachment}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let status = app
            .request(
                Method::DELETE,
                &format!("/v1/messages/{carol_message}"),
                &carol,
            )
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let attachment_id = attachment.strip_prefix(ATTACHMENT_MARKER).unwrap();
        let download = app
            .request(
                Method::GET,
                &format!("/v1/attachments/{attachment_id}"),
                &alice,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(download.bytes().await.unwrap().as_ref(), b"bob's file");
    }

    #[tokio::test]
    async fn message_edit_rejects_removed_member() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let carol_message = app.send_text(&carol, &group, "original").await;
        let status = app
            .request(
                Method::DELETE,
                &format!("/v1/conversations/{group}/members/{}", carol.id),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let status = app
            .request(
                Method::PATCH,
                &format!("/v1/messages/{carol_message}"),
                &carol,
            )
            .json(&serde_json::json!({"content": "rewritten after removal"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NOT_FOUND);
        let messages: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/messages"),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(messages[0]["content"], "original");
    }

    #[tokio::test]
    async fn logout_keeps_other_sessions_connected() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let laptop = app.user("alice").await;
        let desktop = TestUser {
            id: laptop.id.clone(),
            token: add_session(&app.state.db, &laptop.id).await.unwrap(),
        };
        let mut events = app.state.events.subscribe();
        let status = app
            .request(Method::DELETE, "/v1/sessions", &laptop)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.event["type"], "session_revoked");
        assert!(session_revocation_applies(
            &event.event,
            &session_hash(&laptop.token)
        ));
        assert!(
            !session_revocation_applies(&event.event, &session_hash(&desktop.token)),
            "logging out one device must not sign out the account's other sessions"
        );
        let status = app
            .request(Method::GET, "/v1/me", &desktop)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn presence_hides_invisible_mode_from_friends() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        app.befriend(&alice, &bob).await;
        // Alice has the app open (an authenticated WebSocket).
        register_online_connection(&app.state, &alice.id);
        let mut events = app.state.events.subscribe();
        let status = app
            .request(Method::PUT, "/v1/me/presence", &alice)
            .json(&serde_json::json!({"mode": "invisible"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK);
        let mut seen_by_bob = None;
        let mut seen_by_alice = None;
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(500), events.recv()).await
        {
            if event.event["type"] != "presence_updated" {
                continue;
            }
            if event.recipients.contains(&bob.id) {
                seen_by_bob = Some(event.event["mode"].clone());
            }
            if event.recipients.contains(&alice.id) {
                seen_by_alice = Some(event.event["mode"].clone());
            }
        }
        assert_eq!(seen_by_bob, Some(serde_json::json!("offline")));
        assert_eq!(seen_by_alice, Some(serde_json::json!("invisible")));
        let rows: serde_json::Value = app
            .request(Method::GET, "/v1/me/presence", &bob)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let alice_row = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["user_id"] == alice.id.as_str())
            .unwrap();
        assert_eq!(alice_row["mode"], "offline");
        let rows: serde_json::Value = app
            .request(Method::GET, "/v1/me/presence", &alice)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let own_row = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["user_id"] == alice.id.as_str())
            .unwrap();
        assert_eq!(own_row["mode"], "invisible");
    }

    #[tokio::test]
    async fn presence_connect_and_disconnect_of_invisible_user_send_friends_nothing() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        app.befriend(&alice, &bob).await;
        let status = app
            .request(Method::PUT, "/v1/me/presence", &alice)
            .json(&serde_json::json!({"mode": "invisible"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK);
        let mut events = app.state.events.subscribe();
        // Alice opens the app (first WebSocket) and later closes it.
        assert!(register_online_connection(&app.state, &alice.id));
        broadcast_current_presence(&app.state, &alice.id).await;
        assert!(unregister_online_connection(&app.state, &alice.id));
        broadcast_current_presence(&app.state, &alice.id).await;
        let mut own_events = 0;
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(300), events.recv()).await
        {
            assert!(
                !event.recipients.contains(&bob.id),
                "friend was notified of an invisible user's connect/disconnect: {}",
                event.event
            );
            if event.recipients.contains(&alice.id) {
                own_events += 1;
            }
        }
        assert_eq!(own_events, 2, "the account itself still gets its presence");
    }

    #[tokio::test]
    async fn message_edit_rejects_blocked_direct_peer() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        app.befriend(&alice, &bob).await;
        let direct: serde_json::Value = app
            .request(Method::POST, "/v1/conversations/direct", &alice)
            .json(&serde_json::json!({"user_id": bob.id}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let direct = direct["id"].as_str().unwrap().to_owned();
        let message = app.send_text(&alice, &direct, "earlier message").await;
        let status = app
            .request(Method::POST, &format!("/v1/users/{}/block", alice.id), &bob)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let mut events = app.state.events.subscribe();
        let status = app
            .request(Method::PATCH, &format!("/v1/messages/{message}"), &alice)
            .json(&serde_json::json!({"content": "rewritten after block"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::FORBIDDEN);
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(300), events.recv()).await
        {
            assert!(
                !(event.recipients.contains(&bob.id) && event.event["type"] == "message"),
                "blocker received an edited message"
            );
        }
        let messages: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{direct}/messages"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(messages[0]["content"], "earlier message");
    }

    #[tokio::test]
    async fn account_deletion_keeps_other_members_groups() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        // Carol owns two groups: in the first Bob, the newer member, is an
        // admin and wins over the older plain member Alice; in the second
        // nobody is, so ownership falls to the oldest remaining member. The
        // founders are given with the larger ID first, so join order and ID
        // order disagree.
        let with_admin = app.group(&carol, &[&alice, &bob]).await;
        let (oldest, newest) = if alice.id > bob.id {
            (&alice, &bob)
        } else {
            (&bob, &alice)
        };
        let without_admin = {
            let reply: serde_json::Value = app
                .request(Method::POST, "/v1/conversations/groups", &carol)
                .json(&serde_json::json!({"name": "Second", "member_ids": [oldest.id, newest.id]}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            reply["id"].as_str().unwrap().to_owned()
        };
        let status = app
            .request(
                Method::PATCH,
                &format!("/v1/conversations/{with_admin}/members/{}", bob.id),
                &carol,
            )
            .json(&serde_json::json!({"role": "admin"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        app.send_text(&alice, &with_admin, "alice keeps this").await;
        app.send_text(&carol, &with_admin, "carol's own message")
            .await;
        let upload: serde_json::Value = app
            .upload(&alice, &with_admin, b"alice's file".to_vec())
            .await
            .json()
            .await
            .unwrap();
        let attachment = upload["content"]
            .as_str()
            .unwrap()
            .strip_prefix(ATTACHMENT_MARKER)
            .unwrap()
            .to_owned();
        // A group Carol is alone in is removed with her.
        let solo = app.group(&carol, &[&alice, &bob]).await;
        for member in [&alice, &bob] {
            let status = app
                .request(
                    Method::DELETE,
                    &format!("/v1/conversations/{solo}/members/{}", member.id),
                    member,
                )
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::NO_CONTENT);
        }

        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);

        let messages = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{with_admin}/messages"),
                &alice,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(messages.status(), StatusCode::OK);
        let messages: serde_json::Value = messages.json().await.unwrap();
        let contents = messages
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(contents.contains(&"alice keeps this".to_owned()));
        assert!(!contents.contains(&"carol's own message".to_owned()));
        let download = app
            .request(
                Method::GET,
                &format!("/v1/attachments/{attachment}"),
                &alice,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(download.bytes().await.unwrap().as_ref(), b"alice's file");

        let role_of = |conversation: String, user: String| {
            let db = app.state.db.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT role FROM conversation_members WHERE conversation_id=? AND user_id=?",
                )
                .bind(conversation)
                .bind(user)
                .fetch_one(&db)
                .await
                .unwrap()
            }
        };
        assert_eq!(role_of(with_admin.clone(), bob.id.clone()).await, "owner");
        assert_eq!(
            role_of(with_admin.clone(), alice.id.clone()).await,
            "member"
        );
        // Founders join in the order given (creator first), whatever their IDs.
        assert_eq!(
            role_of(without_admin.clone(), oldest.id.clone()).await,
            "owner"
        );
        assert_eq!(
            role_of(without_admin.clone(), newest.id.clone()).await,
            "member"
        );
        let remaining =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversations WHERE id IN (?,?,?)")
                .bind(&with_admin)
                .bind(&without_admin)
                .bind(&solo)
                .fetch_one(&app.state.db)
                .await
                .unwrap();
        assert_eq!(
            remaining, 2,
            "only the group with no members left is deleted"
        );
    }

    #[tokio::test]
    async fn owner_or_admin_removes_deleted_account_from_encrypted_group() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let dave = app.user("dave").await;
        let mallory = app.user("mallory").await;
        let group = app.group(&alice, &[&bob, &carol, &dave]).await;
        let plain_group = app.group(&alice, &[&bob, &carol]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&group)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);

        let deleted_ids = |conversation: String, viewer: &TestUser| {
            app.request(
                Method::GET,
                &format!("/v1/conversations/{conversation}/crypto-lock"),
                viewer,
            )
            .send()
        };
        let lock: serde_json::Value = deleted_ids(group.clone(), &bob)
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(lock["deletedMemberIds"], serde_json::json!([carol.id]));
        let plain: serde_json::Value = deleted_ids(plain_group.clone(), &bob)
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(plain["deletedMemberIds"], serde_json::json!([]));

        let mut remaining = vec![alice.id.clone(), bob.id.clone(), dave.id.clone()];
        remaining.sort();
        let remove = |actor: &TestUser, target: &str, operation: String| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/crypto-membership/remove/{target}"),
                actor,
            )
            .json(&serde_json::json!({
                "operation_id": operation,
                "expected_member_ids": remaining,
                "expected_commit_seq": 0,
                "commit_payload": "AAAA",
                "welcome_payload": "",
            }))
            .send()
        };
        // A plain member cannot remove the deleted account.
        let status = remove(&bob, &carol.id, Uuid::new_v4().to_string())
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::FORBIDDEN);
        // An account that was never a member, and an unknown ID, are not
        // recorded deleted members.
        for target in [mallory.id.clone(), Uuid::new_v4().to_string()] {
            let status = remove(&alice, &target, Uuid::new_v4().to_string())
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::NOT_FOUND, "target {target}");
        }
        // The owner's removal is accepted once and relayed to the others.
        let operation = Uuid::new_v4().to_string();
        let status = remove(&alice, &carol.id, operation.clone())
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let status = remove(&alice, &carol.id, operation).await.unwrap().status();
        assert_eq!(status, StatusCode::NO_CONTENT, "a retry is idempotent");
        let recipients = sqlx::query_scalar::<_, String>(
            "SELECT recipient_id FROM crypto_events WHERE conversation_id=? ORDER BY recipient_id",
        )
        .bind(&group)
        .fetch_all(&app.state.db)
        .await
        .unwrap();
        let mut expected_recipients = vec![bob.id.clone(), dave.id.clone()];
        expected_recipients.sort();
        assert_eq!(recipients, expected_recipients);
        // A second, different removal commit for the same leaf is refused
        // as already done (410), so its sender can drop its staged copy.
        let status = remove(&alice, &carol.id, Uuid::new_v4().to_string())
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::GONE);
        let lock: serde_json::Value = deleted_ids(group.clone(), &bob)
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(lock["deletedMembers"][0]["userId"], carol.id.as_str());
        assert!(lock["deletedMembers"][0]["removalCommittedAt"].is_string());
        // The record stays visible so older-epoch messages still decrypt.
        let lock: serde_json::Value = deleted_ids(group.clone(), &dave)
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(lock["deletedMemberIds"], serde_json::json!([carol.id]));
        let members = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_members WHERE conversation_id=?",
        )
        .bind(&group)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
        assert_eq!(members, 3);
    }

    #[tokio::test]
    async fn share_token_rejects_blocked_direct_peer() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        let path = format!("/v1/conversations/{direct}/share-token");
        // Without a block the route reaches the LiveKit check (not configured).
        let status = app
            .request(Method::POST, &path, &alice)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        app.block(&bob, &alice).await;
        for user in [&alice, &bob] {
            let status = app
                .request(Method::POST, &path, user)
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn typing_is_not_relayed_across_a_block() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        let sender = app.public_user(&alice).await;
        let signal =
            serde_json::json!({"type": "typing", "conversation_id": direct, "active": true})
                .to_string();
        let mut events = app.state.events.subscribe();
        relay_typing_signal(&app.state, &sender, &signal).await;
        let event = tokio::time::timeout(Duration::from_millis(300), events.recv())
            .await
            .expect("typing is relayed without a block")
            .unwrap();
        assert_eq!(event.event["type"], "typing");
        app.block(&bob, &alice).await;
        let mut events = app.state.events.subscribe();
        relay_typing_signal(&app.state, &sender, &signal).await;
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(300), events.recv()).await
        {
            assert_ne!(event.event["type"], "typing", "typing crossed a block");
        }
    }

    #[tokio::test]
    async fn poll_votes_are_rejected_after_closes_at() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let create = |closes_at: serde_json::Value| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/planner/polls"),
                &alice,
            )
            .json(&serde_json::json!({
                "question": "When?", "options": ["Mon", "Tue"], "closes_at": closes_at,
            }))
            .send()
        };
        let status = create(serde_json::json!("not a time"))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        create(serde_json::json!("2000-01-01T00:00:00.000Z"))
            .await
            .unwrap();
        let polls: serde_json::Value = create(serde_json::json!("2999-01-01T00:00:00.000Z"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let vote = |poll: &serde_json::Value| {
            app.request(
                Method::POST,
                &format!("/v1/group-polls/{}/vote", poll["id"].as_str().unwrap()),
                &bob,
            )
            .json(&serde_json::json!({"option_id": poll["options"][0]["id"]}))
            .send()
        };
        let polls = polls.as_array().unwrap();
        let expired = polls
            .iter()
            .find(|p| p["closes_at"] == "2000-01-01T00:00:00.000Z")
            .unwrap();
        let open = polls
            .iter()
            .find(|p| p["closes_at"] == "2999-01-01T00:00:00.000Z")
            .unwrap();
        assert_eq!(vote(expired).await.unwrap().status(), StatusCode::CONFLICT);
        assert_eq!(vote(open).await.unwrap().status(), StatusCode::NO_CONTENT);
        let votes = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM group_poll_votes")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
        assert_eq!(votes, 1);

        // A row stored before closing times were validated can hold a value
        // that is not a time. It has no deadline that can be enforced, so it
        // takes votes as it always did instead of failing the request.
        let polls: serde_json::Value = create(serde_json::Value::Null)
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let legacy = polls
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["closes_at"].is_null())
            .unwrap()
            .clone();
        sqlx::query("UPDATE group_polls SET closes_at='not a time' WHERE id=?")
            .bind(legacy["id"].as_str().unwrap())
            .execute(&app.state.db)
            .await
            .unwrap();
        assert_eq!(
            vote(&legacy).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let votes = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM group_poll_votes")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
        assert_eq!(votes, 2);
    }

    #[tokio::test]
    async fn unblock_is_rate_limited_like_block() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let path = format!("/v1/users/{}/block", bob.id);
        let mut limited = false;
        for _ in 0..61 {
            let status = app
                .request(Method::DELETE, &path, &alice)
                .send()
                .await
                .unwrap()
                .status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                limited = true;
                break;
            }
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        assert!(limited, "61 unblock requests in an hour were all accepted");
    }

    #[tokio::test]
    async fn user_search_treats_like_wildcards_literally() {
        use reqwest::Method;
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        app.user("bob_one").await;
        app.user("bobtwo").await;
        let search = |q: &str| {
            let q = q.replace('%', "%25");
            app.request(Method::GET, &format!("/v1/users/search?q={q}"), &alice)
                .send()
        };
        let names = |rows: serde_json::Value| -> Vec<String> {
            rows.as_array()
                .unwrap()
                .iter()
                .map(|row| row["username"].as_str().unwrap().to_owned())
                .collect()
        };
        let all: serde_json::Value = search("__").await.unwrap().json().await.unwrap();
        assert!(names(all).is_empty(), "`__` must not match every username");
        let percent: serde_json::Value = search("b%").await.unwrap().json().await.unwrap();
        assert!(names(percent).is_empty());
        let literal: serde_json::Value = search("bob_").await.unwrap().json().await.unwrap();
        assert_eq!(names(literal), vec!["bob_one".to_owned()]);
    }

    #[tokio::test]
    async fn encrypted_removal_records_its_target_and_rejects_another_declared_one() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let dave = app.user("dave").await;
        let group = app.group(&alice, &[&bob, &carol, &dave]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&group)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let mut roster = vec![
            alice.id.clone(),
            bob.id.clone(),
            carol.id.clone(),
            dave.id.clone(),
        ];
        roster.sort();
        let remove = |declared: &str, operation: &str| {
            app.request(
                Method::POST,
                &format!(
                    "/v1/conversations/{group}/crypto-membership/remove/{}",
                    dave.id
                ),
                &alice,
            )
            .json(&serde_json::json!({
                "operation_id": operation,
                "expected_member_ids": roster,
                "expected_commit_seq": 0,
                "member_ids": [declared],
                "commit_payload": "AAAA",
                "welcome_payload": "",
            }))
            .send()
        };
        let status = remove(&carol.id, &Uuid::new_v4().to_string())
            .await
            .unwrap()
            .status();
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the commit declares another target"
        );
        let operation = Uuid::new_v4().to_string();
        let status = remove(&dave.id, &operation).await.unwrap().status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let target = sqlx::query_scalar::<_, Option<String>>(
            "SELECT target_id FROM crypto_membership_operations WHERE conversation_id=? AND operation_id=?",
        )
        .bind(&group)
        .bind(&operation)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
        assert_eq!(target.as_deref(), Some(dave.id.as_str()));
    }

    #[tokio::test]
    async fn deleting_an_encrypted_message_deletes_its_encrypted_attachment() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&direct)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let upload = |user: &TestUser| {
            let attachment_id = Uuid::new_v4().to_string();
            let request = app
                .request(
                    Method::POST,
                    &format!(
                        "/v1/conversations/{direct}/encrypted-attachments?attachment_id={attachment_id}&plain_size=0&chunk_size={ENCRYPTED_ATTACHMENT_CHUNK_BYTES}&chunk_count=1"
                    ),
                    user,
                )
                .body(vec![7u8; 16])
                .send();
            async move {
                assert_eq!(request.await.unwrap().status(), StatusCode::OK);
                attachment_id
            }
        };
        let send = |user: &TestUser, attachment_ids: Vec<String>| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{direct}/messages"),
                user,
            )
            .json(&serde_json::json!({
                "client_message_id": Uuid::new_v4().to_string(),
                "content": "e2e1:AAAA",
                "attachment_ids": attachment_ids,
            }))
            .send()
        };
        let blob = |id: &str| app.state.attachment_dir.join(format!("{id}.blob"));

        // Another member's upload cannot be claimed by a message.
        let bobs = upload(&bob).await;
        let status = send(&alice, vec![bobs.clone()]).await.unwrap().status();
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let attachment = upload(&alice).await;
        let message: serde_json::Value = send(&alice, vec![attachment.clone()])
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        // An attachment links to one message only.
        let status = send(&alice, vec![attachment.clone()])
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(blob(&attachment).exists());
        let status = app
            .request(
                Method::DELETE,
                &format!("/v1/messages/{}", message["id"].as_str().unwrap()),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(
            !blob(&attachment).exists(),
            "the encrypted blob is deleted with its message"
        );
        let rows = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE id=?")
            .bind(&attachment)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        assert!(blob(&bobs).exists(), "an unrelated upload is kept");
    }

    #[tokio::test]
    async fn the_sweep_deletes_only_old_unclaimed_encrypted_uploads_made_after_0026() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&direct)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let upload = || {
            let attachment_id = Uuid::new_v4().to_string();
            let request = app
                .request(
                    Method::POST,
                    &format!(
                        "/v1/conversations/{direct}/encrypted-attachments?attachment_id={attachment_id}&plain_size=0&chunk_size={ENCRYPTED_ATTACHMENT_CHUNK_BYTES}&chunk_count=1"
                    ),
                    &alice,
                )
                .body(vec![7u8; 16])
                .send();
            async move {
                assert_eq!(request.await.unwrap().status(), StatusCode::OK);
                attachment_id
            }
        };
        let blob = |id: &str| app.state.attachment_dir.join(format!("{id}.blob"));
        let rows = |id: String| {
            let db = app.state.db.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE id=?")
                    .bind(id)
                    .fetch_one(&db)
                    .await
                    .unwrap()
            }
        };
        let age = |id: String, days: i64| {
            let db = app.state.db.clone();
            async move {
                sqlx::query(
                    "UPDATE attachments SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now',?) WHERE id=?",
                )
                .bind(format!("-{days} days"))
                .bind(id)
                .execute(&db)
                .await
                .unwrap();
            }
        };
        // As on a server that has been running: 0026 was applied three days ago.
        sqlx::query(
            "UPDATE _sqlx_migrations SET installed_on=datetime('now','-3 days') WHERE version=26",
        )
        .execute(&app.state.db)
        .await
        .unwrap();

        let abandoned = upload().await;
        age(abandoned.clone(), 2).await;
        let young = upload().await;
        let claimed = upload().await;
        let status = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{direct}/messages"),
                &alice,
            )
            .json(&serde_json::json!({
                "client_message_id": Uuid::new_v4().to_string(),
                "content": "e2e1:AAAA",
                "attachment_ids": [claimed.clone()],
            }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK);
        age(claimed.clone(), 2).await;
        // Uploaded before 0026: an encrypted message the server cannot read
        // may still point to it, so it is never swept.
        let legacy = upload().await;
        age(legacy.clone(), 4).await;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let removed = sweep_unclaimed_encrypted_uploads(&app.state, now)
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(rows(abandoned.clone()).await, 0);
        assert!(!blob(&abandoned).exists(), "the abandoned blob is deleted");
        for kept in [&young, &claimed, &legacy] {
            assert_eq!(rows(kept.clone()).await, 1);
            assert!(blob(kept).exists());
        }

        // Two days later the young upload is abandoned too; the claimed and
        // the pre-0026 uploads still stay.
        let removed = sweep_unclaimed_encrypted_uploads(&app.state, now + 2 * 86_400)
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(rows(young.clone()).await, 0);
        assert!(!blob(&young).exists());
        for kept in [&claimed, &legacy] {
            assert_eq!(rows(kept.clone()).await, 1);
            assert!(blob(kept).exists());
        }
    }

    #[tokio::test]
    async fn account_deletion_tells_direct_peers_the_conversation_is_gone() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        let mut events = app.state.events.subscribe();
        let status = app
            .request(Method::DELETE, "/v1/me/account", &bob)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let mut told = false;
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(300), events.recv()).await
        {
            if event.recipients.contains(&alice.id)
                && event.event["type"] == "conversation_updated"
                && event.event["conversation_id"] == direct.as_str()
            {
                told = true;
            }
        }
        assert!(
            told,
            "the peer must learn that the direct conversation was removed"
        );
    }

    // Minimal RFC 6455 client over a raw TcpStream (no extra crates): enough
    // to authenticate a session socket and read the server's frames.
    struct RawWebSocket(tokio::net::TcpStream);

    impl RawWebSocket {
        async fn connect(base: &str, token: &str) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let addr = base.trim_start_matches("http://");
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let request = format!(
                "GET /v1/ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = Vec::new();
            while !response.ends_with(b"\r\n\r\n") {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                response.push(byte[0]);
            }
            assert!(
                response.starts_with(b"HTTP/1.1 101"),
                "{}",
                String::from_utf8_lossy(&response)
            );
            let mut socket = Self(stream);
            socket
                .send_text(&serde_json::json!({ "token": token }).to_string())
                .await;
            let (opcode, ready) = socket.read_frame().await;
            assert_eq!(opcode, 1);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&ready).unwrap()["type"],
                "ready"
            );
            socket
        }

        async fn send_text(&mut self, text: &str) {
            use tokio::io::AsyncWriteExt;
            let payload = text.as_bytes();
            let mask = [0x12u8, 0x34, 0x56, 0x78];
            let mut frame = vec![0x81u8];
            if payload.len() < 126 {
                frame.push(0x80 | payload.len() as u8);
            } else {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            frame.extend_from_slice(&mask);
            frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            self.0.write_all(&frame).await.unwrap();
        }

        async fn read_frame(&mut self) -> (u8, Vec<u8>) {
            use tokio::io::AsyncReadExt;
            let mut head = [0u8; 2];
            self.0.read_exact(&mut head).await.unwrap();
            let mut len = u64::from(head[1] & 0x7f);
            if len == 126 {
                let mut ext = [0u8; 2];
                self.0.read_exact(&mut ext).await.unwrap();
                len = u64::from(u16::from_be_bytes(ext));
            } else if len == 127 {
                let mut ext = [0u8; 8];
                self.0.read_exact(&mut ext).await.unwrap();
                len = u64::from_be_bytes(ext);
            }
            let mut payload = vec![0u8; len as usize];
            self.0.read_exact(&mut payload).await.unwrap();
            (head[0] & 0x0f, payload)
        }

        /// The next text frame's JSON within `wait`, skipping other frames.
        async fn next_event(&mut self, wait: Duration) -> Option<serde_json::Value> {
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let frame = tokio::time::timeout_at(deadline, self.read_frame())
                    .await
                    .ok()?;
                match frame {
                    (1, text) => return serde_json::from_slice(&text).ok(),
                    (8, _) => return Some(serde_json::json!({ "type": "__closed" })),
                    _ => continue,
                }
            }
        }
    }

    #[tokio::test]
    async fn logout_revokes_only_the_logged_out_socket() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let laptop = app.user("alice").await;
        let desktop = TestUser {
            id: laptop.id.clone(),
            token: add_session(&app.state.db, &laptop.id).await.unwrap(),
        };
        let mut laptop_socket = RawWebSocket::connect(&app.base, &laptop.token).await;
        let mut desktop_socket = RawWebSocket::connect(&app.base, &desktop.token).await;
        let status = app
            .request(Method::DELETE, "/v1/sessions", &laptop)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Skip the socket's own presence events.
        let mut revoked = None;
        while let Some(event) = laptop_socket.next_event(Duration::from_secs(2)).await {
            if event["type"] != "presence_updated" {
                revoked = Some(event);
                break;
            }
        }
        assert_eq!(
            revoked.as_ref().map(|event| event["type"].clone()),
            Some(serde_json::json!("session_revoked"))
        );
        let closed = laptop_socket.next_event(Duration::from_secs(2)).await;
        assert_eq!(
            closed.map(|event| event["type"].clone()),
            Some(serde_json::json!("__closed")),
            "the logged-out socket is closed"
        );
        // The other device's socket gets neither event and still relays.
        while let Some(event) = desktop_socket.next_event(Duration::from_millis(500)).await {
            assert!(
                event["type"] != "session_revoked" && event["type"] != "__closed",
                "the other session's socket was revoked: {event}"
            );
        }
        let _ = app.state.events.send(ServerEvent {
            recipients: vec![desktop.id.clone()],
            event: serde_json::json!({"type": "contacts_updated"}),
        });
        let mut relayed = false;
        while let Some(event) = desktop_socket.next_event(Duration::from_secs(2)).await {
            if event["type"] == "contacts_updated" {
                relayed = true;
                break;
            }
        }
        assert!(relayed, "the other session's socket still relays events");
    }

    #[tokio::test]
    async fn caller_gets_the_same_relay_reply_from_invisible_and_offline_callees() {
        use reqwest::Method;
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let to_bob = app.direct(&alice, &bob).await;
        let to_carol = app.direct(&alice, &carol).await;
        let status = app
            .request(Method::PUT, "/v1/me/presence", &bob)
            .json(&serde_json::json!({"mode": "invisible"}))
            .send()
            .await
            .unwrap()
            .status();
        assert!(status.is_success());
        // Bob is connected but invisible; Carol is offline.
        register_online_connection(&app.state, &bob.id);
        let caller = app.public_user(&alice).await;
        let mut observed = Vec::new();
        for (conversation, callee) in [(&to_bob, &bob), (&to_carol, &carol)] {
            let mut events = app.state.events.subscribe();
            let offer = serde_json::json!({
                "type": "voice_signal", "conversation_id": conversation, "to_user_id": callee.id,
                "kind": "offer", "payload": {"type": "offer", "sdp": "v=0"},
            });
            relay_voice_signal(&app.state, &caller, &offer.to_string()).await;
            let mut to_caller = Vec::new();
            let mut to_callee = 0;
            while let Ok(Ok(event)) =
                tokio::time::timeout(Duration::from_millis(300), events.recv()).await
            {
                if event.recipients.contains(&alice.id) {
                    to_caller.push(event.event.clone());
                }
                if event.recipients.contains(&callee.id) && event.event["kind"] == "offer" {
                    to_callee += 1;
                }
            }
            assert_eq!(to_callee, 1, "the callee's devices receive the call");
            observed.push(to_caller);
        }
        assert_eq!(
            observed[0], observed[1],
            "invisible and offline look the same to the caller"
        );
        assert!(observed[0].is_empty());
    }

    #[tokio::test]
    async fn account_deletion_keeps_planner_items_with_an_anonymous_creator() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let polls: serde_json::Value = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{group}/planner/polls"),
                &carol,
            )
            .json(&serde_json::json!({"question": "When?", "options": ["Mon", "Tue"], "closes_at": null}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let poll = polls[0].clone();
        let events: serde_json::Value = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{group}/planner/events"),
                &carol,
            )
            .json(&serde_json::json!({"title": "Dinner", "details": "", "scheduled_at": "2999-01-01T19:00:00Z"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let event = events[0].clone();
        for (user, option) in [(&alice, 0), (&carol, 1)] {
            let status = app
                .request(
                    Method::POST,
                    &format!("/v1/group-polls/{}/vote", poll["id"].as_str().unwrap()),
                    user,
                )
                .json(&serde_json::json!({"option_id": poll["options"][option]["id"]}))
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::NO_CONTENT);
            let status = app
                .request(
                    Method::POST,
                    &format!("/v1/group-events/{}/rsvp", event["id"].as_str().unwrap()),
                    user,
                )
                .json(&serde_json::json!({"status": "going"}))
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let polls: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/planner/polls"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            polls.as_array().unwrap().len(),
            1,
            "the poll survives its creator"
        );
        assert_eq!(polls[0]["creator_id"], serde_json::Value::Null);
        let votes: Vec<i64> = polls[0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["votes"].as_i64().unwrap())
            .collect();
        assert_eq!(
            votes,
            vec![1, 0],
            "Alice's vote stays, the deleted account's vote is gone"
        );
        let events: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/planner/events"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            events.as_array().unwrap().len(),
            1,
            "the event survives its creator"
        );
        assert_eq!(events[0]["creator_id"], serde_json::Value::Null);
        assert_eq!(events[0]["going"], 1);
    }

    #[tokio::test]
    async fn owner_takes_over_an_unfinished_setup_of_a_deleted_account() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let dave = app.user("dave").await;
        let group = app.group(&alice, &[&bob, &carol, &dave]).await;
        for (user, role) in [(&bob, "admin"), (&carol, "admin")] {
            let status = app
                .request(
                    Method::PATCH,
                    &format!("/v1/conversations/{group}/members/{}", user.id),
                    &alice,
                )
                .json(&serde_json::json!({"role": role}))
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        // Admin Carol starts encryption setup, then deletes her account.
        let mut roster = vec![
            alice.id.clone(),
            bob.id.clone(),
            carol.id.clone(),
            dave.id.clone(),
        ];
        roster.sort();
        let lock = |user: &TestUser, roster: Vec<String>| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/crypto-lock"),
                user,
            )
            .json(&serde_json::json!({"member_ids": roster}))
            .send()
        };
        assert_eq!(
            lock(&carol, roster.clone()).await.unwrap().status(),
            StatusCode::OK
        );
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let mut remaining = vec![alice.id.clone(), bob.id.clone(), dave.id.clone()];
        remaining.sort();
        let status: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-lock"),
                &dave,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(status["lockedByDeletedAccount"], true);
        // A plain member and an admin (while there is an owner) may not.
        assert_eq!(
            lock(&dave, remaining.clone()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            lock(&bob, remaining.clone()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        // The owner takes the setup over and can complete it.
        let taken: serde_json::Value = lock(&alice, remaining.clone())
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(taken["lockedBy"], alice.id.as_str());
        assert_eq!(taken["lockedByDeletedAccount"], false);
        assert_eq!(taken["setupComplete"], false);
        let status = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{group}/crypto-lock/complete"),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK);
        // Without an owner, the oldest admin may take over.
        let other = app.group(&alice, &[&bob, &dave]).await;
        let mut other_roster = vec![alice.id.clone(), bob.id.clone(), dave.id.clone()];
        other_roster.sort();
        sqlx::query("INSERT INTO conversation_crypto_locks(conversation_id,locked_by) VALUES(?,?)")
            .bind(&other)
            .bind(Uuid::new_v4().to_string())
            .execute(&app.state.db)
            .await
            .unwrap();
        sqlx::query("UPDATE conversation_members SET role='admin' WHERE conversation_id=? AND user_id IN (?,?)")
            .bind(&other)
            .bind(&alice.id)
            .bind(&bob.id)
            .execute(&app.state.db)
            .await
            .unwrap();
        let lock_other = |user: &TestUser| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{other}/crypto-lock"),
                user,
            )
            .json(&serde_json::json!({"member_ids": other_roster}))
            .send()
        };
        assert_eq!(
            lock_other(&bob).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "Bob joined after Alice"
        );
        assert_eq!(lock_other(&alice).await.unwrap().status(), StatusCode::OK);
    }

    /// A database migrated only up to `last_version`, for upgrade tests:
    /// seed it, close it, then let `open_database` apply the rest.
    async fn database_migrated_to(path: &std::path::Path, last_version: i64) -> SqlitePool {
        let all = sqlx::migrate!("./migrations");
        let migrations: Vec<_> = all
            .iter()
            .filter(|migration| migration.version <= last_version)
            .cloned()
            .collect();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(migrations),
            ..sqlx::migrate::Migrator::DEFAULT
        }
        .run(&pool)
        .await
        .unwrap();
        pool
    }

    fn backup_files(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".bak"))
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn upgrading_past_0025_writes_one_backup_of_the_old_database() {
        let dir = std::env::temp_dir().join(format!("ghostchat-backup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let old = database_migrated_to(&path, 24).await;
        seed_user(&old, "u-before").await;
        old.close().await;

        let db = open_database(&path.to_string_lossy()).await.unwrap();
        db.close().await;
        assert_eq!(backup_files(&dir), vec!["db.sqlite.pre-0025.bak"]);

        // The backup is a plain SQLite database at the old level with the old data.
        let backup = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(dir.join("db.sqlite.pre-0025.bak"))
                    .read_only(true),
            )
            .await
            .unwrap();
        let integrity = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&backup)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");
        let level = sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&backup)
            .await
            .unwrap();
        assert_eq!(level, 24);
        let users = sqlx::query_scalar::<_, String>("SELECT id FROM users")
            .fetch_all(&backup)
            .await
            .unwrap();
        assert_eq!(users, vec!["u-before".to_string()]);
        let target_column = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pragma_table_info('crypto_membership_operations') WHERE name='target_id'",
        )
        .fetch_one(&backup)
        .await
        .unwrap();
        assert_eq!(target_column, 0, "the backup must hold the pre-0025 schema");
        backup.close().await;

        // A second start has nothing pending and writes no second backup.
        let again = open_database(&path.to_string_lossy()).await.unwrap();
        again.close().await;
        assert_eq!(backup_files(&dir), vec!["db.sqlite.pre-0025.bak"]);
    }

    #[tokio::test]
    async fn a_fresh_database_gets_no_upgrade_backup() {
        let dir = std::env::temp_dir().join(format!("ghostchat-backup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = open_database(&dir.join("db.sqlite").to_string_lossy())
            .await
            .unwrap();
        db.close().await;
        assert!(backup_files(&dir).is_empty());
    }

    #[tokio::test]
    async fn an_existing_upgrade_backup_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("ghostchat-backup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        database_migrated_to(&path, 26).await.close().await;
        let existing = dir.join("db.sqlite.pre-0027.bak");
        std::fs::write(&existing, b"earlier backup").unwrap();
        open_database(&path.to_string_lossy())
            .await
            .unwrap()
            .close()
            .await;
        assert_eq!(std::fs::read(&existing).unwrap(), b"earlier backup");
        assert_eq!(backup_files(&dir), vec!["db.sqlite.pre-0027.bak"]);
    }

    #[tokio::test]
    async fn upgrade_backups_are_rotated_to_the_three_newest_after_a_new_one() {
        for (existing, expected) in [
            (
                vec![
                    "db.sqlite.pre-0001.bak",
                    "db.sqlite.pre-0002.bak",
                    "db.sqlite.pre-0003.bak",
                    "db.sqlite.pre-0004.bak",
                    "db.sqlite.pre-account-binding.bak",
                    "other.sqlite.pre-0001.bak",
                ],
                vec![
                    "db.sqlite.pre-0003.bak",
                    "db.sqlite.pre-0004.bak",
                    "db.sqlite.pre-0027.bak",
                    "db.sqlite.pre-account-binding.bak",
                    "other.sqlite.pre-0001.bak",
                ],
            ),
            // Newer backups from before a restore: the one just written stays.
            (
                vec![
                    "db.sqlite.pre-0030.bak",
                    "db.sqlite.pre-0031.bak",
                    "db.sqlite.pre-0032.bak",
                ],
                vec![
                    "db.sqlite.pre-0027.bak",
                    "db.sqlite.pre-0031.bak",
                    "db.sqlite.pre-0032.bak",
                ],
            ),
        ] {
            let dir = std::env::temp_dir().join(format!("ghostchat-backup-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("db.sqlite");
            database_migrated_to(&path, 26).await.close().await;
            for name in &existing {
                std::fs::write(dir.join(name), b"older").unwrap();
            }
            open_database(&path.to_string_lossy())
                .await
                .unwrap()
                .close()
                .await;
            assert_eq!(backup_files(&dir), expected);
        }
    }

    #[tokio::test]
    async fn an_upgrade_backup_that_cannot_be_written_stops_the_migration() {
        let dir = std::env::temp_dir().join(format!("ghostchat-backup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        database_migrated_to(&path, 24).await.close().await;
        // A non-empty directory where the backup's temporary file must go.
        std::fs::create_dir_all(dir.join("db.sqlite.pre-0025.bak.tmp/blocker")).unwrap();
        let error = open_database(&path.to_string_lossy()).await.unwrap_err();
        assert!(
            error.to_string().contains("backup"),
            "the error must name the backup: {error}"
        );
        let old = database_migrated_to(&path, 24).await;
        let level = sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&old)
            .await
            .unwrap();
        assert_eq!(level, 24, "nothing may be migrated without a backup");
        assert!(backup_files(&dir).is_empty());
    }

    async fn seed_user(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO users(id,username,display_name,recovery_lookup,recovery_verifier) VALUES(?,?,?,?,?)",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(format!("lookup-{id}"))
        .bind("verifier")
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_group(pool: &SqlitePool, id: &str, members: &[&str]) {
        sqlx::query("INSERT INTO conversations(id,kind,title) VALUES(?,'group','Seeded')")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        for (index, member) in members.iter().enumerate() {
            sqlx::query(
                "INSERT INTO conversation_members(conversation_id,user_id,role) VALUES(?,?,?)",
            )
            .bind(id)
            .bind(member)
            .bind(if index == 0 { "owner" } else { "member" })
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn deleted_account_crypto_events_stay_for_members_who_have_not_fetched_them() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let direct = app.direct(&carol, &bob).await;
        for conversation in [&group, &direct] {
            sqlx::query(
                "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
            )
            .bind(conversation)
            .bind(&carol.id)
            .execute(&app.state.db)
            .await
            .unwrap();
        }
        // Carol relays a commit to the group, a Welcome to Bob and a direct
        // event; Bob has fetched none of them when Carol deletes her account.
        let send = |conversation: String, event: &'static str, recipient: Option<String>| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{conversation}/crypto-events"),
                &carol,
            )
            .json(&serde_json::json!({
                "client_event_id": event,
                "recipient_user_id": recipient,
                "payload": "AAAA",
            }))
            .send()
        };
        for (conversation, event, recipient) in [
            (group.clone(), "commit-1", None),
            (group.clone(), "welcome-1", Some(bob.id.clone())),
            (direct.clone(), "direct-1", None),
        ] {
            let status = send(conversation, event, recipient).await.unwrap().status();
            assert_eq!(status, StatusCode::OK, "{event}");
        }
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);

        let events: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-events"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let events = events.as_array().unwrap();
        assert_eq!(
            events.len(),
            2,
            "the deleted account's commit and Welcome must still reach Bob"
        );
        assert!(events
            .iter()
            .all(|event| event["sender_id"] == carol.id.as_str()));
        // The direct conversation went with the account, and its events too.
        let left = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM crypto_events WHERE conversation_id=?",
        )
        .bind(&direct)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn migration_0029_keeps_crypto_events_and_their_sequence() {
        let dir = std::env::temp_dir().join(format!("ghostchat-migrate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let old = database_migrated_to(&path, 28).await;
        for user in ["u-a", "u-b", "u-c"] {
            seed_user(&old, user).await;
        }
        seed_group(&old, "g-1", &["u-a", "u-b", "u-c"]).await;
        seed_group(&old, "g-2", &["u-a", "u-b"]).await;
        for (id, conversation, sender, recipient) in [
            ("e-1", "g-1", "u-c", None),
            ("e-2", "g-1", "u-c", Some("u-b")),
            ("e-3", "g-1", "u-a", Some("u-c")),
            ("e-4", "g-2", "u-a", None),
            ("e-5", "g-1", "u-a", None),
        ] {
            sqlx::query(
                "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,recipient_id,payload,created_at) VALUES(?,?,?,?,?,?,'2026-01-02T03:04:05.000Z')",
            )
            .bind(id)
            .bind(conversation)
            .bind(sender)
            .bind(format!("client-{id}"))
            .bind(recipient)
            .bind(format!("payload-{id}"))
            .execute(&old)
            .await
            .unwrap();
        }
        // The newest event is gone before the upgrade: its sequence number
        // must still never be handed out again (clients keep a cursor).
        sqlx::query("DELETE FROM crypto_events WHERE id='e-5'")
            .execute(&old)
            .await
            .unwrap();
        type Row = (
            i64,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        );
        let dump = "SELECT sequence,id,conversation_id,sender_id,client_event_id,recipient_id,payload,created_at FROM crypto_events ORDER BY sequence";
        let before: Vec<Row> = sqlx::query_as(dump).fetch_all(&old).await.unwrap();
        assert_eq!(before.len(), 4);
        old.close().await;

        let db = open_database(&path.to_string_lossy()).await.unwrap();
        let after: Vec<Row> = sqlx::query_as(dump).fetch_all(&db).await.unwrap();
        assert_eq!(after, before);
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&db)
            .await
            .unwrap();
        assert!(violations.is_empty());
        let leftovers = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE '%0029%'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(leftovers, 0);
        let indexes = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='crypto_events' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(
            indexes,
            [
                "crypto_events_conversation_recipient_sequence_idx",
                "crypto_events_conversation_sequence_idx"
            ]
        );
        sqlx::query(
            "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,payload) VALUES('e-6','g-1','u-a','client-e-6','p')",
        )
        .execute(&db)
        .await
        .unwrap();
        let sequence =
            sqlx::query_scalar::<_, i64>("SELECT sequence FROM crypto_events WHERE id='e-6'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(sequence, 6, "a deleted event's sequence is not reused");
        let duplicate = sqlx::query(
            "INSERT INTO crypto_events(id,conversation_id,sender_id,client_event_id,payload) VALUES('e-7','g-1','u-a','client-e-6','p')",
        )
        .execute(&db)
        .await;
        assert!(duplicate.is_err(), "the idempotency key is still unique");

        // A deleted sender keeps its events; a deleted recipient's targeted
        // events and a deleted conversation's events still go.
        sqlx::query("DELETE FROM users WHERE id='u-c'")
            .execute(&db)
            .await
            .unwrap();
        let ids = |db: SqlitePool| async move {
            sqlx::query_scalar::<_, String>("SELECT id FROM crypto_events ORDER BY sequence")
                .fetch_all(&db)
                .await
                .unwrap()
        };
        assert_eq!(ids(db.clone()).await, ["e-1", "e-2", "e-4", "e-6"]);
        sqlx::query("DELETE FROM conversations WHERE id='g-1'")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(ids(db.clone()).await, ["e-4"]);
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn taking_over_a_deleted_accounts_setup_starts_a_new_setup_generation() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let status = app
            .request(
                Method::PATCH,
                &format!("/v1/conversations/{group}/members/{}", carol.id),
                &alice,
            )
            .json(&serde_json::json!({"role": "admin"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let lock = |user: &TestUser, mut roster: Vec<String>| {
            roster.sort();
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/crypto-lock"),
                user,
            )
            .json(&serde_json::json!({"member_ids": roster}))
            .send()
        };
        let welcome = |sender: &TestUser, recipient: &TestUser, event: &'static str| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/crypto-events"),
                sender,
            )
            .json(&serde_json::json!({
                "client_event_id": event,
                "recipient_user_id": recipient.id,
                "payload": "AAAA",
            }))
            .send()
        };
        let events = |viewer: &TestUser| {
            app.request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-events"),
                viewer,
            )
            .send()
        };
        // Admin Carol starts the setup and delivers a Welcome to Bob only,
        // then deletes her account before Alice gets hers.
        let started: serde_json::Value = lock(
            &carol,
            vec![alice.id.clone(), bob.id.clone(), carol.id.clone()],
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(started["setupGeneration"], 0);
        assert_eq!(started["commitSeq"], 0);
        assert_eq!(
            welcome(&carol, &bob, "welcome-0").await.unwrap().status(),
            StatusCode::OK
        );
        let listed: serde_json::Value = events(&bob).await.unwrap().json().await.unwrap();
        assert_eq!(listed[0]["setup_generation"], 0);
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let before: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-lock"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(before["setupGeneration"], 0);
        assert_eq!(before["deletedMemberIds"], serde_json::json!([carol.id]));

        // The owner's takeover is a reset: a new generation, and nothing of
        // the old setup (its Welcomes, its deleted-leaf record) is left for
        // a device to join or tolerate.
        let remaining = vec![alice.id.clone(), bob.id.clone()];
        let taken: serde_json::Value = lock(&alice, remaining.clone())
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(taken["lockedBy"], alice.id.as_str());
        assert_eq!(taken["setupComplete"], false);
        assert_eq!(taken["setupGeneration"], 1);
        // Nothing built on the old setup's group may be accepted any more.
        assert_eq!(taken["commitSeq"], 1);
        assert_eq!(taken["deletedMemberIds"], serde_json::json!([]));
        let listed: serde_json::Value = events(&bob).await.unwrap().json().await.unwrap();
        assert_eq!(
            listed,
            serde_json::json!([]),
            "the old setup's Welcome is gone"
        );
        // Resuming the taken-over setup is not another reset.
        let resumed: serde_json::Value = lock(&alice, remaining.clone())
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resumed["setupGeneration"], 1);
        assert_eq!(resumed["commitSeq"], 1);
        assert_eq!(
            welcome(&alice, &bob, "welcome-1").await.unwrap().status(),
            StatusCode::OK
        );
        let listed: serde_json::Value = events(&bob).await.unwrap().json().await.unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["setup_generation"], 1);
        assert_eq!(listed[0]["sender_id"], alice.id.as_str());
    }

    #[tokio::test]
    async fn migration_0030_keeps_crypto_locks_at_generation_zero() {
        let dir = std::env::temp_dir().join(format!("ghostchat-migrate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let old = database_migrated_to(&path, 29).await;
        for user in ["u-a", "u-b"] {
            seed_user(&old, user).await;
        }
        seed_group(&old, "g-1", &["u-a", "u-b"]).await;
        seed_group(&old, "g-2", &["u-a", "u-b"]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete,locked_at) VALUES('g-1','u-a',1,'2026-01-02T03:04:05.000Z'),('g-2','u-gone',0,'2026-02-02T03:04:05.000Z')",
        )
        .execute(&old)
        .await
        .unwrap();
        old.close().await;
        let db = open_database(&path.to_string_lossy()).await.unwrap();
        let rows: Vec<(String, String, i64, String, i64)> = sqlx::query_as(
            "SELECT conversation_id,locked_by,setup_complete,locked_at,setup_generation FROM conversation_crypto_locks ORDER BY conversation_id",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [
                (
                    "g-1".into(),
                    "u-a".into(),
                    1,
                    "2026-01-02T03:04:05.000Z".into(),
                    0
                ),
                (
                    "g-2".into(),
                    "u-gone".into(),
                    0,
                    "2026-02-02T03:04:05.000Z".into(),
                    0
                ),
            ]
        );
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn two_commits_built_on_one_group_state_are_not_both_accepted() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let dave = app.user("dave").await;
        let eve = app.user("eve").await;
        app.befriend(&alice, &eve).await;
        let group = app.group(&alice, &[&bob, &carol, &dave]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&group)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE conversation_members SET role='admin' WHERE conversation_id=? AND user_id=?",
        )
        .bind(&group)
        .bind(&bob.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let sorted = |users: &[&TestUser]| {
            let mut ids = users.iter().map(|user| user.id.clone()).collect::<Vec<_>>();
            ids.sort();
            ids
        };
        let commit_seq = |viewer: &TestUser| {
            let request = app.request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-lock"),
                viewer,
            );
            async move {
                let lock: serde_json::Value = request.send().await.unwrap().json().await.unwrap();
                lock["commitSeq"].as_i64()
            }
        };
        let update = |operation: &str, roster: &[String], seq: Option<i64>| {
            let mut body = serde_json::json!({
                "operation_id": operation,
                "expected_member_ids": roster,
                "commit_payload": "AAAA",
                "welcome_payload": "",
            });
            if let Some(seq) = seq {
                body["expected_commit_seq"] = seq.into();
            }
            body
        };
        let remove = |actor: &TestUser, target: &TestUser, body: serde_json::Value| {
            app.request(
                Method::POST,
                &format!(
                    "/v1/conversations/{group}/crypto-membership/remove/{}",
                    target.id
                ),
                actor,
            )
            .json(&body)
            .send()
        };
        let add = |actor: &TestUser, target: &TestUser, mut body: serde_json::Value| {
            body["welcome_payload"] = "AAAA".into();
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/members"),
                actor,
            )
            .json(&serde_json::json!({"member_ids": [target.id], "crypto_update": body}))
            .send()
        };
        let stale = |response: reqwest::Response| async move {
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], "stale_commit_seq", "{body}");
        };
        assert_eq!(commit_seq(&alice).await, Some(0));

        // Admin Bob removes the deleted account's leaf and, from the same
        // group state, owner Alice adds Eve. Members can apply only one of
        // the two commits, so the server takes only the first.
        let roster = sorted(&[&alice, &bob, &dave]);
        let bob_operation = Uuid::new_v4().to_string();
        let status = remove(&bob, &carol, update(&bob_operation, &roster, Some(0)))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(commit_seq(&alice).await, Some(1));
        let add_operation = Uuid::new_v4().to_string();
        stale(
            add(&alice, &eve, update(&add_operation, &roster, Some(0)))
                .await
                .unwrap(),
        )
        .await;
        assert!(!is_member(&app.state.db, &group, &eve.id).await.unwrap());
        let relayed = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM crypto_events WHERE conversation_id=? AND sender_id=?",
        )
        .bind(&group)
        .bind(&alice.id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
        assert_eq!(relayed, 0, "the refused commit is not relayed");
        assert_eq!(commit_seq(&alice).await, Some(1));
        // A retry of the commit the server already took still succeeds.
        let status = remove(&bob, &carol, update(&bob_operation, &roster, Some(0)))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(commit_seq(&alice).await, Some(1));
        // A commit that does not say what it was built on is refused too.
        stale(
            add(
                &alice,
                &eve,
                update(&Uuid::new_v4().to_string(), &roster, None),
            )
            .await
            .unwrap(),
        )
        .await;

        // Built on the current state, each kind of commit is taken once.
        let status = add(&alice, &eve, update(&add_operation, &roster, Some(1)))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(commit_seq(&alice).await, Some(2));
        let roster = sorted(&[&alice, &bob, &dave, &eve]);
        stale(
            remove(
                &alice,
                &dave,
                update(&Uuid::new_v4().to_string(), &roster, Some(1)),
            )
            .await
            .unwrap(),
        )
        .await;
        let status = remove(
            &alice,
            &dave,
            update(&Uuid::new_v4().to_string(), &roster, Some(2)),
        )
        .await
        .unwrap()
        .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(commit_seq(&alice).await, Some(3));

        // A leave commit is fenced the same way.
        let roster = sorted(&[&alice, &bob, &eve]);
        let leave_operation = Uuid::new_v4().to_string();
        let status = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{group}/crypto-membership/leave"),
                &eve,
            )
            .json(&serde_json::json!({
                "operation_id": leave_operation,
                "expected_member_ids": roster,
                "proposal_payload": "AAAA",
            }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let leave_commit = |seq: i64| {
            app.request(
                Method::POST,
                &format!(
                    "/v1/conversations/{group}/crypto-membership/leave/commit/{}",
                    eve.id
                ),
                &alice,
            )
            .json(&update(&leave_operation, &roster, Some(seq)))
            .send()
        };
        stale(leave_commit(2).await.unwrap()).await;
        assert!(is_member(&app.state.db, &group, &eve.id).await.unwrap());
        assert_eq!(
            leave_commit(3).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(commit_seq(&alice).await, Some(4));
        // Listed relay events say which commit the conversation is at, read
        // in the same statement as the events.
        let listed: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-events"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!listed.as_array().unwrap().is_empty());
        assert!(listed
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["commit_seq"] == 4));
    }

    #[tokio::test]
    async fn migration_0031_keeps_crypto_locks_at_commit_zero() {
        let dir = std::env::temp_dir().join(format!("ghostchat-migrate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let old = database_migrated_to(&path, 30).await;
        for user in ["u-a", "u-b"] {
            seed_user(&old, user).await;
        }
        seed_group(&old, "g-1", &["u-a", "u-b"]).await;
        seed_group(&old, "g-2", &["u-a", "u-b"]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete,locked_at,setup_generation) VALUES('g-1','u-a',1,'2026-01-02T03:04:05.000Z',0),('g-2','u-b',0,'2026-02-02T03:04:05.000Z',2)",
        )
        .execute(&old)
        .await
        .unwrap();
        old.close().await;
        let db = open_database(&path.to_string_lossy()).await.unwrap();
        let rows: Vec<(String, String, i64, String, i64, i64)> = sqlx::query_as(
            "SELECT conversation_id,locked_by,setup_complete,locked_at,setup_generation,commit_seq FROM conversation_crypto_locks ORDER BY conversation_id",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [
                (
                    "g-1".into(),
                    "u-a".into(),
                    1,
                    "2026-01-02T03:04:05.000Z".into(),
                    0,
                    0
                ),
                (
                    "g-2".into(),
                    "u-b".into(),
                    0,
                    "2026-02-02T03:04:05.000Z".into(),
                    2,
                    0
                ),
            ]
        );
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn crypto_lock_reports_the_server_clock_next_to_deleted_at() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&group)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        // The record is backdated, so the grace period a client derives is
        // the difference of two server timestamps, whatever its own clock.
        sqlx::query(
            "UPDATE crypto_group_deleted_members SET deleted_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-90 seconds') WHERE conversation_id=?",
        )
        .bind(&group)
        .execute(&app.state.db)
        .await
        .unwrap();
        let lock: serde_json::Value = app
            .request(
                Method::GET,
                &format!("/v1/conversations/{group}/crypto-lock"),
                &bob,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let parse = |value: &serde_json::Value| {
            let text = value.as_str().expect("an RFC 3339 timestamp");
            assert_eq!(text.len(), 24, "{text}");
            assert!(text.ends_with('Z'), "{text}");
            let (date, time) = text.trim_end_matches('Z').split_once('T').unwrap();
            let day: Vec<i64> = date.split('-').map(|part| part.parse().unwrap()).collect();
            let clock: Vec<f64> = time.split(':').map(|part| part.parse().unwrap()).collect();
            (day, clock[0] * 3600.0 + clock[1] * 60.0 + clock[2])
        };
        let (now_day, now_seconds) = parse(&lock["serverNow"]);
        let (deleted_day, deleted_seconds) = parse(&lock["deletedMembers"][0]["deletedAt"]);
        let elapsed =
            now_seconds - deleted_seconds + if now_day == deleted_day { 0.0 } else { 86400.0 };
        assert!(
            (89.0..120.0).contains(&elapsed),
            "serverNow - deletedAt was {elapsed}s"
        );
    }

    #[tokio::test]
    async fn an_encrypted_message_can_carry_several_encrypted_attachments() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&direct)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let upload = || {
            let attachment_id = Uuid::new_v4().to_string();
            let request = app
                .request(
                    Method::POST,
                    &format!(
                        "/v1/conversations/{direct}/encrypted-attachments?attachment_id={attachment_id}&plain_size=0&chunk_size={ENCRYPTED_ATTACHMENT_CHUNK_BYTES}&chunk_count=1"
                    ),
                    &alice,
                )
                .body(vec![7u8; 16])
                .send();
            async move {
                assert_eq!(request.await.unwrap().status(), StatusCode::OK);
                attachment_id
            }
        };
        let send = |attachment_ids: Vec<String>| {
            app.request(
                Method::POST,
                &format!("/v1/conversations/{direct}/messages"),
                &alice,
            )
            .json(&serde_json::json!({
                "client_message_id": Uuid::new_v4().to_string(),
                "content": "e2e1:AAAA",
                "attachment_ids": attachment_ids,
            }))
            .send()
        };
        let blob = |id: &str| app.state.attachment_dir.join(format!("{id}.blob"));
        // The route allows up to eight attachments on one message (the
        // upload rate limit is ten a minute, so nine are uploaded here).
        let mut attachments = Vec::new();
        for _ in 0..8 {
            attachments.push(upload().await);
        }
        let other = upload().await;
        // A ninth is over the limit, and nothing of that request is linked.
        let mut too_many = attachments.clone();
        too_many.push(other.clone());
        assert_eq!(
            send(too_many).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        let linked = || async {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM attachments WHERE message_id IS NOT NULL",
            )
            .fetch_one(&app.state.db)
            .await
            .unwrap()
        };
        assert_eq!(linked().await, 0);
        let response = send(attachments.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let message: serde_json::Value = response.json().await.unwrap();
        assert_eq!(linked().await, 8);
        // Deleting the message deletes all of its blobs and only those.
        let status = app
            .request(
                Method::DELETE,
                &format!("/v1/messages/{}", message["id"].as_str().unwrap()),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        for attachment in &attachments {
            assert!(!blob(attachment).exists());
        }
        assert!(blob(&other).exists());
    }

    #[tokio::test]
    async fn migration_0032_keeps_attachment_links_and_allows_several_per_message() {
        let dir = std::env::temp_dir().join(format!("ghostchat-migrate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let old = database_migrated_to(&path, 31).await;
        for user in ["u-a", "u-b"] {
            seed_user(&old, user).await;
        }
        seed_group(&old, "g-1", &["u-a", "u-b"]).await;
        for (id, message) in [("a-1", Some("m-1")), ("a-2", None), ("a-3", Some("m-2"))] {
            sqlx::query(
                "INSERT INTO attachments(id,conversation_id,uploaded_by,filename,content_type,size_bytes,file_data,stored_on_disk,encrypted,message_id,created_at) VALUES(?,'g-1','u-a','f','application/octet-stream',3,x'010203',1,1,?,'2026-01-02T03:04:05.000Z')",
            )
            .bind(id)
            .bind(message)
            .execute(&old)
            .await
            .unwrap();
        }
        type Row = (
            String,
            String,
            String,
            i64,
            Vec<u8>,
            i64,
            i64,
            Option<String>,
            String,
        );
        let dump = "SELECT id,conversation_id,uploaded_by,size_bytes,file_data,stored_on_disk,encrypted,message_id,created_at FROM attachments ORDER BY id";
        let before: Vec<Row> = sqlx::query_as(dump).fetch_all(&old).await.unwrap();
        assert_eq!(before.len(), 3);
        old.close().await;

        let db = open_database(&path.to_string_lossy()).await.unwrap();
        let after: Vec<Row> = sqlx::query_as(dump).fetch_all(&db).await.unwrap();
        assert_eq!(after, before);
        // A second attachment can now be linked to the same message, and the
        // lookup by message still has its index.
        sqlx::query("UPDATE attachments SET message_id='m-1' WHERE id='a-2'")
            .execute(&db)
            .await
            .unwrap();
        let (unique, partial): (i64, i64) = sqlx::query_as(
            "SELECT \"unique\",partial FROM pragma_index_list('attachments') WHERE name='attachments_by_message'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!((unique, partial), (0, 1));
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn group_members_report_their_join_order() {
        use reqwest::Method;
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let (first, second) = if bob.id > carol.id {
            (&bob, &carol)
        } else {
            (&carol, &bob)
        };
        app.befriend(&alice, &bob).await;
        app.befriend(&alice, &carol).await;
        let group: serde_json::Value = app
            .request(Method::POST, "/v1/conversations/groups", &alice)
            .json(&serde_json::json!({"name": "Order", "member_ids": [first.id, second.id]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let members: serde_json::Value = app
            .request(
                Method::GET,
                &format!(
                    "/v1/conversations/{}/members",
                    group["id"].as_str().unwrap()
                ),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let seq = |id: &str| {
            members
                .as_array()
                .unwrap()
                .iter()
                .find(|member| member["id"] == id)
                .unwrap()["joined_seq"]
                .as_i64()
                .unwrap()
        };
        assert!(seq(&alice.id) < seq(&first.id) && seq(&first.id) < seq(&second.id));
    }
}
