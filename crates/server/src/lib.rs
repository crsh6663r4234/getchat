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
mod attachments;
mod auth;
mod conversations;
mod crypto_devices;
mod crypto_events;
mod crypto_lock;
mod database;
pub mod device_proof;
mod group_invites;
mod group_membership;
mod media_relay;
mod media_revocation;
mod messages;
mod planner;
pub mod ratelimit;
pub mod self_update;
mod social;
mod startup;
mod users;
mod voice;
mod websocket;

use account_data::*;
use admission::*;
use api_error::*;
use app_state::*;
pub use app_state::{build_state, build_state_with_attachment_dir, AppState};
use attachments::*;
use auth::*;
use conversations::*;
use crypto_devices::*;
use crypto_events::*;
use crypto_lock::*;
use database::*;
pub use database::{is_pre_upgrade_backup_error, open_database, PreUpgradeBackupError};
use group_invites::*;
use group_membership::*;
use messages::*;
use planner::*;
use social::*;
pub use startup::{bind_default, run_standalone, serve};
use users::*;
use voice::*;
use websocket::*;

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

#[cfg(test)]
mod tests;
