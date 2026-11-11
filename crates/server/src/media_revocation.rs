//! Durable removal RPC retries, including the screen-share room. Not a webhook.
use super::*;

pub(super) const JOIN_TTL: u64 = 30;
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(super) async fn enqueue(
    connection: &mut sqlx::SqliteConnection,
    conversation: &str,
    account: &str,
) -> Result<(), ApiError> {
    // Keep rescanning beyond all previously issued join tokens plus skew. A
    // successful RPC alone cannot prevent reuse of an unexpired signed token.
    for room in [
        conversation.to_owned(),
        format!("ghostshare-{conversation}"),
    ] {
        sqlx::query("INSERT INTO media_revocation_outbox(room,account_id,expires_at) VALUES(?,?,?) ON CONFLICT(room,account_id) DO UPDATE SET expires_at=MAX(expires_at,excluded.expires_at),next_attempt_at=0")
            .bind(room).bind(account).bind(now()+JOIN_TTL as i64+30)
            .execute(&mut *connection).await.map_err(|_| ApiError::internal())?;
    }
    Ok(())
}

pub(super) async fn admit(state: &AppState, conversation: &str) -> Result<(), ApiError> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM media_revocation_outbox WHERE room=? OR room=?)",
    )
    .bind(conversation)
    .bind(format!("ghostshare-{conversation}"))
    .fetch_one(&state.db)
    .await
    .map_err(|_| ApiError::internal())?;
    if pending != 0 {
        return Err(ApiError::unavailable(
            "Media membership is being secured. Try joining again shortly.",
        ));
    }
    Ok(())
}

pub(super) async fn run(state: AppState) {
    loop {
        if state.voice.livekit_url.is_some()
            && state.voice.livekit_api_key.is_some()
            && state.voice.livekit_api_secret.is_some()
        {
            if let Err(_) = reconcile(&state).await {
                tracing::warn!(
                    "Media revocation reconciliation failed; room admission remains blocked"
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn reconcile(state: &AppState) -> Result<(), sqlx::Error> {
    let jobs = sqlx::query_as::<_, (String,String,i64,i64)>("SELECT room,account_id,expires_at,attempts FROM media_revocation_outbox WHERE next_attempt_at<=? ORDER BY next_attempt_at LIMIT 8")
        .bind(now()).fetch_all(&state.db).await?;
    for (room, account, expires, attempts) in jobs {
        let success = remove_livekit_participant(state, &room, &account)
            .await
            .is_ok();
        if success && now() >= expires {
            sqlx::query("DELETE FROM media_revocation_outbox WHERE room=? AND account_id=? AND expires_at=?")
                .bind(&room).bind(&account).bind(expires).execute(&state.db).await?;
        } else {
            let delay = if success {
                2
            } else {
                (2_i64.saturating_pow(attempts.min(5) as u32)).min(30)
            };
            sqlx::query("UPDATE media_revocation_outbox SET attempts=MIN(attempts+1,1000000),next_attempt_at=? WHERE room=? AND account_id=? AND expires_at=?")
                .bind(now()+delay).bind(&room).bind(&account).bind(expires).execute(&state.db).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn queue_is_atomic_and_blocks_both_media_rooms_until_reconciled() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/0021_media_revocation_outbox.sql"
        ))
        .execute(&db)
        .await
        .unwrap();
        let mut tx = db.begin().await.unwrap();
        enqueue(&mut tx, "conversation", "account").await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_revocation_outbox")
                .fetch_one(&db)
                .await
                .unwrap(),
            0
        );
        let mut tx = db.begin().await.unwrap();
        enqueue(&mut tx, "conversation", "account").await.unwrap();
        tx.commit().await.unwrap();
        let state = build_state(db).await;
        assert!(admit(&state, "conversation").await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_revocation_outbox")
                .fetch_one(&state.db)
                .await
                .unwrap(),
            2
        );
    }
}
