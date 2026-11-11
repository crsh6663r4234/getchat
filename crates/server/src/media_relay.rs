//! Server media relay: forwards opaque binary media frames between the
//! participants of one call room over an authenticated WebSocket
//! (`/v1/media/ws`). Calls fall back to it when TURN or LiveKit (UDP) cannot
//! be reached, for example when the server sits behind an HTTP-only tunnel.
//!
//! Like TURN and LiveKit, the server sees the media: frames are not end-to-end
//! encrypted. Authorization follows call signaling: a direct room admits the
//! two members of the direct conversation while neither blocks the other; a
//! group room admits current members. Every block or membership removal bumps
//! an epoch, and the next frame forwarded in an affected room re-checks the
//! whole room first, so a blocked or removed user neither sends nor receives
//! another frame.
//!
//! Wire format. The client's first text message is
//! `{"token": "...", "conversation_id": "..."}`; the server answers
//! `{"type":"ready","user_id":..,"peers":[..]}` or `{"type":"error","code":..}`
//! and closes. Afterwards each binary message from a client (at most
//! [`MAX_FRAME_BYTES`]) reaches every other participant as
//! `[sender id length: u8][sender id bytes][the client's bytes unchanged]`.
//! The first byte of the client's bytes (its frame kind; 2 is screen video)
//! only picks the rate budget the frame counts against.
//! Text events `peer_joined` / `peer_left` announce participants.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;

pub(super) const MAX_FRAME_BYTES: usize = 64 * 1024;
const FRAME_RATE: f64 = 100.0;
const FRAME_BURST: f64 = 200.0;
const BYTE_RATE: f64 = 256.0 * 1024.0;
const BYTE_BURST: f64 = 512.0 * 1024.0;
/// The client's frame kind (first byte) of a screen-share video fragment.
/// Video gets the budget above; audio and the small control frames get
/// their own, so a key frame burst never drops call audio.
const VIDEO_KIND: u8 = 2;
const AUDIO_BYTE_RATE: f64 = 64.0 * 1024.0;
const AUDIO_BYTE_BURST: f64 = 128.0 * 1024.0;
const QUEUE_DEPTH: usize = 64;
const RECHECK_EVERY: Duration = Duration::from_secs(5);
/// Call rooms one user may hold at once (each socket rechecks its session
/// and membership every few seconds and may queue frames).
const MAX_ROOMS_PER_USER: usize = 4;
/// Relay sockets this server holds at once.
const MAX_PARTICIPANTS: usize = 512;

#[derive(Clone, Default)]
pub(super) struct Rooms {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    epoch: Arc<AtomicU64>,
    next_connection: Arc<AtomicU64>,
}

#[derive(Default)]
struct Room {
    checked_epoch: u64,
    participants: HashMap<String, Participant>,
}

struct Participant {
    connection: u64,
    revoked: Arc<AtomicBool>,
    outgoing: mpsc::Sender<WsMessage>,
}

impl Participant {
    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }
}

impl Rooms {
    /// A block, unblock or membership change happened somewhere: every room
    /// re-checks its participants before it forwards the next frame.
    pub(super) fn authorization_changed(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// Whether any call room has a participant (the self-updater defers a
    /// swap while one does, because the rooms live in this process).
    pub(super) fn any_active(&self) -> bool {
        !self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Room>> {
        match self.rooms.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Whether `user` may open a socket in the room of `conversation_id` given
/// the open rooms: a refusal code, or None. Replacing the user's own socket
/// in a room it already holds is never a new room.
fn over_capacity(
    rooms: &HashMap<String, Room>,
    user: &str,
    conversation_id: &str,
    per_user: usize,
    total: usize,
) -> Option<&'static str> {
    let rejoin = rooms
        .get(conversation_id)
        .is_some_and(|room| room.participants.contains_key(user));
    if rejoin {
        return None;
    }
    let held = rooms
        .values()
        .filter(|room| room.participants.contains_key(user))
        .count();
    if held >= per_user {
        return Some("too_many_rooms");
    }
    let open: usize = rooms.values().map(|room| room.participants.len()).sum();
    (open >= total).then_some("relay_full")
}

/// Token bucket over frames and bytes for one connection.
struct FrameBudget {
    frames: f64,
    bytes: f64,
    byte_rate: f64,
    byte_burst: f64,
    at: Instant,
}

impl FrameBudget {
    fn new() -> Self {
        Self::with_bytes(BYTE_RATE, BYTE_BURST)
    }

    fn with_bytes(byte_rate: f64, byte_burst: f64) -> Self {
        Self {
            frames: FRAME_BURST,
            bytes: byte_burst,
            byte_rate,
            byte_burst,
            at: Instant::now(),
        }
    }

    fn admit(&mut self, size: usize) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_secs_f64();
        self.at = now;
        self.frames = (self.frames + elapsed * FRAME_RATE).min(FRAME_BURST);
        self.bytes = (self.bytes + elapsed * self.byte_rate).min(self.byte_burst);
        if self.frames < 1.0 || self.bytes < size as f64 {
            return false;
        }
        self.frames -= 1.0;
        self.bytes -= size as f64;
        true
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaHello {
    token: String,
    conversation_id: String,
}

/// May `user_id` take part in the call room of `conversation_id`?
async fn allowed(db: &SqlitePool, conversation_id: &str, user_id: &str) -> bool {
    let kind = sqlx::query_scalar::<_, String>(
        "SELECT c.kind FROM conversations c JOIN conversation_members cm ON cm.conversation_id=c.id WHERE c.id=? AND cm.user_id=?",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(db)
    .await;
    match kind {
        Ok(Some(kind)) if kind == "group" => true,
        Ok(Some(kind)) if kind == "direct" => {
            ensure_direct_not_blocked(db, conversation_id, user_id)
                .await
                .is_ok()
        }
        _ => false,
    }
}

/// Re-check every participant of a room after an authorization change and
/// evict the ones no longer allowed.
async fn revalidate(state: &AppState, conversation_id: &str, epoch: u64) {
    let users: Vec<String> = {
        let rooms = state.media_rooms.lock();
        match rooms.get(conversation_id) {
            Some(room) => room.participants.keys().cloned().collect(),
            None => return,
        }
    };
    let mut refused = Vec::new();
    for user in users {
        if !allowed(&state.db, conversation_id, &user).await {
            refused.push(user);
        }
    }
    let mut rooms = state.media_rooms.lock();
    if let Some(room) = rooms.get_mut(conversation_id) {
        for user in refused {
            if let Some(participant) = room.participants.remove(&user) {
                participant.revoke();
            }
        }
        room.checked_epoch = room.checked_epoch.max(epoch);
    }
}

pub(super) async fn websocket(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| session(socket, state))
}

async fn refuse(mut socket: WebSocket, code: &str) {
    let _ = socket
        .send(WsMessage::Text(
            serde_json::json!({"type": "error", "code": code}).to_string(),
        ))
        .await;
    let _ = socket.send(WsMessage::Close(None)).await;
}

async fn session_user(state: &AppState, token_hash: &str) -> Option<PublicUser> {
    sqlx::query_as::<_, PublicUser>("SELECT u.id,u.username,u.display_name,u.avatar_data FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND datetime(s.expires_at)>datetime('now')")
        .bind(token_hash)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

fn announce(room: &Room, except: &str, event: serde_json::Value) {
    let text = event.to_string();
    for (user, participant) in &room.participants {
        if user != except {
            let _ = participant.outgoing.try_send(WsMessage::Text(text.clone()));
        }
    }
}

async fn session(mut socket: WebSocket, state: AppState) {
    let first = tokio::time::timeout(Duration::from_secs(8), socket.recv()).await;
    let hello = match first {
        Ok(Some(Ok(WsMessage::Text(text)))) => serde_json::from_str::<MediaHello>(&text).ok(),
        _ => None,
    };
    let Some(hello) = hello.filter(|hello| hello.conversation_id.len() <= 64) else {
        return refuse(socket, "bad_request").await;
    };
    let token_hash = session_hash(&hello.token);
    let Some(user) = session_user(&state, &token_hash).await else {
        return refuse(socket, "unauthorized").await;
    };
    let conversation_id = hello.conversation_id;
    if !allow_rate(
        &state,
        &format!("media-join:{}", user.id),
        30,
        Duration::from_secs(60),
    ) {
        return refuse(socket, "rate_limited").await;
    }
    if !allowed(&state.db, &conversation_id, &user.id).await {
        return refuse(socket, "forbidden").await;
    }

    let connection = state
        .media_rooms
        .next_connection
        .fetch_add(1, Ordering::SeqCst);
    let revoked = Arc::new(AtomicBool::new(false));
    let (outgoing, mut queue) = mpsc::channel::<WsMessage>(QUEUE_DEPTH);
    let joined: Result<Vec<String>, &'static str> = {
        let epoch = state.media_rooms.epoch.load(Ordering::SeqCst);
        let mut rooms = state.media_rooms.lock();
        if let Some(code) = over_capacity(
            &rooms,
            &user.id,
            &conversation_id,
            MAX_ROOMS_PER_USER,
            MAX_PARTICIPANTS,
        ) {
            Err(code)
        } else {
            let room = rooms
                .entry(conversation_id.clone())
                .or_insert_with(|| Room {
                    checked_epoch: epoch,
                    participants: HashMap::new(),
                });
            // One connection per user and room: a newer device replaces the older.
            if let Some(previous) = room.participants.insert(
                user.id.clone(),
                Participant {
                    connection,
                    revoked: revoked.clone(),
                    outgoing,
                },
            ) {
                previous.revoke();
            }
            announce(
                room,
                &user.id,
                serde_json::json!({"type": "peer_joined", "user_id": user.id}),
            );
            Ok(room
                .participants
                .keys()
                .filter(|id| *id != &user.id)
                .cloned()
                .collect())
        }
    };
    let peers = match joined {
        Ok(peers) => peers,
        Err(code) => return refuse(socket, code).await,
    };
    let ready = serde_json::json!({"type": "ready", "user_id": user.id, "peers": peers});
    if socket
        .send(WsMessage::Text(ready.to_string()))
        .await
        .is_ok()
    {
        run(
            &mut socket,
            &state,
            &user,
            &conversation_id,
            &token_hash,
            &revoked,
            &mut queue,
        )
        .await;
    }
    let _ = socket.send(WsMessage::Close(None)).await;
    let mut rooms = state.media_rooms.lock();
    if let Some(room) = rooms.get_mut(&conversation_id) {
        if room
            .participants
            .get(&user.id)
            .is_some_and(|participant| participant.connection == connection)
        {
            room.participants.remove(&user.id);
            announce(
                room,
                &user.id,
                serde_json::json!({"type": "peer_left", "user_id": user.id}),
            );
        }
        if room.participants.is_empty() {
            rooms.remove(&conversation_id);
        }
    }
}

async fn run(
    socket: &mut WebSocket,
    state: &AppState,
    user: &PublicUser,
    conversation_id: &str,
    token_hash: &str,
    revoked: &AtomicBool,
    queue: &mut mpsc::Receiver<WsMessage>,
) {
    let mut video_budget = FrameBudget::new();
    let mut audio_budget = FrameBudget::with_bytes(AUDIO_BYTE_RATE, AUDIO_BYTE_BURST);
    let mut recheck = tokio::time::interval(RECHECK_EVERY);
    recheck.tick().await;
    // Tunnels drop idle sockets; see `Keepalive`.
    let mut keepalive = tokio::time::interval(state.keepalive.interval);
    keepalive.tick().await;
    let mut last_heard = Instant::now();
    let mut prefix = Vec::with_capacity(1 + user.id.len());
    prefix.push(user.id.len().min(255) as u8);
    prefix.extend_from_slice(&user.id.as_bytes()[..user.id.len().min(255)]);
    loop {
        if revoked.load(Ordering::SeqCst) {
            return;
        }
        tokio::select! {
            _ = keepalive.tick() => {
                if last_heard.elapsed() >= state.keepalive.timeout
                    || socket.send(WsMessage::Ping(Vec::new())).await.is_err()
                {
                    return;
                }
            }
            _ = recheck.tick() => {
                if session_user(state, token_hash).await.is_none()
                    || !allowed(&state.db, conversation_id, &user.id).await
                {
                    return;
                }
            }
            outgoing = queue.recv() => match outgoing {
                Some(message) => {
                    if revoked.load(Ordering::SeqCst) || socket.send(message).await.is_err() {
                        return;
                    }
                }
                None => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(WsMessage::Pong(_))) => last_heard = Instant::now(),
                Some(Ok(WsMessage::Binary(payload))) => {
                    last_heard = Instant::now();
                    let budget = match payload.first() {
                        None => continue,
                        Some(&VIDEO_KIND) => &mut video_budget,
                        Some(_) => &mut audio_budget,
                    };
                    if !budget.admit(payload.len()) {
                        continue;
                    }
                    let epoch = state.media_rooms.epoch.load(Ordering::SeqCst);
                    let stale = state
                        .media_rooms
                        .lock()
                        .get(conversation_id)
                        .is_some_and(|room| room.checked_epoch != epoch);
                    if stale {
                        revalidate(state, conversation_id, epoch).await;
                    }
                    if revoked.load(Ordering::SeqCst) {
                        return;
                    }
                    let mut frame = Vec::with_capacity(prefix.len() + payload.len());
                    frame.extend_from_slice(&prefix);
                    frame.extend_from_slice(&payload);
                    let rooms = state.media_rooms.lock();
                    if let Some(room) = rooms.get(conversation_id) {
                        for (peer, participant) in &room.participants {
                            if peer != &user.id && !participant.revoked.load(Ordering::SeqCst) {
                                // Media is lossy: a slow receiver drops frames
                                // instead of slowing the room down.
                                let _ = participant.outgoing.try_send(WsMessage::Binary(frame.clone()));
                            }
                        }
                    }
                }
                Some(Ok(WsMessage::Ping(bytes))) => {
                    last_heard = Instant::now();
                    if socket.send(WsMessage::Pong(bytes)).await.is_err() {
                        return;
                    }
                }
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relay_caps_rooms_per_user_and_sockets_in_total() {
        let rooms = Rooms::default();
        let join = |conversation: &str, user: &str| {
            let (outgoing, queue) = mpsc::channel(1);
            std::mem::forget(queue);
            rooms
                .lock()
                .entry(conversation.into())
                .or_default()
                .participants
                .insert(
                    user.into(),
                    Participant {
                        connection: 1,
                        revoked: Arc::new(AtomicBool::new(false)),
                        outgoing,
                    },
                );
        };
        join("one", "alice");
        join("two", "alice");
        join("two", "bob");
        let check = |user: &str, room: &str, per_user, total| {
            over_capacity(&rooms.lock(), user, room, per_user, total)
        };
        assert_eq!(check("alice", "three", 2, 100), Some("too_many_rooms"));
        assert_eq!(
            check("alice", "one", 2, 100),
            None,
            "rejoining is no new room"
        );
        assert_eq!(check("bob", "three", 2, 100), None);
        assert_eq!(check("carol", "three", 2, 3), Some("relay_full"));
        assert_eq!(check("alice", "two", 2, 3), None);
        assert_eq!(check("carol", "three", 2, 4), None);
    }

    #[test]
    fn a_room_with_a_participant_counts_as_an_active_call() {
        let rooms = Rooms::default();
        assert!(!rooms.any_active());
        let (outgoing, _queue) = mpsc::channel(1);
        rooms
            .lock()
            .entry("conversation".into())
            .or_default()
            .participants
            .insert(
                "alice".into(),
                Participant {
                    connection: 1,
                    revoked: Arc::new(AtomicBool::new(false)),
                    outgoing,
                },
            );
        assert!(rooms.any_active());
        rooms.lock().clear();
        assert!(!rooms.any_active());
    }

    #[test]
    fn frame_budget_refuses_bursts_beyond_the_limits() {
        let mut budget = FrameBudget::new();
        let admitted = (0..1000).filter(|_| budget.admit(10)).count();
        assert!(
            admitted >= FRAME_BURST as usize && admitted < FRAME_BURST as usize + 10,
            "admitted {admitted}"
        );
        let mut budget = FrameBudget::new();
        let admitted = (0..100).filter(|_| budget.admit(MAX_FRAME_BYTES)).count();
        assert!(admitted <= (BYTE_BURST as usize / MAX_FRAME_BYTES) + 1);
    }
}
