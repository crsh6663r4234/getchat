//! The shared server state and how it is built from the environment.

use super::*;

#[derive(Clone)]
pub struct AppState {
    pub(crate) db: SqlitePool,
    pub(crate) attachment_dir: PathBuf,
    pub(crate) attachment_ticket_key: [u8; 32],
    pub(crate) events: broadcast::Sender<ServerEvent>,
    pub(crate) limiter: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    pub(crate) online_users: Arc<Mutex<HashMap<String, usize>>>,
    pub(crate) voice: VoiceServerConfig,
    pub(crate) distributed_limiter: Result<Option<ratelimit::RateLimiter>, ratelimit::LimitError>,
    /// GHOSTCHAT_REQUIRE_REDIS=1, read once at startup.
    pub(crate) require_redis: bool,
    /// Call rooms of the server media relay (`/v1/media/ws`).
    pub(crate) media_rooms: media_relay::Rooms,
    /// WebSocket keepalive for `/v1/ws` and `/v1/media/ws`.
    pub(crate) keepalive: Keepalive,
}

/// Server-side WebSocket keepalive. Tunnels such as Cloudflare close a
/// WebSocket that carries nothing for about 100 s, so the server pings every
/// `interval`, and closes a socket that has sent nothing (not even a pong)
/// for `timeout`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Keepalive {
    pub(crate) interval: Duration,
    pub(crate) timeout: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(90),
        }
    }
}

#[derive(Clone)]
pub(crate) struct VoiceServerConfig {
    pub(crate) stun_urls: Vec<String>,
    pub(crate) turn_urls: Vec<String>,
    pub(crate) turn_shared_secret: Option<String>,
    pub(crate) livekit_url: Option<String>,
    pub(crate) livekit_api_key: Option<String>,
    pub(crate) livekit_api_secret: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ServerEvent {
    pub(crate) recipients: Vec<String>,
    pub(crate) event: serde_json::Value,
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
        media_rooms: media_relay::Rooms::default(),
        keepalive: Keepalive::default(),
    }
}

pub(crate) fn parse_server_urls(value: &str, allowed_schemes: &[&str]) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|url| allowed_schemes.iter().any(|scheme| url.starts_with(scheme)))
        .map(str::to_owned)
        .collect()
}
