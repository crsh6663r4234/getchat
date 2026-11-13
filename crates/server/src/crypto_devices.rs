//! Crypto devices and MLS key packages, and the shared crypto payload check.

use super::*;

pub(crate) const MAX_CRYPTO_PAYLOAD_BYTES: usize = 64 * 1024;

pub(crate) fn validate_crypto_payload(payload: &str) -> Result<(), ApiError> {
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

#[derive(Serialize, FromRow)]
pub(crate) struct CryptoDeviceRow {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) created_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) revoked_at: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct RegisterCryptoDevice {
    pub(crate) id: String,
    pub(crate) label: String,
}

pub(crate) fn normalize_device_id(id: &str) -> Result<String, ApiError> {
    Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| ApiError::bad("Device IDs must be UUIDs."))
}

pub(crate) fn validate_device_label(label: &str) -> Result<String, ApiError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 32 || label.chars().any(char::is_control) {
        return Err(ApiError::bad(
            "Device names must contain 1 to 32 characters.",
        ));
    }
    Ok(label.to_owned())
}

pub(crate) async fn list_crypto_devices(
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

pub(crate) async fn register_crypto_device(
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

pub(crate) async fn revoke_crypto_device(
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
pub(crate) struct UploadCryptoKeyPackage {
    pub(crate) package_ref: String,
    pub(crate) payload: String,
}

#[derive(Serialize, FromRow)]
pub(crate) struct CryptoKeyPackageAck {
    pub(crate) package_ref: String,
    pub(crate) created_at: String,
}

pub(crate) async fn upload_crypto_key_package(
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
pub(crate) struct ClaimedCryptoKeyPackage {
    pub(crate) package_ref: String,
    pub(crate) device_id: String,
    pub(crate) payload: String,
    pub(crate) created_at: String,
}

#[derive(Deserialize)]
pub(crate) struct ClaimCryptoKeyPackage {
    pub(crate) client_claim_id: String,
}

pub(crate) async fn claim_crypto_key_package(
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
