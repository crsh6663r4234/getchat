use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};
use tauri::Emitter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(windows)]
fn pick_file() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let filter = "All files\0*.*\0\0".encode_utf16().collect::<Vec<_>>();
    let mut file_path = vec![0u16; 32_768];
    let mut dialog = OPENFILENAMEW::default();
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file_path.as_mut_ptr();
    dialog.nMaxFile = file_path.len() as u32;
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let end = file_path.iter().position(|value| *value == 0)?;
    Some(PathBuf::from(std::ffi::OsString::from_wide(
        &file_path[..end],
    )))
}

#[cfg(windows)]
fn save_file(suggested_name: &str) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let filter = "All files\0*.*\0\0".encode_utf16().collect::<Vec<_>>();
    let mut file_path = vec![0u16; 32_768];
    let suggested = suggested_name.encode_utf16().collect::<Vec<_>>();
    let copy_length = suggested.len().min(file_path.len() - 1);
    file_path[..copy_length].copy_from_slice(&suggested[..copy_length]);
    let mut dialog = OPENFILENAMEW::default();
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file_path.as_mut_ptr();
    dialog.nMaxFile = file_path.len() as u32;
    dialog.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let end = file_path.iter().position(|value| *value == 0)?;
    Some(PathBuf::from(std::ffi::OsString::from_wide(
        &file_path[..end],
    )))
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

const CHUNK_SIZE: u64 = 1024 * 1024;
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;
const GCM_TAG_BYTES: u64 = 16;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedAttachmentManifest {
    pub attachment_id: String,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub key: String,
    pub nonce_prefix: String,
    pub chunk_size: u64,
    pub chunk_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedAttachment {
    attachment_id: String,
}

struct UploadState {
    file: tokio::fs::File,
    cipher: Aes256Gcm,
    attachment_id: String,
    chunk_index: u64,
    chunk_count: u64,
    size_bytes: u64,
    chunk_size: u64,
    nonce_prefix: [u8; 8],
    app: tauri::AppHandle,
}

struct DownloadReader {
    response: reqwest::Response,
    current: bytes::Bytes,
    offset: usize,
}

impl DownloadReader {
    fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            current: bytes::Bytes::new(),
            offset: 0,
        }
    }

    async fn read_exact(&mut self, length: usize) -> Result<Vec<u8>, String> {
        let mut result = Vec::with_capacity(length);
        while result.len() < length {
            if self.offset == self.current.len() {
                self.current = self
                    .response
                    .chunk()
                    .await
                    .map_err(|_| "The encrypted attachment download was interrupted.".to_string())?
                    .ok_or_else(|| "The encrypted attachment is incomplete.".to_string())?;
                self.offset = 0;
            }
            let remaining = length - result.len();
            let available = self.current.len() - self.offset;
            let take = remaining.min(available);
            result.extend_from_slice(&self.current[self.offset..self.offset + take]);
            self.offset += take;
        }
        Ok(result)
    }

    async fn has_remaining(&mut self) -> Result<bool, String> {
        if self.offset < self.current.len() {
            return Ok(true);
        }
        self.current = self
            .response
            .chunk()
            .await
            .map_err(|_| "The encrypted attachment download was interrupted.".to_string())?
            .unwrap_or_default();
        self.offset = 0;
        Ok(!self.current.is_empty())
    }
}

fn chunk_count(size_bytes: u64, chunk_size: u64) -> Result<u64, String> {
    if size_bytes > MAX_FILE_SIZE || chunk_size != CHUNK_SIZE {
        return Err("This file cannot be encrypted with the current attachment settings.".into());
    }
    if size_bytes == 0 {
        return Ok(1);
    }
    size_bytes
        .checked_add(chunk_size - 1)
        .map(|size| size / chunk_size)
        .ok_or_else(|| "The file is too large.".into())
}

fn nonce_for_chunk(prefix: &[u8; 8], index: u64) -> Result<[u8; 12], String> {
    let index = u32::try_from(index).map_err(|_| "The file has too many chunks.".to_string())?;
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(prefix);
    nonce[8..].copy_from_slice(&index.to_be_bytes());
    Ok(nonce)
}

fn chunk_aad(attachment_id: &str, index: u64) -> Vec<u8> {
    format!("ghostchat-file-v1:{attachment_id}:{index}").into_bytes()
}

fn base_url(server_base: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(server_base.trim_end_matches('/'))
        .map_err(|_| "The selected server address is invalid.".to_string())?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("The selected server address is invalid.".into());
    }
    Ok(url)
}

fn safe_file_name(value: &str) -> String {
    let name = value.rsplit(['/', '\\']).next().unwrap_or("file");
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
    if clean.is_empty() || clean == ".." {
        "encrypted-file.bin".into()
    } else {
        clean.into()
    }
}

fn content_type_for_name(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "gif" => "image/gif",
        "heic" | "heif" => "image/heic",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "m4v" => "video/x-m4v",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp4" => "video/mp4",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "GhostChat could not prepare a secure file transfer.".to_string())
}

#[cfg(windows)]
pub async fn upload(
    app: tauri::AppHandle,
    conversation_id: String,
    server_base: String,
    token: String,
) -> Result<Option<EncryptedAttachmentManifest>, String> {
    let conversation_id = Uuid::parse_str(&conversation_id)
        .map_err(|_| "This conversation has an invalid ID.".to_string())?
        .to_string();
    let selected = tokio::task::spawn_blocking(pick_file)
        .await
        .map_err(|_| "The file picker could not be opened.".to_string())?;
    let Some(path) = selected else {
        return Ok(None);
    };
    let file_name = safe_file_name(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("encrypted-file.bin"),
    );
    // Measure the same open handle that will be streamed. Looking up the path
    // first permits a replacement between the size check and the open.
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| "GhostChat could not read the selected file.".to_string())?;
    let size_bytes = file
        .metadata()
        .await
        .map_err(|_| "GhostChat could not read the selected file.".to_string())?
        .len();
    let chunk_count = chunk_count(size_bytes, CHUNK_SIZE)?;
    let encrypted_size = size_bytes
        .checked_add(
            chunk_count
                .checked_mul(GCM_TAG_BYTES)
                .ok_or("The file is too large.")?,
        )
        .ok_or("The file is too large.")?;
    let attachment_id = Uuid::new_v4().to_string();
    let mut key = Zeroizing::new([0u8; 32]);
    let mut nonce_prefix = [0u8; 8];
    OsRng.fill_bytes(&mut *key);
    OsRng.fill_bytes(&mut nonce_prefix);
    let cipher = Aes256Gcm::new_from_slice(&*key)
        .map_err(|_| "GhostChat could not prepare file encryption.".to_string())?;
    let upload_state = UploadState {
        file,
        cipher,
        attachment_id: attachment_id.clone(),
        chunk_index: 0,
        chunk_count,
        size_bytes,
        chunk_size: CHUNK_SIZE,
        nonce_prefix,
        app: app.clone(),
    };
    let encrypted_stream = futures_util::stream::try_unfold(upload_state, |mut state| async move {
        if state.chunk_index >= state.chunk_count {
            let mut extra = [0u8; 1];
            if state.file.read(&mut extra).await? != 0 {
                return Err(io::Error::other("Selected file changed during encryption"));
            }
            return Ok::<_, io::Error>(None);
        }
        let index = state.chunk_index;
        let start = index * state.chunk_size;
        let plain_length = (state.size_bytes - start).min(state.chunk_size) as usize;
        let mut plaintext = Zeroizing::new(vec![0u8; plain_length]);
        state.file.read_exact(&mut plaintext).await?;
        let aad = chunk_aad(&state.attachment_id, index);
        let nonce = nonce_for_chunk(&state.nonce_prefix, index)
            .map_err(|_| io::Error::other("Invalid encrypted attachment chunk index"))?;
        let encrypted = state
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| io::Error::other("File encryption failed"))?;
        state.chunk_index += 1;
        let sent = (state.chunk_index * state.chunk_size).min(state.size_bytes);
        if state.chunk_index % 8 == 0 || state.chunk_index == state.chunk_count {
            let _ = state.app.emit(
                "ghostchat:encrypted-attachment-progress",
                serde_json::json!({
                    "direction": "upload", "attachmentId": state.attachment_id,
                    "sentBytes": sent, "totalBytes": state.size_bytes,
                }),
            );
        }
        Ok(Some((encrypted, state)))
    });
    let base = base_url(&server_base)?;
    let url = base
        .join(&format!(
            "/v1/conversations/{conversation_id}/encrypted-attachments?attachment_id={attachment_id}&plain_size={size_bytes}&chunk_size={CHUNK_SIZE}&chunk_count={chunk_count}"
        ))
        .map_err(|_| "The selected server address is invalid.".to_string())?;
    let response = client()?
        .post(url)
        .bearer_auth(&token)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, encrypted_size)
        .body(reqwest::Body::wrap_stream(encrypted_stream))
        .send()
        .await
        .map_err(|_| "GhostChat could not upload the encrypted file.".to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("Encrypted file upload failed ({status})."));
        return Err(message);
    }
    let created = response
        .json::<CreatedAttachment>()
        .await
        .map_err(|_| "The server returned an invalid encrypted file response.".to_string())?;
    if created.attachment_id != attachment_id {
        return Err("The server returned a different encrypted file ID.".into());
    }
    let manifest = EncryptedAttachmentManifest {
        attachment_id,
        file_name: file_name.clone(),
        content_type: content_type_for_name(&file_name).into(),
        size_bytes,
        key: URL_SAFE_NO_PAD.encode(&*key),
        nonce_prefix: URL_SAFE_NO_PAD.encode(nonce_prefix),
        chunk_size: CHUNK_SIZE,
        chunk_count,
    };
    let _ = app.emit(
        "ghostchat:encrypted-attachment-progress",
        serde_json::json!({
            "direction": "upload", "attachmentId": manifest.attachment_id,
            "sentBytes": size_bytes, "totalBytes": size_bytes,
        }),
    );
    Ok(Some(manifest))
}

#[cfg(not(windows))]
pub async fn upload(
    _app: tauri::AppHandle,
    _conversation_id: String,
    _server_base: String,
    _token: String,
) -> Result<Option<EncryptedAttachmentManifest>, String> {
    Err("Encrypted file transfers are currently available in the Windows app.".into())
}

#[cfg(windows)]
pub async fn download(
    app: tauri::AppHandle,
    server_base: String,
    token: String,
    manifest: EncryptedAttachmentManifest,
) -> Result<Option<String>, String> {
    let attachment_id = Uuid::parse_str(&manifest.attachment_id)
        .map_err(|_| "This encrypted attachment has an invalid ID.".to_string())?
        .to_string();
    let expected_chunks = chunk_count(manifest.size_bytes, manifest.chunk_size)?;
    if manifest.chunk_count != expected_chunks {
        return Err("This encrypted attachment has an invalid chunk manifest.".into());
    }
    let key_bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(&manifest.key)
            .map_err(|_| "This encrypted attachment has an invalid key.".to_string())?,
    );
    if key_bytes.len() != 32 {
        return Err("This encrypted attachment has an invalid key.".into());
    }
    let nonce_bytes = URL_SAFE_NO_PAD
        .decode(&manifest.nonce_prefix)
        .map_err(|_| "This encrypted attachment has an invalid nonce.".to_string())?;
    let nonce_prefix: [u8; 8] = nonce_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "This encrypted attachment has an invalid nonce.".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|_| "This encrypted attachment has an invalid key.".to_string())?;
    let file_name = safe_file_name(&manifest.file_name);
    let dialog_name = file_name.clone();
    let path = tokio::task::spawn_blocking(move || save_file(&dialog_name))
        .await
        .map_err(|_| "The save dialog could not be opened.".to_string())?;
    let Some(path) = path else { return Ok(None) };
    let base = base_url(&server_base)?;
    let url = base
        .join(&format!("/v1/attachments/{attachment_id}"))
        .map_err(|_| "The selected server address is invalid.".to_string())?;
    let response = client()?
        .get(url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|_| "GhostChat could not download the encrypted file.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Encrypted file download failed ({}).",
            response.status()
        ));
    }
    let encrypted_size = manifest
        .size_bytes
        .checked_add(
            manifest
                .chunk_count
                .checked_mul(GCM_TAG_BYTES)
                .ok_or("This encrypted attachment is too large.")?,
        )
        .ok_or("This encrypted attachment is too large.")?;
    if response
        .content_length()
        .is_some_and(|length| length != encrypted_size)
    {
        return Err("The encrypted attachment has an invalid size.".into());
    }
    let parent = path.parent().ok_or("Choose a valid destination folder.")?;
    let temp_path: PathBuf = parent.join(format!(".ghostchat-{}.part", Uuid::new_v4()));
    let mut reader = DownloadReader::new(response);
    let mut temp_created = false;
    let transfer: Result<(), String> = async {
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await
            .map_err(|_| "GhostChat could not create the destination file.".to_string())?;
        temp_created = true;
        for index in 0..manifest.chunk_count {
            let start = index * manifest.chunk_size;
            let plain_length = (manifest.size_bytes - start).min(manifest.chunk_size) as usize;
            let encrypted = reader
                .read_exact(plain_length + GCM_TAG_BYTES as usize)
                .await?;
            let nonce = nonce_for_chunk(&nonce_prefix, index)?;
            let aad = chunk_aad(&attachment_id, index);
            let plaintext = cipher
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &encrypted,
                        aad: &aad,
                    },
                )
                .map_err(|_| {
                    "This file failed its integrity check and was not saved.".to_string()
                })?;
            let plaintext = Zeroizing::new(plaintext);
            output
                .write_all(&plaintext)
                .await
                .map_err(|_| "GhostChat could not write the decrypted file.".to_string())?;
            if index % 8 == 7 || index + 1 == manifest.chunk_count {
                let _ = app.emit(
                    "ghostchat:encrypted-attachment-progress",
                    serde_json::json!({
                        "direction": "download", "attachmentId": attachment_id,
                        "sentBytes": start + plain_length as u64,
                        "totalBytes": manifest.size_bytes,
                    }),
                );
            }
        }
        if reader.has_remaining().await? {
            return Err("The encrypted attachment contains unexpected extra data.".into());
        }
        output
            .flush()
            .await
            .map_err(|_| "GhostChat could not finish writing the decrypted file.".to_string())?;
        output
            .sync_all()
            .await
            .map_err(|_| "GhostChat could not finish writing the decrypted file.".to_string())?;
        drop(output);
        let source = temp_path.clone();
        let destination = path.clone();
        tokio::task::spawn_blocking(move || replace_file(&source, &destination))
            .await
            .map_err(|_| "GhostChat could not move the decrypted file into place.".to_string())?
            .map_err(|_| "GhostChat could not move the decrypted file into place.".to_string())?;
        Ok(())
    }
    .await;
    if let Err(error) = transfer {
        if temp_created {
            let _ = tokio::fs::remove_file(&temp_path).await;
        }
        return Err(error);
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(not(windows))]
pub async fn download(
    _app: tauri::AppHandle,
    _server_base: String,
    _token: String,
    _manifest: EncryptedAttachmentManifest,
) -> Result<Option<String>, String> {
    Err("Encrypted file transfers are currently available in the Windows app.".into())
}
