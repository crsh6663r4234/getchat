//! Attachments: plain and encrypted uploads, download tickets, ranged downloads and the unclaimed-upload sweeps.

use super::*;

pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

pub(crate) const ENCRYPTED_ATTACHMENT_CHUNK_BYTES: u64 = 1024 * 1024;

pub(crate) const ATTACHMENT_DOWNLOAD_TTL_SECONDS: u64 = 30 * 60;

pub(crate) const ATTACHMENT_MARKER: &str = "file:";

#[derive(Deserialize)]
pub(crate) struct UploadAttachmentQuery {
    pub(crate) name: String,
    pub(crate) client_message_id: String,
}

#[derive(Deserialize)]
pub(crate) struct EncryptedAttachmentUploadQuery {
    pub(crate) attachment_id: String,
    pub(crate) plain_size: u64,
    pub(crate) chunk_size: u64,
    pub(crate) chunk_count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EncryptedAttachmentCreated {
    pub(crate) attachment_id: String,
}

pub(crate) async fn upload_encrypted_attachment(
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

pub(crate) fn safe_attachment_name(name: &str) -> String {
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

pub(crate) async fn upload_attachment(
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

pub(crate) async fn download_attachment(
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
pub(crate) struct AttachmentLink {
    pub(crate) url: String,
    pub(crate) filename: String,
    pub(crate) content_type: String,
}

pub(crate) async fn create_attachment_link(
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
pub(crate) struct AttachmentTicketQuery {
    pub(crate) ticket: String,
}

pub(crate) async fn download_attachment_ticket(
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

pub(crate) type AttachmentRow = (String, String, String, i64, Vec<u8>, i64);

pub(crate) async fn attachment_row(state: &AppState, id: &str) -> Result<AttachmentRow, ApiError> {
    sqlx::query_as::<_, AttachmentRow>("SELECT conversation_id,filename,content_type,size_bytes,file_data,stored_on_disk FROM attachments WHERE id=?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)
}

pub(crate) fn make_attachment_ticket(
    state: &AppState,
    attachment_id: &str,
    user_id: &str,
) -> String {
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

pub(crate) fn verify_attachment_ticket(
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

pub(crate) fn parse_attachment_range(
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

pub(crate) async fn attachment_response(
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

/// An encrypted upload no message has claimed within this time is abandoned.
pub(crate) const UNCLAIMED_UPLOAD_TTL_SECS: i64 = 24 * 60 * 60;

pub(crate) const UNCLAIMED_UPLOAD_SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// Deletes encrypted uploads that no message claimed (row and blob), once
/// they are older than a day as seen from `now` (Unix seconds). Only uploads
/// made after migration 0026 qualify: before it an upload could not be
/// linked, and an encrypted message the server cannot read may still point
/// to it. `installed_on` has whole seconds, so a one-second margin keeps an
/// upload from the second the migration ran out of the sweep. Without a
/// record for 0026 the comparison is NULL and nothing is deleted.
pub(crate) async fn sweep_unclaimed_encrypted_uploads(
    state: &AppState,
    now: i64,
) -> Result<u64, sqlx::Error> {
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
pub(crate) async fn sweep_stale_upload_temp_files(
    dir: &std::path::Path,
    now: i64,
) -> std::io::Result<u64> {
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

pub(crate) async fn run_unclaimed_upload_sweep(state: AppState) {
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
