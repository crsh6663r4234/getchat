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
mod account_data;
mod admission;
mod api_error;
mod app_state;
mod auth;
mod conversations;
mod crypto_devices;
mod crypto_lock;
mod database;
pub mod device_proof;
mod group_invites;
mod group_membership;
mod media_relay;
mod media_revocation;
mod messages;
pub mod ratelimit;
pub mod self_update;
mod social;
mod startup;
mod users;
mod voice;

use account_data::*;
use admission::*;
use api_error::*;
use app_state::*;
pub use app_state::{build_state, build_state_with_attachment_dir, AppState};
use auth::*;
use conversations::*;
use crypto_devices::*;
use crypto_lock::*;
use database::*;
pub use database::{is_pre_upgrade_backup_error, open_database, PreUpgradeBackupError};
use group_invites::*;
use group_membership::*;
use messages::*;
use social::*;
pub use startup::{bind_default, run_standalone, serve};
use users::*;
use voice::*;

const MAX_ATTACHMENT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const ENCRYPTED_ATTACHMENT_CHUNK_BYTES: u64 = 1024 * 1024;
const ATTACHMENT_DOWNLOAD_TTL_SECONDS: u64 = 30 * 60;
const ATTACHMENT_MARKER: &str = "file:";

/// The version this server binary was built as; `/health` reports it so
/// clients can tell an older server apart and the self-updater can compare.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

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
            get(|| async {
                Json(serde_json::json!({
                    "status": "ok",
                    "service": "ghostchat",
                    "version": SERVER_VERSION,
                }))
            }),
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
        .route("/v1/media/ws", get(media_relay::websocket))
        .fallback(|| async { ApiError::not_found() })
        .layer(DefaultBodyLimit::max(96 * 1024))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            distributed_admission,
        ))
        .with_state(state)
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

/// The refusal an older client gets when it writes plaintext planner data to
/// an encrypted group: the server never stores it.
const PLANNER_ENCRYPTION_REQUIRED: &str = "planner_encryption_required";
/// Sent when ciphertext is posted to a group whose chat is not encrypted.
const PLANNER_NOT_ENCRYPTED: &str = "planner_not_encrypted";
/// Largest decoded ciphertext of a poll or event (its plaintext is at most
/// 8 KB on the client) and of a vote or RSVP.
const PLANNER_ITEM_CIPHERTEXT_LIMIT: usize = 8 * 1024 + 64;
const PLANNER_RESPONSE_CIPHERTEXT_LIMIT: usize = 1024;
/// "GP" 0x01, a 96-bit nonce and a 128-bit tag.
const PLANNER_CIPHERTEXT_MINIMUM: usize = 3 + 12 + 16;

/// The sealed form of a planner item or response of an encrypted group. The
/// server checks its shape and size only; it cannot read it.
#[derive(Deserialize, Default)]
struct EncryptedPlannerFields {
    #[serde(default)]
    ciphertext: Option<String>,
    #[serde(default)]
    epoch: Option<i64>,
    #[serde(default)]
    key_id: Option<String>,
}

struct EncryptedPlannerPayload {
    ciphertext: Vec<u8>,
    epoch: i64,
    key_id: String,
}

impl EncryptedPlannerFields {
    fn is_empty(&self) -> bool {
        self.ciphertext.is_none() && self.epoch.is_none() && self.key_id.is_none()
    }

    fn validated(&self, limit: usize) -> Result<Option<EncryptedPlannerPayload>, ApiError> {
        if self.is_empty() {
            return Ok(None);
        }
        let invalid = || ApiError::bad("The encrypted planner data is not valid.");
        let (Some(ciphertext), Some(epoch), Some(key_id)) = (
            self.ciphertext.as_deref(),
            self.epoch,
            self.key_id.as_deref(),
        ) else {
            return Err(invalid());
        };
        if ciphertext.len() > limit.div_ceil(3) * 4 + 4
            || epoch < 0
            || key_id.len() != 32
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid());
        }
        let decoded = URL_SAFE_NO_PAD
            .decode(ciphertext.as_bytes())
            .map_err(|_| invalid())?;
        if decoded.len() < PLANNER_CIPHERTEXT_MINIMUM
            || decoded.len() > limit
            || !decoded.starts_with(b"GP\x01")
        {
            return Err(invalid());
        }
        Ok(Some(EncryptedPlannerPayload {
            ciphertext: decoded,
            epoch,
            key_id: key_id.to_string(),
        }))
    }
}

fn planner_plaintext_refused() -> ApiError {
    ApiError::conflict(
        "This group's planner is end-to-end encrypted. Update GhostChat to add or answer polls and events here.",
    )
    .with_code(PLANNER_ENCRYPTION_REQUIRED)
}

fn planner_ciphertext_refused() -> ApiError {
    ApiError::bad("This group's planner is not encrypted.").with_code(PLANNER_NOT_ENCRYPTED)
}

async fn group_planner_is_encrypted(
    db: &SqlitePool,
    conversation_id: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM conversation_crypto_locks WHERE conversation_id=?)",
    )
    .bind(conversation_id)
    .fetch_one(db)
    .await
    .map_err(|_| ApiError::internal())
}

fn serialize_planner_ciphertext<S: serde::Serializer>(
    value: &Option<Vec<u8>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(bytes) => serializer.serialize_some(&URL_SAFE_NO_PAD.encode(bytes)),
        None => serializer.serialize_none(),
    }
}

fn serialize_planner_response_ciphertext<S: serde::Serializer>(
    value: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&URL_SAFE_NO_PAD.encode(value))
}

/// A vote or RSVP of an encrypted group: the server knows who answered and
/// when, not what.
#[derive(Serialize, FromRow)]
struct PlannerResponseRow {
    user_id: String,
    #[serde(
        rename = "ciphertext",
        serialize_with = "serialize_planner_response_ciphertext"
    )]
    payload_ciphertext: Vec<u8>,
    #[serde(rename = "epoch")]
    payload_epoch: i64,
    #[serde(rename = "key_id")]
    payload_key_id: String,
}

fn planner_item_id(id: Option<&str>) -> Result<String, ApiError> {
    id.and_then(|id| Uuid::parse_str(id).ok())
        .map(|id| id.to_string())
        .ok_or_else(|| ApiError::bad("An encrypted planner item needs a valid id."))
}

#[derive(Deserialize)]
struct CreateGroupPollRequest {
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    options: Option<Vec<String>>,
    closes_at: Option<String>,
    /// Chosen by the client for an encrypted poll: its ciphertext is bound
    /// to it.
    #[serde(default)]
    id: Option<String>,
    #[serde(flatten)]
    encrypted: EncryptedPlannerFields,
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
    /// `None` for an encrypted poll: question and options are in its
    /// ciphertext.
    question: Option<String>,
    closes_at: Option<String>,
    closed_at: Option<String>,
    created_at: String,
    #[serde(rename = "ciphertext", serialize_with = "serialize_planner_ciphertext")]
    payload_ciphertext: Option<Vec<u8>>,
    #[serde(rename = "epoch")]
    payload_epoch: Option<i64>,
    #[serde(rename = "key_id")]
    payload_key_id: Option<String>,
}

#[derive(Serialize)]
struct GroupPollDetail {
    #[serde(flatten)]
    poll: GroupPollRow,
    options: Vec<GroupPollOption>,
    my_option_id: Option<String>,
    /// Encrypted votes (of an encrypted poll, or of a poll stored before its
    /// group was encrypted), tallied by the clients.
    responses: Vec<PlannerResponseRow>,
}

async fn group_poll_details(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<Vec<GroupPollDetail>, ApiError> {
    let polls = sqlx::query_as::<_, GroupPollRow>(
        "SELECT id,conversation_id,creator_id,question,closes_at,closed_at,created_at,payload_ciphertext,payload_epoch,payload_key_id FROM group_polls WHERE conversation_id=? ORDER BY created_at DESC LIMIT 100",
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
        let my_option_id = sqlx::query_scalar::<_, Option<String>>(
            "SELECT option_id FROM group_poll_votes WHERE poll_id=? AND user_id=?",
        )
        .bind(&poll.id)
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(|_| ApiError::internal())?
        .flatten();
        let responses = sqlx::query_as::<_, PlannerResponseRow>(
            "SELECT user_id,payload_ciphertext,payload_epoch,payload_key_id FROM group_poll_votes WHERE poll_id=? AND payload_ciphertext IS NOT NULL ORDER BY created_at,user_id",
        )
        .bind(&poll.id)
        .fetch_all(db)
        .await
        .map_err(|_| ApiError::internal())?;
        details.push(GroupPollDetail {
            poll,
            options,
            my_option_id,
            responses,
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
    let encrypted_group = group_planner_is_encrypted(&state.db, &conversation_id).await?;
    let sealed = body.encrypted.validated(PLANNER_ITEM_CIPHERTEXT_LIMIT)?;
    match (encrypted_group, sealed.is_some()) {
        (true, false) => return Err(planner_plaintext_refused()),
        (false, true) => return Err(planner_ciphertext_refused()),
        _ => {}
    }
    if sealed.is_some() && (body.question.is_some() || body.options.is_some()) {
        return Err(ApiError::bad(
            "An encrypted poll must not carry a plaintext question or options.",
        ));
    }
    let question = body.question.as_deref().unwrap_or_default().trim();
    let options: Vec<_> = body
        .options
        .iter()
        .flatten()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    if sealed.is_none()
        && (question.is_empty()
            || question.len() > 240
            || !(2..=8).contains(&options.len())
            || options.iter().any(|item| item.len() > 100))
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
    if let Some(sealed) = sealed {
        let poll_id = planner_item_id(body.id.as_deref())?;
        let inserted = sqlx::query("INSERT OR IGNORE INTO group_polls(id,conversation_id,creator_id,question,closes_at,payload_ciphertext,payload_epoch,payload_key_id) VALUES(?,?,?,NULL,?,?,?,?)")
            .bind(&poll_id).bind(&conversation_id).bind(&user.id).bind(body.closes_at.as_deref())
            .bind(&sealed.ciphertext).bind(sealed.epoch).bind(&sealed.key_id)
            .execute(&state.db).await.map_err(|_| ApiError::internal())?
            .rows_affected();
        if inserted == 0 {
            return Err(ApiError::conflict(
                "This planner item id is already in use.",
            ));
        }
        let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
        return Ok(Json(
            group_poll_details(&state.db, &conversation_id, &user.id).await?,
        ));
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
    #[serde(default)]
    option_id: Option<String>,
    #[serde(flatten)]
    encrypted: EncryptedPlannerFields,
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
    let encrypted_group = group_planner_is_encrypted(&state.db, &conversation_id).await?;
    let sealed = body
        .encrypted
        .validated(PLANNER_RESPONSE_CIPHERTEXT_LIMIT)?;
    match (encrypted_group, sealed.is_some()) {
        (true, false) => return Err(planner_plaintext_refused()),
        (false, true) => return Err(planner_ciphertext_refused()),
        _ => {}
    }
    if expired {
        return Err(ApiError::conflict("This poll is closed."));
    }
    if let Some(sealed) = sealed {
        if body.option_id.is_some() {
            return Err(ApiError::bad(
                "An encrypted vote must not carry a plaintext option.",
            ));
        }
        // A vote on a poll stored before the group was encrypted replaces
        // that voter's plaintext vote, if any, with an encrypted one.
        sqlx::query("INSERT INTO group_poll_votes(poll_id,user_id,option_id,payload_ciphertext,payload_epoch,payload_key_id) VALUES(?,?,NULL,?,?,?) ON CONFLICT(poll_id,user_id) DO UPDATE SET option_id=NULL,payload_ciphertext=excluded.payload_ciphertext,payload_epoch=excluded.payload_epoch,payload_key_id=excluded.payload_key_id,created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
            .bind(&poll_id).bind(&user.id).bind(&sealed.ciphertext).bind(sealed.epoch).bind(&sealed.key_id)
            .execute(&state.db).await.map_err(|_| ApiError::internal())?;
        let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
        return Ok(StatusCode::NO_CONTENT);
    }
    let Some(option_id) = body.option_id.as_deref() else {
        return Err(ApiError::bad("Choose an option from this poll."));
    };
    let valid = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM group_poll_options WHERE id=? AND poll_id=?",
    )
    .bind(option_id)
    .bind(&poll_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if valid == 0 {
        return Err(ApiError::bad("Choose an option from this poll."));
    }
    sqlx::query("INSERT INTO group_poll_votes(poll_id,user_id,option_id) VALUES(?,?,?) ON CONFLICT(poll_id,user_id) DO UPDATE SET option_id=excluded.option_id,payload_ciphertext=NULL,payload_epoch=NULL,payload_key_id=NULL,created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .bind(&poll_id).bind(&user.id).bind(option_id).execute(&state.db).await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CreateGroupEventRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    details: Option<String>,
    #[serde(default)]
    scheduled_at: Option<String>,
    /// Chosen by the client for an encrypted event: its ciphertext is bound
    /// to it.
    #[serde(default)]
    id: Option<String>,
    #[serde(flatten)]
    encrypted: EncryptedPlannerFields,
}

#[derive(Deserialize)]
struct GroupEventRsvpRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(flatten)]
    encrypted: EncryptedPlannerFields,
}

#[derive(Serialize, FromRow)]
struct GroupEventRow {
    id: String,
    conversation_id: String,
    /// `None` once the creator deleted their account.
    creator_id: Option<String>,
    /// `None` for an encrypted event, like `details` and `scheduled_at`:
    /// they are in its ciphertext.
    title: Option<String>,
    details: Option<String>,
    scheduled_at: Option<String>,
    cancelled_at: Option<String>,
    created_at: String,
    going: i64,
    maybe: i64,
    declined: i64,
    my_status: Option<String>,
    #[serde(rename = "ciphertext", serialize_with = "serialize_planner_ciphertext")]
    payload_ciphertext: Option<Vec<u8>>,
    #[serde(rename = "epoch")]
    payload_epoch: Option<i64>,
    #[serde(rename = "key_id")]
    payload_key_id: Option<String>,
}

#[derive(Serialize)]
struct GroupEventDetail {
    #[serde(flatten)]
    event: GroupEventRow,
    /// Encrypted RSVPs, counted by the clients (the counts above are the
    /// plaintext RSVPs stored before the group was encrypted).
    responses: Vec<PlannerResponseRow>,
}

async fn group_event_details(
    db: &SqlitePool,
    conversation_id: &str,
    user_id: &str,
) -> Result<Vec<GroupEventDetail>, ApiError> {
    // An encrypted event's time is in its ciphertext; it is ordered by its
    // creation time here and by its time on the clients.
    let events = sqlx::query_as::<_, GroupEventRow>(
        "SELECT e.id,e.conversation_id,e.creator_id,e.title,e.details,e.scheduled_at,e.cancelled_at,e.created_at,(SELECT count(*) FROM group_event_rsvps r WHERE r.event_id=e.id AND r.status='going') AS going,(SELECT count(*) FROM group_event_rsvps r WHERE r.event_id=e.id AND r.status='maybe') AS maybe,(SELECT count(*) FROM group_event_rsvps r WHERE r.event_id=e.id AND r.status='declined') AS declined,(SELECT status FROM group_event_rsvps r WHERE r.event_id=e.id AND r.user_id=?) AS my_status,e.payload_ciphertext,e.payload_epoch,e.payload_key_id FROM group_events e WHERE e.conversation_id=? ORDER BY COALESCE(e.scheduled_at,e.created_at) ASC LIMIT 100",
    ).bind(user_id).bind(conversation_id).fetch_all(db).await.map_err(|_| ApiError::internal())?;
    let mut details = Vec::with_capacity(events.len());
    for event in events {
        let responses = sqlx::query_as::<_, PlannerResponseRow>(
            "SELECT user_id,payload_ciphertext,payload_epoch,payload_key_id FROM group_event_rsvps WHERE event_id=? AND payload_ciphertext IS NOT NULL ORDER BY updated_at,user_id",
        )
        .bind(&event.id)
        .fetch_all(db)
        .await
        .map_err(|_| ApiError::internal())?;
        details.push(GroupEventDetail { event, responses });
    }
    Ok(details)
}

async fn list_group_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Json<Vec<GroupEventDetail>>, ApiError> {
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
) -> Result<Json<Vec<GroupEventDetail>>, ApiError> {
    let user = authenticate(&headers, &state).await?;
    ensure_group_planner_member(&state.db, &conversation_id, &user.id).await?;
    let encrypted_group = group_planner_is_encrypted(&state.db, &conversation_id).await?;
    let sealed = body.encrypted.validated(PLANNER_ITEM_CIPHERTEXT_LIMIT)?;
    match (encrypted_group, sealed.is_some()) {
        (true, false) => return Err(planner_plaintext_refused()),
        (false, true) => return Err(planner_ciphertext_refused()),
        _ => {}
    }
    if let Some(sealed) = sealed {
        if body.title.is_some() || body.details.is_some() || body.scheduled_at.is_some() {
            return Err(ApiError::bad(
                "An encrypted event must not carry a plaintext title, notes or time.",
            ));
        }
        let event_id = planner_item_id(body.id.as_deref())?;
        let inserted = sqlx::query("INSERT OR IGNORE INTO group_events(id,conversation_id,creator_id,title,details,scheduled_at,payload_ciphertext,payload_epoch,payload_key_id) VALUES(?,?,?,NULL,NULL,NULL,?,?,?)")
            .bind(&event_id).bind(&conversation_id).bind(&user.id)
            .bind(&sealed.ciphertext).bind(sealed.epoch).bind(&sealed.key_id)
            .execute(&state.db).await.map_err(|_| ApiError::internal())?
            .rows_affected();
        if inserted == 0 {
            return Err(ApiError::conflict(
                "This planner item id is already in use.",
            ));
        }
        let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
        return Ok(Json(
            group_event_details(&state.db, &conversation_id, &user.id).await?,
        ));
    }
    let title = body.title.as_deref().unwrap_or_default().trim();
    let details = body.details.as_deref().unwrap_or_default().trim();
    let scheduled_at = body.scheduled_at.as_deref().unwrap_or_default().trim();
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
    let sealed = body
        .encrypted
        .validated(PLANNER_RESPONSE_CIPHERTEXT_LIMIT)?;
    if sealed.is_some() && body.status.is_some() {
        return Err(ApiError::bad(
            "An encrypted RSVP must not carry a plaintext answer.",
        ));
    }
    if sealed.is_none() && !matches!(body.status.as_deref(), Some("going" | "maybe" | "declined")) {
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
    let encrypted_group = group_planner_is_encrypted(&state.db, &conversation_id).await?;
    match (encrypted_group, sealed.is_some()) {
        (true, false) => return Err(planner_plaintext_refused()),
        (false, true) => return Err(planner_ciphertext_refused()),
        _ => {}
    }
    if let Some(sealed) = sealed {
        sqlx::query("INSERT INTO group_event_rsvps(event_id,user_id,status,payload_ciphertext,payload_epoch,payload_key_id) VALUES(?,?,NULL,?,?,?) ON CONFLICT(event_id,user_id) DO UPDATE SET status=NULL,payload_ciphertext=excluded.payload_ciphertext,payload_epoch=excluded.payload_epoch,payload_key_id=excluded.payload_key_id,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
            .bind(&event_id).bind(&user.id).bind(&sealed.ciphertext).bind(sealed.epoch).bind(&sealed.key_id)
            .execute(&state.db).await.map_err(|_| ApiError::internal())?;
        let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
        return Ok(StatusCode::NO_CONTENT);
    }
    sqlx::query("INSERT INTO group_event_rsvps(event_id,user_id,status) VALUES(?,?,?) ON CONFLICT(event_id,user_id) DO UPDATE SET status=excluded.status,payload_ciphertext=NULL,payload_epoch=NULL,payload_key_id=NULL,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .bind(&event_id).bind(&user.id).bind(body.status.as_deref()).execute(&state.db).await.map_err(|_| ApiError::internal())?;
    let _ = state.events.send(ServerEvent { recipients: group_recipients(&state.db, &conversation_id).await?, event: serde_json::json!({"type":"group_planner_updated","conversation_id":conversation_id}) });
    Ok(StatusCode::NO_CONTENT)
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

/// Deletes `<uuid>.upload` temp files in the attachment directory whose last
/// write is more than a day before `now` (Unix seconds). Both upload routes
/// write their temp file chunk by chunk and remove it on every error path, so
/// such a file is only left when the process stopped mid-upload. Nothing
/// tracks running uploads, so the modification time stands in: a running
/// upload rewrites it with every chunk, and one that has not received a byte
/// for a day is abandoned. Even then the open handle keeps writing to the
/// unlinked file on Unix and the final rename fails, so that upload ends with
/// an error and no blob, never with a truncated one.
async fn sweep_stale_upload_temp_files(dir: &std::path::Path, now: i64) -> std::io::Result<u64> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let cutoff = UNIX_EPOCH + Duration::from_secs((now - UNCLAIMED_UPLOAD_TTL_SECS).max(0) as u64);
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".upload")) else {
            continue;
        };
        // Exactly the name the upload routes give: a UUID in its usual form.
        if Uuid::parse_str(stem).map(|id| id.to_string()).as_deref() != Ok(stem) {
            continue;
        }
        let Ok(metadata) = tokio::fs::symlink_metadata(entry.path()).await else {
            continue;
        };
        if !metadata.is_file() || !metadata.modified().is_ok_and(|time| time < cutoff) {
            continue;
        }
        if tokio::fs::remove_file(entry.path()).await.is_ok() {
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
        if sweep_stale_upload_temp_files(&state.attachment_dir, now)
            .await
            .is_err()
        {
            tracing::warn!("Sweeping stale upload temp files failed; retrying later");
        }
        tokio::time::sleep(UNCLAIMED_UPLOAD_SWEEP_EVERY).await;
    }
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
        // The side that falls back to the server media relay tells the
        // other side to switch too, so both ends use the same path.
        "media_path" => signal.payload == serde_json::json!({ "path": "server" }),
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
    let mut keepalive = tokio::time::interval(state.keepalive.interval);
    keepalive.tick().await;
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if last_heard.elapsed() >= state.keepalive.timeout
                    || socket.send(WsMessage::Ping(Vec::new())).await.is_err()
                {
                    break;
                }
            }
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
                Some(Ok(WsMessage::Pong(_))) => last_heard = Instant::now(),
                Some(Ok(WsMessage::Ping(bytes))) => {
                    last_heard = Instant::now();
                    if socket.send(WsMessage::Pong(bytes)).await.is_err() { break; }
                },
                Some(Ok(WsMessage::Text(text))) => {
                    last_heard = Instant::now();
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

#[cfg(test)]
mod tests {
    use super::startup::*;
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
            Self::build(require_redis, Keepalive::default()).await
        }

        /// A server whose WebSocket keepalive uses the given (short) timings.
        async fn with_keepalive(keepalive: Keepalive) -> Self {
            Self::build(false, keepalive).await
        }

        async fn build(require_redis: bool, keepalive: Keepalive) -> Self {
            let dir = std::env::temp_dir().join(format!("ghostchat-route-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let db = open_database(&dir.join("db.sqlite").to_string_lossy())
                .await
                .unwrap();
            let mut state = build_state_with_attachment_dir(db, dir.join("attachments")).await;
            // Do not inherit the developer's shell: no Redis admission, no
            // TURN and no LiveKit, whatever GHOSTCHAT_* variables are set.
            state.require_redis = require_redis;
            state.keepalive = keepalive;
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
    async fn health_reports_the_server_version() {
        let app = TestApp::new().await;
        let health: serde_json::Value = app
            .client
            .get(format!("{}/health", app.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["status"], "ok");
        assert_eq!(health["service"], "ghostchat");
        assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn standalone_server_releases_its_port_for_a_self_update_and_listens_again() {
        let app = TestApp::new().await;
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let (control, mut signals) = self_update::ServeControl::new();
        let state = app.state.clone();
        let serving =
            tokio::spawn(async move { serve_standalone(addr, state, &mut signals).await });
        let health = format!("http://{addr}/health");
        let answers = |client: reqwest::Client, url: String| async move {
            client
                .get(url)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
        };
        control.listening().await;
        assert!(answers(app.client.clone(), health.clone()).await);
        control.stop().await;
        // The port is free for the new exe.
        drop(tokio::net::TcpListener::bind(addr).await.unwrap());
        assert!(!answers(app.client.clone(), health.clone()).await);
        control.resume().await;
        assert!(answers(app.client.clone(), health.clone()).await);
        serving.abort();
    }

    /// The standalone exe goes through `run_standalone`, not `serve`; it must
    /// run the same background tasks (upload sweeps, media revocation
    /// reconciler), seen here by the sweep removing a stale temp file.
    #[tokio::test]
    async fn standalone_server_runs_the_background_sweeps() {
        let dir = std::env::temp_dir().join(format!("ghostchat-standalone-{}", Uuid::new_v4()));
        let attachments = dir.join("ghostchat-attachments");
        std::fs::create_dir_all(&attachments).unwrap();
        let stale = attachments.join(format!("{}.upload", Uuid::new_v4()));
        std::fs::write(&stale, b"partial").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(25 * 3600))
            .unwrap();
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let database = dir.join("ghostchat.sqlite").to_string_lossy().into_owned();
        let serving = tokio::spawn(async move { run_standalone(addr, &database, None).await });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while stale.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let swept = !stale.exists();
        serving.abort();
        let _ = serving.await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(swept, "the standalone server never ran the upload sweep");
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
        let refused = send(&bob, &direct, "imposter").await.unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "client_message_id_in_use", "{body}");
        assert_eq!(
            body["error"],
            "This message ID is already used in this conversation."
        );
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
    async fn the_sweep_deletes_only_stale_upload_temp_files() {
        let app = TestApp::new().await;
        let dir = app.state.attachment_dir.clone();
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, idle_hours: u64| {
            let path = dir.join(name);
            std::fs::write(&path, b"partial").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(idle_hours * 3600))
                .unwrap();
            path
        };
        // Left behind by an upload interrupted before its cleanup ran.
        let stale = file(&format!("{}.upload", Uuid::new_v4()), 25);
        // Still being written: its last write is recent.
        let running = file(&format!("{}.upload", Uuid::new_v4()), 1);
        // Not the temp-file pattern, however old.
        let blob = file(&format!("{}.blob", Uuid::new_v4()), 48);
        let other = file("notes.upload", 48);
        let suffixed = file(&format!("{}.upload.bak", Uuid::new_v4()), 48);
        let nested = dir.join(format!("{}.upload", Uuid::new_v4()));
        std::fs::create_dir(&nested).unwrap();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(sweep_stale_upload_temp_files(&dir, now).await.unwrap(), 1);
        assert!(!stale.exists(), "the stale temp file is deleted");
        for kept in [&running, &blob, &other, &suffixed, &nested] {
            assert!(kept.exists(), "{} is kept", kept.display());
        }
        // A missing storage directory (nothing uploaded yet) is no error.
        assert_eq!(
            sweep_stale_upload_temp_files(&dir.join("missing"), now)
                .await
                .unwrap(),
            0
        );
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
    async fn media_path_signal_reaches_only_the_unblocked_direct_peer() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let conversation = app.direct(&alice, &bob).await;
        let sender = app.public_user(&alice).await;
        let outsider = app.public_user(&carol).await;
        let signal = |payload: serde_json::Value| {
            serde_json::json!({
                "type": "voice_signal", "conversation_id": conversation, "to_user_id": bob.id,
                "kind": "media_path", "payload": payload,
            })
            .to_string()
        };
        // Relays one signal and returns the media_path events Bob receives.
        let collect = |from: PublicUser, text: String| {
            let state = app.state.clone();
            let bob_id = bob.id.clone();
            async move {
                let mut events = state.events.subscribe();
                relay_voice_signal(&state, &from, &text).await;
                let mut to_bob = Vec::new();
                while let Ok(Ok(event)) =
                    tokio::time::timeout(Duration::from_millis(200), events.recv()).await
                {
                    if event.recipients.contains(&bob_id) && event.event["kind"] == "media_path" {
                        to_bob.push(event.event.clone());
                    }
                }
                to_bob
            }
        };
        let delivered = collect(
            sender.clone(),
            signal(serde_json::json!({"path": "server"})),
        )
        .await;
        assert_eq!(delivered.len(), 1, "the switch reaches the peer");
        assert_eq!(delivered[0]["payload"]["path"], "server");
        assert_eq!(delivered[0]["from_user_id"], alice.id);
        for payload in [
            serde_json::json!({"path": "turn"}),
            serde_json::json!({"path": "server", "extra": "x".repeat(10_000)}),
            serde_json::Value::Null,
        ] {
            let dropped = collect(sender.clone(), signal(payload.clone())).await;
            assert!(dropped.is_empty(), "invalid payload relayed: {payload}");
        }
        let foreign = collect(
            outsider.clone(),
            signal(serde_json::json!({"path": "server"})),
        )
        .await;
        assert!(
            foreign.is_empty(),
            "a non-member cannot switch someone's call"
        );
        app.block(&bob, &alice).await;
        let blocked = collect(
            sender.clone(),
            signal(serde_json::json!({"path": "server"})),
        )
        .await;
        assert!(blocked.is_empty(), "a blocked pair exchanges no switch");
    }

    // Client for the server media relay (`/v1/media/ws`): binary frames of
    // any length, and a bounded wait for the next frame.
    struct MediaSocket(tokio::net::TcpStream);

    impl MediaSocket {
        /// Connect and say hello; returns the socket and the server's first
        /// text reply (`ready` or `error`).
        async fn join(base: &str, token: &str, conversation: &str) -> (Self, serde_json::Value) {
            let mut socket = Self::upgrade(base, "/v1/media/ws").await;
            let hello = serde_json::json!({"token": token, "conversation_id": conversation});
            socket.send(1, hello.to_string().as_bytes()).await;
            let reply = match socket.next(Duration::from_secs(3)).await {
                Some((1, text)) => serde_json::from_slice(&text).unwrap(),
                other => panic!("no reply to the hello: {other:?}"),
            };
            (socket, reply)
        }

        /// Open a WebSocket on `path` without saying anything yet.
        async fn upgrade(base: &str, path: &str) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let addr = base.trim_start_matches("http://");
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
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
            Self(stream)
        }

        async fn send(&mut self, opcode: u8, payload: &[u8]) {
            use tokio::io::AsyncWriteExt;
            let mask = [0x12u8, 0x34, 0x56, 0x78];
            let mut frame = vec![0x80 | opcode];
            if payload.len() < 126 {
                frame.push(0x80 | payload.len() as u8);
            } else if payload.len() <= usize::from(u16::MAX) {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            } else {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            }
            frame.extend_from_slice(&mask);
            frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            let _ = self.0.write_all(&frame).await;
        }

        /// The next frame (opcode, payload) within `wait`; None on timeout
        /// or a closed connection.
        async fn next(&mut self, wait: Duration) -> Option<(u8, Vec<u8>)> {
            use tokio::io::AsyncReadExt;
            tokio::time::timeout(wait, async {
                let mut head = [0u8; 2];
                self.0.read_exact(&mut head).await.ok()?;
                let mut len = u64::from(head[1] & 0x7f);
                if len == 126 {
                    let mut ext = [0u8; 2];
                    self.0.read_exact(&mut ext).await.ok()?;
                    len = u64::from(u16::from_be_bytes(ext));
                } else if len == 127 {
                    let mut ext = [0u8; 8];
                    self.0.read_exact(&mut ext).await.ok()?;
                    len = u64::from_be_bytes(ext);
                }
                let mut payload = vec![0u8; len as usize];
                self.0.read_exact(&mut payload).await.ok()?;
                Some((head[0] & 0x0f, payload))
            })
            .await
            .ok()
            .flatten()
        }

        /// The next binary frame within `wait`, skipping text events; None
        /// on timeout or close.
        async fn next_media(&mut self, wait: Duration) -> Option<Vec<u8>> {
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                match self.next(left).await? {
                    (2, payload) => return Some(payload),
                    (8, _) => return None,
                    _ => continue,
                }
            }
        }
    }

    fn relayed_from(sender: &str, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![sender.len() as u8];
        frame.extend_from_slice(sender.as_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[tokio::test]
    async fn media_relay_refuses_bad_sessions_and_non_members() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let mallory = app.user("mallory").await;
        let direct = app.direct(&alice, &bob).await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let (_, reply) = MediaSocket::join(&app.base, "not-a-session", &direct).await;
        assert_eq!(reply["code"], "unauthorized");
        for room in [&direct, &group] {
            let (_, reply) = MediaSocket::join(&app.base, &mallory.token, room).await;
            assert_eq!(
                reply["code"], "forbidden",
                "a non-member cannot join {room}"
            );
        }
        let (_, reply) = MediaSocket::join(&app.base, &bob.token, &group).await;
        assert_eq!(reply["type"], "ready");
        let reply: serde_json::Value = app
            .request(reqwest::Method::GET, "/v1/voice/ice", &alice)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            reply["media_relay"], true,
            "clients learn that the relay exists"
        );
    }

    #[tokio::test]
    async fn media_relay_caps_the_call_rooms_one_user_holds_at_once() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let mut groups = Vec::new();
        for _ in 0..5 {
            groups.push(app.group(&alice, &[&bob, &carol]).await);
        }
        let mut open = Vec::new();
        for group in &groups[..4] {
            let (socket, reply) = MediaSocket::join(&app.base, &alice.token, group).await;
            assert_eq!(reply["type"], "ready");
            open.push(socket);
        }
        let (_, reply) = MediaSocket::join(&app.base, &alice.token, &groups[4]).await;
        assert_eq!(
            reply["code"], "too_many_rooms",
            "a fifth room at once is refused"
        );
        // Another user is not affected, and rejoining a held room (a newer
        // device replacing the older) is not a new room.
        let (_, reply) = MediaSocket::join(&app.base, &bob.token, &groups[4]).await;
        assert_eq!(reply["type"], "ready");
        let (replacement, reply) = MediaSocket::join(&app.base, &alice.token, &groups[0]).await;
        assert_eq!(reply["type"], "ready");
        open[0] = replacement;
        // Leaving a room frees its place.
        drop(open.pop());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (_, reply) = MediaSocket::join(&app.base, &alice.token, &groups[4]).await;
            if reply["type"] == "ready" {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{reply}");
            // Slow enough to stay under the join rate limit.
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    #[tokio::test]
    async fn media_relay_forwards_frames_between_direct_and_group_members() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let direct = app.direct(&alice, &bob).await;
        let (mut a, ready) = MediaSocket::join(&app.base, &alice.token, &direct).await;
        assert_eq!(ready["peers"], serde_json::json!([]));
        let (mut b, ready) = MediaSocket::join(&app.base, &bob.token, &direct).await;
        assert_eq!(ready["peers"], serde_json::json!([alice.id]));
        let joined = a.next(Duration::from_secs(2)).await.unwrap();
        let joined: serde_json::Value = serde_json::from_slice(&joined.1).unwrap();
        assert_eq!(joined["type"], "peer_joined");
        assert_eq!(joined["user_id"], bob.id);
        a.send(2, b"\x01opus-a").await;
        assert_eq!(
            b.next_media(Duration::from_secs(2)).await,
            Some(relayed_from(&alice.id, b"\x01opus-a"))
        );
        b.send(2, b"\x01opus-b").await;
        assert_eq!(
            a.next_media(Duration::from_secs(2)).await,
            Some(relayed_from(&bob.id, b"\x01opus-b"))
        );
        assert_eq!(
            b.next_media(Duration::from_millis(200)).await,
            None,
            "no echo"
        );

        let group = app.group(&alice, &[&bob, &carol]).await;
        let (mut ga, _) = MediaSocket::join(&app.base, &alice.token, &group).await;
        let (mut gb, _) = MediaSocket::join(&app.base, &bob.token, &group).await;
        let (mut gc, _) = MediaSocket::join(&app.base, &carol.token, &group).await;
        gc.send(2, b"\x01carol").await;
        for socket in [&mut ga, &mut gb] {
            assert_eq!(
                socket.next_media(Duration::from_secs(2)).await,
                Some(relayed_from(&carol.id, b"\x01carol"))
            );
        }
        // Frames of the group room never leak into the direct room.
        assert_eq!(a.next_media(Duration::from_millis(200)).await, None);
    }

    #[tokio::test]
    async fn media_relay_drops_frames_once_a_peer_is_blocked_or_removed() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let dave = app.user("dave").await;
        let direct = app.direct(&alice, &bob).await;
        let (mut a, _) = MediaSocket::join(&app.base, &alice.token, &direct).await;
        let (mut b, _) = MediaSocket::join(&app.base, &bob.token, &direct).await;
        a.send(2, b"\x01before").await;
        assert!(b.next_media(Duration::from_secs(2)).await.is_some());
        app.block(&bob, &alice).await;
        for _ in 0..5 {
            a.send(2, b"\x01after-block").await;
        }
        assert_eq!(
            b.next_media(Duration::from_millis(500)).await,
            None,
            "the blocking user hears nothing more"
        );
        let (_, reply) = MediaSocket::join(&app.base, &alice.token, &direct).await;
        assert_eq!(reply["code"], "forbidden", "a blocked pair cannot rejoin");

        // A removed group member neither receives nor sends.
        let group = app.group(&alice, &[&carol, &dave]).await;
        let (mut ga, _) = MediaSocket::join(&app.base, &alice.token, &group).await;
        let (mut gc, _) = MediaSocket::join(&app.base, &carol.token, &group).await;
        gc.send(2, b"\x01still-member").await;
        assert!(ga.next_media(Duration::from_secs(2)).await.is_some());
        let status = app
            .request(
                reqwest::Method::DELETE,
                &format!("/v1/conversations/{group}/members/{}", carol.id),
                &alice,
            )
            .send()
            .await
            .unwrap()
            .status();
        assert!(status.is_success(), "removing carol returned {status}");
        ga.send(2, b"\x01after-removal").await;
        assert_eq!(gc.next_media(Duration::from_millis(500)).await, None);
        gc.send(2, b"\x01from-removed").await;
        assert_eq!(ga.next_media(Duration::from_millis(500)).await, None);
    }

    #[tokio::test]
    async fn sockets_are_pinged_and_a_silent_client_is_closed() {
        // Tunnels such as Cloudflare drop WebSockets that carry nothing for
        // about 100 s: the server pings both the app socket and the media
        // relay socket, and closes a client that stopped answering.
        let keepalive = Keepalive {
            interval: Duration::from_millis(100),
            timeout: Duration::from_millis(600),
        };
        let app = TestApp::with_keepalive(keepalive).await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        async fn open(app: &TestApp, kind: &str, user: &TestUser, room: &str) -> MediaSocket {
            if kind == "media" {
                let (socket, reply) = MediaSocket::join(&app.base, &user.token, room).await;
                assert_eq!(reply["type"], "ready");
                return socket;
            }
            let mut socket = MediaSocket::upgrade(&app.base, "/v1/ws").await;
            let hello = serde_json::json!({ "token": user.token });
            socket.send(1, hello.to_string().as_bytes()).await;
            match socket.next(Duration::from_secs(3)).await {
                Some((1, text)) => assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&text).unwrap()["type"],
                    "ready"
                ),
                other => panic!("no ready on the app socket: {other:?}"),
            }
            socket
        }
        for kind in ["app", "media"] {
            // A client that answers the pings stays connected past the timeout.
            let mut live = open(&app, kind, &alice, &direct).await;
            let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
            let mut pings = 0;
            loop {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                match live.next(left).await {
                    Some((9, payload)) => {
                        pings += 1;
                        live.send(10, &payload).await;
                    }
                    Some((8, _)) => panic!("{kind}: a client answering pings was closed"),
                    None if tokio::time::Instant::now() + Duration::from_millis(20) < deadline => {
                        panic!("{kind}: a client answering pings was disconnected")
                    }
                    _ => {}
                }
            }
            assert!(
                pings >= 5,
                "{kind}: the server pings every interval ({pings})"
            );
            // A client that never answers is closed after the timeout.
            let started = tokio::time::Instant::now();
            let mut silent = open(&app, kind, &bob, &direct).await;
            let closed = loop {
                match silent.next(Duration::from_secs(3)).await {
                    Some((8, _)) | None => break started.elapsed(),
                    Some(_) => continue,
                }
            };
            assert!(
                closed >= Duration::from_millis(500) && closed < Duration::from_millis(2500),
                "{kind}: a silent client is closed after the timeout ({closed:?})"
            );
        }
    }

    #[tokio::test]
    async fn media_relay_limits_frame_size_and_rate() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        let (mut a, _) = MediaSocket::join(&app.base, &alice.token, &direct).await;
        let (mut b, _) = MediaSocket::join(&app.base, &bob.token, &direct).await;
        // A flood far above the frame rate: only about the burst arrives.
        let reader = tokio::spawn(async move {
            let mut received = 0;
            while b.next_media(Duration::from_millis(700)).await.is_some() {
                received += 1;
            }
            received
        });
        for _ in 0..1000 {
            a.send(2, b"\x01x").await;
        }
        let received = reader.await.unwrap();
        assert!(
            (1..400).contains(&received),
            "{received} of 1000 flooded frames were forwarded"
        );
        // An oversized frame is never forwarded; the connection is closed.
        let (mut b, _) = MediaSocket::join(&app.base, &bob.token, &direct).await;
        a.send(2, &vec![1u8; media_relay::MAX_FRAME_BYTES + 1])
            .await;
        assert_eq!(b.next_media(Duration::from_millis(500)).await, None);
        loop {
            match a.next(Duration::from_secs(2)).await {
                None | Some((8, _)) => break,
                Some((2, _)) => panic!("the oversized sender still receives media"),
                Some(_) => continue,
            }
        }
    }

    /// Screen video (client frame kind 2) has its own budget: a burst of
    /// large key frame fragments must not drop the call audio (kind 1).
    #[tokio::test]
    async fn media_relay_screen_video_cannot_starve_the_call_audio() {
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let direct = app.direct(&alice, &bob).await;
        let (mut a, _) = MediaSocket::join(&app.base, &alice.token, &direct).await;
        let (mut b, _) = MediaSocket::join(&app.base, &bob.token, &direct).await;
        let reader = tokio::spawn(async move {
            let mut received = Vec::new();
            while let Some(frame) = b.next_media(Duration::from_millis(700)).await {
                received.push(frame);
            }
            received
        });
        // Key frame fragments of the largest size use up the whole byte
        // burst; the audio frame right after them must still pass.
        let fragment = [vec![2u8], vec![0u8; media_relay::MAX_FRAME_BYTES - 1]].concat();
        for _ in 0..12 {
            a.send(2, &fragment).await;
        }
        let audio = [b"\x01".to_vec(), vec![7u8; 16 * 1024]].concat();
        a.send(2, &audio).await;
        let received = reader.await.unwrap();
        let video = received.iter().filter(|f| f.ends_with(&[0u8; 64])).count();
        assert!(video < 12, "the video burst is limited ({video} forwarded)");
        assert!(
            received.contains(&relayed_from(&alice.id, &audio)),
            "the audio frame after a video burst is forwarded"
        );
    }

    /// A well-formed sealed planner payload (opaque to the server).
    fn sealed_planner(bytes: usize) -> serde_json::Value {
        let mut payload = b"GP\x01".to_vec();
        payload.extend((0..bytes).map(|index| (index % 251) as u8));
        serde_json::json!({
            "ciphertext": URL_SAFE_NO_PAD.encode(payload),
            "epoch": 3,
            "key_id": "0123456789abcdef0123456789abcdef",
        })
    }

    fn with_fields(mut value: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        for (key, item) in extra.as_object().unwrap() {
            value[key] = item.clone();
        }
        value
    }

    async fn lock_group_crypto(app: &TestApp, group: &str, owner: &TestUser) {
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(group)
        .bind(&owner.id)
        .execute(&app.state.db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn encrypted_group_planner_stores_only_ciphertext_and_refuses_plaintext() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let outsider = app.user("outsider").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        lock_group_crypto(&app, &group, &alice).await;
        let polls_path = format!("/v1/conversations/{group}/planner/polls");
        let events_path = format!("/v1/conversations/{group}/planner/events");
        let post = |path: &str, user: &TestUser, body: serde_json::Value| {
            app.request(Method::POST, path, user).json(&body).send()
        };

        // An older client's plaintext writes are refused with a stable code.
        for (path, body) in [
            (
                polls_path.as_str(),
                serde_json::json!({"question": "Secret?", "options": ["A", "B"], "closes_at": null}),
            ),
            (
                events_path.as_str(),
                serde_json::json!({"title": "Secret party", "details": "", "scheduled_at": "2999-01-01T19:00:00Z"}),
            ),
        ] {
            let response = post(path, &bob, body).await.unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], "planner_encryption_required", "{body}");
        }

        // The shape and size of sealed data are checked.
        let poll_id = Uuid::new_v4().to_string();
        for bad in [
            sealed_planner(40),
            with_fields(sealed_planner(40), serde_json::json!({"id": "not-a-uuid"})),
            with_fields(
                sealed_planner(40),
                serde_json::json!({"id": poll_id, "key_id": "NOT-HEX"}),
            ),
            with_fields(
                sealed_planner(40),
                serde_json::json!({"id": poll_id, "epoch": -1}),
            ),
            with_fields(sealed_planner(9 * 1024), serde_json::json!({"id": poll_id})),
            with_fields(sealed_planner(2), serde_json::json!({"id": poll_id})),
            with_fields(
                sealed_planner(40),
                serde_json::json!({"id": poll_id, "ciphertext": "!!!"}),
            ),
            with_fields(
                sealed_planner(40),
                serde_json::json!({"id": poll_id, "question": "Leak?"}),
            ),
            serde_json::json!({"id": poll_id, "ciphertext": sealed_planner(40)["ciphertext"]}),
        ] {
            let status = post(&polls_path, &bob, bad.clone()).await.unwrap().status();
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        }

        let created: serde_json::Value = post(
            &polls_path,
            &bob,
            with_fields(
                sealed_planner(64),
                serde_json::json!({"id": poll_id, "closes_at": "2999-01-01T00:00:00.000Z"}),
            ),
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        let poll = &created[0];
        assert_eq!(poll["id"], poll_id);
        assert_eq!(poll["question"], serde_json::Value::Null);
        assert_eq!(poll["options"], serde_json::json!([]));
        assert_eq!(poll["ciphertext"], sealed_planner(64)["ciphertext"]);
        assert_eq!(poll["epoch"], 3);
        assert_eq!(poll["key_id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(poll["creator_id"], bob.id.as_str());
        assert_eq!(poll["closes_at"], "2999-01-01T00:00:00.000Z");
        let status = post(
            &polls_path,
            &carol,
            with_fields(sealed_planner(64), serde_json::json!({"id": poll_id})),
        )
        .await
        .unwrap()
        .status();
        assert_eq!(status, StatusCode::CONFLICT, "an item id is used once");
        let status = post(
            &polls_path,
            &outsider,
            with_fields(
                sealed_planner(64),
                serde_json::json!({"id": Uuid::new_v4().to_string()}),
            ),
        )
        .await
        .unwrap()
        .status();
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Votes: plaintext is refused, ciphertext is stored per voter.
        let vote_path = format!("/v1/group-polls/{poll_id}/vote");
        let response = post(&vote_path, &carol, serde_json::json!({"option_id": "x"}))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], "planner_encryption_required");
        let status = post(
            &vote_path,
            &carol,
            with_fields(sealed_planner(32), serde_json::json!({"option_id": "x"})),
        )
        .await
        .unwrap()
        .status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let status = post(&vote_path, &carol, sealed_planner(2 * 1024))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::BAD_REQUEST, "a vote is small");
        for user in [&carol, &alice, &carol] {
            let status = post(&vote_path, user, sealed_planner(32))
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        let polls: serde_json::Value = app
            .request(Method::GET, &polls_path, &bob)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let responses = polls[0]["responses"].as_array().unwrap();
        assert_eq!(responses.len(), 2, "one encrypted vote per voter");
        let mut voters: Vec<_> = responses
            .iter()
            .map(|row| row["user_id"].as_str().unwrap().to_string())
            .collect();
        voters.sort();
        let mut expected = vec![alice.id.clone(), carol.id.clone()];
        expected.sort();
        assert_eq!(voters, expected);
        assert_eq!(responses[0]["ciphertext"], sealed_planner(32)["ciphertext"]);
        assert_eq!(responses[0]["key_id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(polls[0]["my_option_id"], serde_json::Value::Null);

        // Events and RSVPs.
        let event_id = Uuid::new_v4().to_string();
        let status = post(
            &events_path,
            &alice,
            with_fields(
                sealed_planner(80),
                serde_json::json!({"id": event_id, "title": "Leak"}),
            ),
        )
        .await
        .unwrap()
        .status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let events: serde_json::Value = post(
            &events_path,
            &alice,
            with_fields(sealed_planner(80), serde_json::json!({"id": event_id})),
        )
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(events[0]["id"], event_id);
        assert_eq!(events[0]["title"], serde_json::Value::Null);
        assert_eq!(events[0]["scheduled_at"], serde_json::Value::Null);
        assert_eq!(events[0]["ciphertext"], sealed_planner(80)["ciphertext"]);
        let rsvp_path = format!("/v1/group-events/{event_id}/rsvp");
        let response = post(&rsvp_path, &bob, serde_json::json!({"status": "going"}))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], "planner_encryption_required");
        let status = post(&rsvp_path, &bob, sealed_planner(28))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let events: serde_json::Value = app
            .request(Method::GET, &events_path, &carol)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(events[0]["going"], 0, "the server cannot count answers");
        assert_eq!(events[0]["responses"][0]["user_id"], bob.id.as_str());

        // Nothing readable reached the database.
        let plaintext = sqlx::query_scalar::<_, i64>(
            "SELECT (SELECT count(*) FROM group_polls WHERE question IS NOT NULL)+(SELECT count(*) FROM group_poll_options)+(SELECT count(*) FROM group_poll_votes WHERE option_id IS NOT NULL)+(SELECT count(*) FROM group_events WHERE title IS NOT NULL OR details IS NOT NULL OR scheduled_at IS NOT NULL)+(SELECT count(*) FROM group_event_rsvps WHERE status IS NOT NULL)",
        )
        .fetch_one(&app.state.db)
        .await
        .unwrap();
        assert_eq!(plaintext, 0);
    }

    #[tokio::test]
    async fn plaintext_group_planner_is_unchanged_and_refuses_ciphertext() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let polls_path = format!("/v1/conversations/{group}/planner/polls");
        let response = app
            .request(Method::POST, &polls_path, &alice)
            .json(&with_fields(
                sealed_planner(40),
                serde_json::json!({"id": Uuid::new_v4().to_string()}),
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], "planner_not_encrypted");
        let polls: serde_json::Value = app
            .request(Method::POST, &polls_path, &alice)
            .json(&serde_json::json!({"question": "When?", "options": ["Mon", "Tue"], "closes_at": null}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(polls[0]["question"], "When?");
        assert_eq!(polls[0]["ciphertext"], serde_json::Value::Null);
        let vote_path = format!("/v1/group-polls/{}/vote", polls[0]["id"].as_str().unwrap());
        let status = app
            .request(Method::POST, &vote_path, &bob)
            .json(&sealed_planner(32))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let status = app
            .request(Method::POST, &vote_path, &bob)
            .json(&serde_json::json!({"option_id": polls[0]["options"][1]["id"]}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let events: serde_json::Value = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{group}/planner/events"),
                &alice,
            )
            .json(&serde_json::json!({"title": "Dinner", "details": "Bring food", "scheduled_at": "2999-01-01T19:00:00Z"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(events[0]["title"], "Dinner");
        let rsvp_path = format!(
            "/v1/group-events/{}/rsvp",
            events[0]["id"].as_str().unwrap()
        );
        for (body, expected) in [
            (
                serde_json::json!({"status": "nope"}),
                StatusCode::BAD_REQUEST,
            ),
            (sealed_planner(28), StatusCode::BAD_REQUEST),
            (
                serde_json::json!({"status": "maybe"}),
                StatusCode::NO_CONTENT,
            ),
        ] {
            let status = app
                .request(Method::POST, &rsvp_path, &bob)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, expected, "{body}");
        }
        let polls: serde_json::Value = app
            .request(Method::GET, &polls_path, &bob)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(polls[0]["options"][1]["votes"], 1);
        assert_eq!(polls[0]["my_option_id"], polls[0]["options"][1]["id"]);
        assert_eq!(polls[0]["responses"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn items_stored_before_encryption_stay_and_take_encrypted_answers() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        let polls_path = format!("/v1/conversations/{group}/planner/polls");
        let polls: serde_json::Value = app
            .request(Method::POST, &polls_path, &alice)
            .json(&serde_json::json!({"question": "Before?", "options": ["Yes", "No"], "closes_at": null}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let vote_path = format!("/v1/group-polls/{}/vote", polls[0]["id"].as_str().unwrap());
        app.request(Method::POST, &vote_path, &bob)
            .json(&serde_json::json!({"option_id": polls[0]["options"][0]["id"]}))
            .send()
            .await
            .unwrap();
        lock_group_crypto(&app, &group, &alice).await;
        let polls: serde_json::Value = app
            .request(Method::GET, &polls_path, &bob)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            polls[0]["question"], "Before?",
            "the old item is not rewritten"
        );
        assert_eq!(polls[0]["options"][0]["votes"], 1);
        let status = app
            .request(Method::POST, &vote_path, &bob)
            .json(&serde_json::json!({"option_id": polls[0]["options"][1]["id"]}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::CONFLICT, "no new plaintext answers");
        let status = app
            .request(Method::POST, &vote_path, &bob)
            .json(&sealed_planner(32))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let polls: serde_json::Value = app
            .request(Method::GET, &polls_path, &bob)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            polls[0]["options"][0]["votes"], 0,
            "the encrypted vote replaced the plaintext one"
        );
        assert_eq!(polls[0]["my_option_id"], serde_json::Value::Null);
        assert_eq!(polls[0]["responses"][0]["user_id"], bob.id.as_str());
    }

    #[tokio::test]
    async fn account_deletion_keeps_encrypted_planner_items_with_an_anonymous_creator() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let group = app.group(&alice, &[&bob, &carol]).await;
        lock_group_crypto(&app, &group, &alice).await;
        let poll_id = Uuid::new_v4().to_string();
        let event_id = Uuid::new_v4().to_string();
        for (path, id) in [
            (format!("/v1/conversations/{group}/planner/polls"), &poll_id),
            (
                format!("/v1/conversations/{group}/planner/events"),
                &event_id,
            ),
        ] {
            let status = app
                .request(Method::POST, &path, &carol)
                .json(&with_fields(
                    sealed_planner(40),
                    serde_json::json!({"id": id}),
                ))
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::OK);
        }
        for user in [&alice, &carol] {
            for path in [
                format!("/v1/group-polls/{poll_id}/vote"),
                format!("/v1/group-events/{event_id}/rsvp"),
            ] {
                let status = app
                    .request(Method::POST, &path, user)
                    .json(&sealed_planner(28))
                    .send()
                    .await
                    .unwrap()
                    .status();
                assert_eq!(status, StatusCode::NO_CONTENT);
            }
        }
        let status = app
            .request(Method::DELETE, "/v1/me/account", &carol)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        for path in [
            format!("/v1/conversations/{group}/planner/polls"),
            format!("/v1/conversations/{group}/planner/events"),
        ] {
            let items: serde_json::Value = app
                .request(Method::GET, &path, &bob)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(items.as_array().unwrap().len(), 1, "the item survives");
            assert_eq!(items[0]["creator_id"], serde_json::Value::Null);
            let responses = items[0]["responses"].as_array().unwrap();
            assert_eq!(responses.len(), 1, "the deleted account's answer is gone");
            assert_eq!(responses[0]["user_id"], alice.id.as_str());
        }
    }

    #[tokio::test]
    async fn migration_0033_keeps_planner_rows() {
        let dir = std::env::temp_dir().join(format!("ghostchat-migrate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let old = database_migrated_to(&path, 32).await;
        for user in ["u-a", "u-b"] {
            seed_user(&old, user).await;
        }
        seed_group(&old, "g-1", &["u-a", "u-b"]).await;
        for statement in [
            "INSERT INTO group_polls(id,conversation_id,creator_id,question,closes_at,created_at) VALUES('p-1','g-1','u-a','When?','2999-01-01T00:00:00Z','2026-01-01T00:00:00.000Z')",
            "INSERT INTO group_poll_options(id,poll_id,label,position) VALUES('o-1','p-1','Mon',0),('o-2','p-1','Tue',1)",
            "INSERT INTO group_poll_votes(poll_id,user_id,option_id,created_at) VALUES('p-1','u-b','o-2','2026-01-01T00:00:01.000Z')",
            "INSERT INTO group_events(id,conversation_id,creator_id,title,details,scheduled_at,created_at) VALUES('e-1','g-1',NULL,'Dinner','Food','2999-01-01T19:00:00Z','2026-01-01T00:00:02.000Z')",
            "INSERT INTO group_event_rsvps(event_id,user_id,status,updated_at) VALUES('e-1','u-a','maybe','2026-01-01T00:00:03.000Z')",
        ] {
            sqlx::query(statement).execute(&old).await.unwrap();
        }
        let dump = "SELECT json_array((SELECT json_group_array(json_array(id,conversation_id,creator_id,question,closes_at,closed_at,created_at)) FROM group_polls),(SELECT json_group_array(json_array(id,poll_id,label,position)) FROM group_poll_options),(SELECT json_group_array(json_array(poll_id,user_id,option_id,created_at)) FROM group_poll_votes),(SELECT json_group_array(json_array(id,conversation_id,creator_id,title,details,scheduled_at,cancelled_at,created_at)) FROM group_events),(SELECT json_group_array(json_array(event_id,user_id,status,updated_at)) FROM group_event_rsvps))";
        let before: String = sqlx::query_scalar(dump).fetch_one(&old).await.unwrap();
        old.close().await;

        let db = open_database(&path.to_string_lossy()).await.unwrap();
        let after: String = sqlx::query_scalar(dump).fetch_one(&db).await.unwrap();
        assert_eq!(after, before);
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&db)
            .await
            .unwrap();
        assert!(violations.is_empty());
        let leftovers = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE '%0033%'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(leftovers, 0);
        // A row is plaintext or ciphertext, never both or neither.
        assert!(sqlx::query(
            "INSERT INTO group_polls(id,conversation_id,question,payload_ciphertext,payload_epoch,payload_key_id) VALUES('p-2','g-1','Both',x'00',1,'k')",
        )
        .execute(&db)
        .await
        .is_err());
        assert!(
            sqlx::query("INSERT INTO group_polls(id,conversation_id) VALUES('p-3','g-1')")
                .execute(&db)
                .await
                .is_err()
        );
        // Deleting a poll still cascades into its options and votes.
        sqlx::query("DELETE FROM group_polls WHERE id='p-1'")
            .execute(&db)
            .await
            .unwrap();
        let children = sqlx::query_scalar::<_, i64>(
            "SELECT (SELECT count(*) FROM group_poll_options)+(SELECT count(*) FROM group_poll_votes)",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(children, 0);
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
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
    async fn self_update_backup_replaces_the_previous_one_with_current_data() {
        let dir = std::env::temp_dir().join(format!("ghostchat-backup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let db = open_database(&path.to_string_lossy()).await.unwrap();
        seed_user(&db, "u-first").await;
        let written = backup_before_self_update(&db, &path.to_string_lossy())
            .await
            .unwrap();
        assert_eq!(written, Some(dir.join("db.sqlite.pre-update.bak")));
        seed_user(&db, "u-second").await;
        backup_before_self_update(&db, &path.to_string_lossy())
            .await
            .unwrap();
        db.close().await;
        assert_eq!(backup_files(&dir), vec!["db.sqlite.pre-update.bak"]);
        let backup = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(dir.join("db.sqlite.pre-update.bak"))
                    .read_only(true),
            )
            .await
            .unwrap();
        let mut users = sqlx::query_scalar::<_, String>("SELECT id FROM users")
            .fetch_all(&backup)
            .await
            .unwrap();
        users.sort();
        assert_eq!(users, vec!["u-first".to_string(), "u-second".to_string()]);
        let memory = open_database(":memory:").await.unwrap();
        assert_eq!(
            backup_before_self_update(&memory, ":memory:").await,
            Ok(None)
        );
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

    /// Clients up to 4.0.50 send no `expected_commit_seq`. Plain groups and
    /// messages keep working for them; an encrypted membership commit that
    /// names no group state cannot be fenced, so it is refused with a code
    /// and a message that asks for an update instead of a generic error.
    #[tokio::test]
    async fn requests_without_expected_commit_seq_from_older_clients() {
        use reqwest::{Method, StatusCode};
        let app = TestApp::new().await;
        let alice = app.user("alice").await;
        let bob = app.user("bob").await;
        let carol = app.user("carol").await;
        let eve = app.user("eve").await;
        app.befriend(&alice, &eve).await;

        // A plain group: adding and removing members is unchanged.
        let plain = app.group(&alice, &[&bob, &carol]).await;
        let added = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{plain}/members"),
                &alice,
            )
            .json(&serde_json::json!({"member_ids": [eve.id]}))
            .send()
            .await
            .unwrap();
        assert_eq!(added.status(), StatusCode::OK);
        assert!(is_member(&app.state.db, &plain, &eve.id).await.unwrap());
        let removed = app
            .request(
                Method::DELETE,
                &format!("/v1/conversations/{plain}/members/{}", eve.id),
                &alice,
            )
            .send()
            .await
            .unwrap();
        assert!(removed.status().is_success(), "{}", removed.status());
        assert!(!is_member(&app.state.db, &plain, &eve.id).await.unwrap());

        // An encrypted group at commit 0.
        let group = app.group(&alice, &[&bob, &carol]).await;
        sqlx::query(
            "INSERT INTO conversation_crypto_locks(conversation_id,locked_by,setup_complete) VALUES(?,?,1)",
        )
        .bind(&group)
        .bind(&alice.id)
        .execute(&app.state.db)
        .await
        .unwrap();
        let mut roster = vec![alice.id.clone(), bob.id.clone(), carol.id.clone()];
        roster.sort();
        let update = serde_json::json!({
            "operation_id": Uuid::new_v4().to_string(),
            "expected_member_ids": roster,
            "commit_payload": "AAAA",
            "welcome_payload": "AAAA",
        });
        let asks_for_update = |response: reqwest::Response| async move {
            let status = response.status();
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["code"], "stale_commit_seq", "{body}");
            assert!(
                body["error"].as_str().unwrap().contains("Update GhostChat"),
                "{body}"
            );
        };
        asks_for_update(
            app.request(
                Method::POST,
                &format!("/v1/conversations/{group}/members"),
                &alice,
            )
            .json(&serde_json::json!({"member_ids": [eve.id], "crypto_update": update}))
            .send()
            .await
            .unwrap(),
        )
        .await;
        assert!(!is_member(&app.state.db, &group, &eve.id).await.unwrap());
        let mut removal = update.clone();
        removal["welcome_payload"] = "".into();
        asks_for_update(
            app.request(
                Method::POST,
                &format!(
                    "/v1/conversations/{group}/crypto-membership/remove/{}",
                    carol.id
                ),
                &alice,
            )
            .json(&removal)
            .send()
            .await
            .unwrap(),
        )
        .await;
        assert!(is_member(&app.state.db, &group, &carol.id).await.unwrap());

        // An encrypted message without a group state is stored as before.
        let sent = app
            .request(
                Method::POST,
                &format!("/v1/conversations/{group}/messages"),
                &bob,
            )
            .json(&serde_json::json!({
                "client_message_id": Uuid::new_v4().to_string(),
                "content": "e2e1:AAAA",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(sent.status(), StatusCode::OK);
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
