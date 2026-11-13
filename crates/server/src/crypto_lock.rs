//! Group crypto roster locks and the encrypted group setup.

use super::*;

#[derive(Deserialize)]
pub(crate) struct LockGroupCryptoRosterRequest {
    pub(crate) member_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupCryptoLockStatus {
    pub(crate) locked: bool,
    pub(crate) locked_by: Option<String>,
    pub(crate) setup_complete: bool,
    /// Accounts deleted while members of this MLS-locked group, whose MLS
    /// leaves remaining clients may tolerate until they are removed.
    pub(crate) deleted_member_ids: Vec<String>,
    /// The same accounts with when they were deleted and when the removal
    /// commit for their leaf was accepted (null while it is still pending).
    pub(crate) deleted_members: Vec<DeletedCryptoMember>,
    /// The unfinished setup was started by an account that no longer exists;
    /// the group owner (or the oldest admin when there is no owner) may take
    /// it over through the lock route.
    pub(crate) locked_by_deleted_account: bool,
    /// Counts takeovers of an unfinished setup. A takeover is a reset, so a
    /// device holding the group of an older generation must drop it.
    pub(crate) setup_generation: i64,
    /// Counts the accepted membership commits (and setup takeovers). A
    /// membership commit names the value it was built on.
    pub(crate) commit_seq: i64,
    /// The server's clock, in the format of `deletedAt`. A client measures
    /// how long a deleted account's leaf has been waiting as the difference
    /// of the two, so its own clock does not matter.
    pub(crate) server_now: String,
}

pub(crate) async fn server_now(db: &SqlitePool) -> Result<String, ApiError> {
    sqlx::query_scalar::<_, String>("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')")
        .fetch_one(db)
        .await
        .map_err(|_| ApiError::internal())
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeletedCryptoMember {
    pub(crate) user_id: String,
    pub(crate) deleted_at: String,
    pub(crate) removal_committed_at: Option<String>,
}

pub(crate) async fn deleted_crypto_members(
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
pub(crate) async fn group_crypto_lock_status(
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

pub(crate) async fn deleted_crypto_member_ids(
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

pub(crate) async fn get_group_crypto_lock(
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

pub(crate) async fn lock_group_crypto_roster(
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

pub(crate) async fn complete_group_crypto_setup(
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
