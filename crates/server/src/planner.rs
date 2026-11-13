//! The group planner: encrypted polls and events.

use super::*;

pub(crate) async fn ensure_group_planner_member(
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
pub(crate) const PLANNER_ENCRYPTION_REQUIRED: &str = "planner_encryption_required";

/// Sent when ciphertext is posted to a group whose chat is not encrypted.
pub(crate) const PLANNER_NOT_ENCRYPTED: &str = "planner_not_encrypted";

/// Largest decoded ciphertext of a poll or event (its plaintext is at most
/// 8 KB on the client) and of a vote or RSVP.
pub(crate) const PLANNER_ITEM_CIPHERTEXT_LIMIT: usize = 8 * 1024 + 64;

pub(crate) const PLANNER_RESPONSE_CIPHERTEXT_LIMIT: usize = 1024;

/// "GP" 0x01, a 96-bit nonce and a 128-bit tag.
pub(crate) const PLANNER_CIPHERTEXT_MINIMUM: usize = 3 + 12 + 16;

/// The sealed form of a planner item or response of an encrypted group. The
/// server checks its shape and size only; it cannot read it.
#[derive(Deserialize, Default)]
pub(crate) struct EncryptedPlannerFields {
    #[serde(default)]
    pub(crate) ciphertext: Option<String>,
    #[serde(default)]
    pub(crate) epoch: Option<i64>,
    #[serde(default)]
    pub(crate) key_id: Option<String>,
}

pub(crate) struct EncryptedPlannerPayload {
    pub(crate) ciphertext: Vec<u8>,
    pub(crate) epoch: i64,
    pub(crate) key_id: String,
}

impl EncryptedPlannerFields {
    pub(crate) fn is_empty(&self) -> bool {
        self.ciphertext.is_none() && self.epoch.is_none() && self.key_id.is_none()
    }

    pub(crate) fn validated(
        &self,
        limit: usize,
    ) -> Result<Option<EncryptedPlannerPayload>, ApiError> {
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

pub(crate) fn planner_plaintext_refused() -> ApiError {
    ApiError::conflict(
        "This group's planner is end-to-end encrypted. Update GhostChat to add or answer polls and events here.",
    )
    .with_code(PLANNER_ENCRYPTION_REQUIRED)
}

pub(crate) fn planner_ciphertext_refused() -> ApiError {
    ApiError::bad("This group's planner is not encrypted.").with_code(PLANNER_NOT_ENCRYPTED)
}

pub(crate) async fn group_planner_is_encrypted(
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

pub(crate) fn serialize_planner_ciphertext<S: serde::Serializer>(
    value: &Option<Vec<u8>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(bytes) => serializer.serialize_some(&URL_SAFE_NO_PAD.encode(bytes)),
        None => serializer.serialize_none(),
    }
}

pub(crate) fn serialize_planner_response_ciphertext<S: serde::Serializer>(
    value: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&URL_SAFE_NO_PAD.encode(value))
}

/// A vote or RSVP of an encrypted group: the server knows who answered and
/// when, not what.
#[derive(Serialize, FromRow)]
pub(crate) struct PlannerResponseRow {
    pub(crate) user_id: String,
    #[serde(
        rename = "ciphertext",
        serialize_with = "serialize_planner_response_ciphertext"
    )]
    pub(crate) payload_ciphertext: Vec<u8>,
    #[serde(rename = "epoch")]
    pub(crate) payload_epoch: i64,
    #[serde(rename = "key_id")]
    pub(crate) payload_key_id: String,
}

pub(crate) fn planner_item_id(id: Option<&str>) -> Result<String, ApiError> {
    id.and_then(|id| Uuid::parse_str(id).ok())
        .map(|id| id.to_string())
        .ok_or_else(|| ApiError::bad("An encrypted planner item needs a valid id."))
}

#[derive(Deserialize)]
pub(crate) struct CreateGroupPollRequest {
    #[serde(default)]
    pub(crate) question: Option<String>,
    #[serde(default)]
    pub(crate) options: Option<Vec<String>>,
    pub(crate) closes_at: Option<String>,
    /// Chosen by the client for an encrypted poll: its ciphertext is bound
    /// to it.
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(flatten)]
    pub(crate) encrypted: EncryptedPlannerFields,
}

#[derive(Serialize, FromRow)]
pub(crate) struct GroupPollOption {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) position: i64,
    pub(crate) votes: i64,
}

#[derive(Serialize, FromRow)]
pub(crate) struct GroupPollRow {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    /// `None` once the creator deleted their account.
    pub(crate) creator_id: Option<String>,
    /// `None` for an encrypted poll: question and options are in its
    /// ciphertext.
    pub(crate) question: Option<String>,
    pub(crate) closes_at: Option<String>,
    pub(crate) closed_at: Option<String>,
    pub(crate) created_at: String,
    #[serde(rename = "ciphertext", serialize_with = "serialize_planner_ciphertext")]
    pub(crate) payload_ciphertext: Option<Vec<u8>>,
    #[serde(rename = "epoch")]
    pub(crate) payload_epoch: Option<i64>,
    #[serde(rename = "key_id")]
    pub(crate) payload_key_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct GroupPollDetail {
    #[serde(flatten)]
    pub(crate) poll: GroupPollRow,
    pub(crate) options: Vec<GroupPollOption>,
    pub(crate) my_option_id: Option<String>,
    /// Encrypted votes (of an encrypted poll, or of a poll stored before its
    /// group was encrypted), tallied by the clients.
    pub(crate) responses: Vec<PlannerResponseRow>,
}

pub(crate) async fn group_poll_details(
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

pub(crate) async fn list_group_polls(
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

pub(crate) async fn create_group_poll(
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
pub(crate) struct VoteGroupPollRequest {
    #[serde(default)]
    pub(crate) option_id: Option<String>,
    #[serde(flatten)]
    pub(crate) encrypted: EncryptedPlannerFields,
}

pub(crate) async fn vote_group_poll(
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
pub(crate) struct CreateGroupEventRequest {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) details: Option<String>,
    #[serde(default)]
    pub(crate) scheduled_at: Option<String>,
    /// Chosen by the client for an encrypted event: its ciphertext is bound
    /// to it.
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(flatten)]
    pub(crate) encrypted: EncryptedPlannerFields,
}

#[derive(Deserialize)]
pub(crate) struct GroupEventRsvpRequest {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(flatten)]
    pub(crate) encrypted: EncryptedPlannerFields,
}

#[derive(Serialize, FromRow)]
pub(crate) struct GroupEventRow {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    /// `None` once the creator deleted their account.
    pub(crate) creator_id: Option<String>,
    /// `None` for an encrypted event, like `details` and `scheduled_at`:
    /// they are in its ciphertext.
    pub(crate) title: Option<String>,
    pub(crate) details: Option<String>,
    pub(crate) scheduled_at: Option<String>,
    pub(crate) cancelled_at: Option<String>,
    pub(crate) created_at: String,
    pub(crate) going: i64,
    pub(crate) maybe: i64,
    pub(crate) declined: i64,
    pub(crate) my_status: Option<String>,
    #[serde(rename = "ciphertext", serialize_with = "serialize_planner_ciphertext")]
    pub(crate) payload_ciphertext: Option<Vec<u8>>,
    #[serde(rename = "epoch")]
    pub(crate) payload_epoch: Option<i64>,
    #[serde(rename = "key_id")]
    pub(crate) payload_key_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct GroupEventDetail {
    #[serde(flatten)]
    pub(crate) event: GroupEventRow,
    /// Encrypted RSVPs, counted by the clients (the counts above are the
    /// plaintext RSVPs stored before the group was encrypted).
    pub(crate) responses: Vec<PlannerResponseRow>,
}

pub(crate) async fn group_event_details(
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

pub(crate) async fn list_group_events(
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

pub(crate) async fn create_group_event(
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

pub(crate) async fn rsvp_group_event(
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
