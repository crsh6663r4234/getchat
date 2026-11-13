//! Group membership changes: adding, roles, leaving and removal, with the encrypted (MLS) commit sequence.

use super::*;

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct EncryptedGroupMembershipUpdate {
    pub(crate) operation_id: String,
    pub(crate) expected_member_ids: Vec<String>,
    /// Accounts the client says the commit adds or removes. Optional for
    /// older clients; when given for a removal it must be the route target.
    /// Not part of the idempotency hash (the route target already is).
    #[serde(default, skip_serializing)]
    pub(crate) member_ids: Vec<String>,
    /// The conversation's commit sequence this commit was built on. Not part
    /// of the idempotency hash, so a retry of an accepted operation is
    /// recognised whatever it carries here.
    #[serde(default, skip_serializing)]
    pub(crate) expected_commit_seq: Option<i64>,
    pub(crate) commit_payload: String,
    #[serde(default)]
    pub(crate) welcome_payload: String,
}

/// The server cannot read an MLS commit, so it cannot see which group state
/// (epoch) a commit was built on. `conversation_crypto_locks.commit_seq`
/// counts the commits accepted for a conversation; a membership commit must
/// name the value it was built on. Two different commits built on the same
/// state can therefore never both be accepted, which would fork the group.
pub(crate) fn stale_crypto_commit(missing: bool) -> ApiError {
    ApiError::conflict(if missing {
        "This encrypted membership change does not say which group state it was built on. Update GhostChat and try again."
    } else {
        "The encrypted group changed while this update was being prepared. Sync the group and try again."
    })
    .with_code("stale_commit_seq")
}

pub(crate) async fn require_crypto_commit_seq(
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
pub(crate) async fn advance_crypto_commit_seq(
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
pub(crate) struct EncryptedGroupLeaveRequest {
    pub(crate) operation_id: String,
    pub(crate) expected_member_ids: Vec<String>,
    pub(crate) proposal_payload: String,
}

#[derive(Deserialize)]
pub(crate) struct AddGroupMembersRequest {
    pub(crate) member_ids: Vec<String>,
    pub(crate) crypto_update: Option<EncryptedGroupMembershipUpdate>,
}

pub(crate) async fn add_group_members(
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
pub(crate) struct ChangeGroupRoleRequest {
    pub(crate) role: String,
}

pub(crate) async fn change_group_role(
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

pub(crate) async fn request_encrypted_group_leave(
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

pub(crate) async fn commit_encrypted_group_leave(
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
    state.media_rooms.authorization_changed();

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

pub(crate) async fn remove_encrypted_group_member(
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
    state.media_rooms.authorization_changed();

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

pub(crate) async fn remove_group_member(
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
    state.media_rooms.authorization_changed();
    if let Err(error) = remove_livekit_participant(&state, &id, &target_id).await {
        tracing::warn!(conversation_id = %id, account_id = %target_id, %error, "could not remove group member from LiveKit");
    }
    let _ = state.events.send(ServerEvent {
        recipients: old_recipients,
        event: serde_json::json!({"type":"conversation_updated","conversation_id":id}),
    });
    Ok(StatusCode::NO_CONTENT)
}
