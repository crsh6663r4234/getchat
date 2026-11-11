use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use openmls::framing::errors::{MessageDecryptionError, SecretTreeError};
use openmls::group::{ProcessMessageError, ValidationError};
use openmls::prelude::{
    BasicCredential, Ciphersuite, CredentialType, CredentialWithKey, GroupId, KeyPackage, MlsGroup,
    MlsGroupCreateConfig, MlsGroupJoinConfig, MlsMessageBodyIn, MlsMessageIn, MlsMessageOut,
    ProcessedMessageContent, Proposal, ProtocolVersion, Sender, StagedWelcome,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::RustCrypto;
use openmls_sqlx_storage::{Codec, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;
use rand::{rngs::OsRng, RngCore};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{error::Error, fmt, sync::OnceLock};
use tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const MLS_CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

static STORAGE_KEY: OnceLock<Zeroizing<[u8; 32]>> = OnceLock::new();
static CRYPTO_OPERATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// MLS group state is stored under the wire GroupId, which every member of a
// group shares, so two GhostChat accounts on one desktop profile that are in
// the same conversation would collide in the shared storage. The storage
// lookup key of a GroupId is therefore bound to the account the current
// crypto operation runs for. Operations hold CRYPTO_OPERATION_LOCK for their
// whole duration, so one account scope is active at a time.
static STORAGE_ACCOUNT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(test)]
static LEGACY_LAYOUT_ACCOUNTS: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

/// Test hook: store this account's groups like builds before account-bound
/// group storage did.
#[cfg(test)]
fn use_legacy_storage_layout(user_id: &str, legacy: bool) {
    let mut accounts = LEGACY_LAYOUT_ACCOUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let accounts = accounts.get_or_insert_with(Default::default);
    if legacy {
        accounts.insert(user_id.to_owned());
    } else {
        accounts.remove(user_id);
    }
}

struct StorageAccountScope(Option<String>);

impl StorageAccountScope {
    fn set(account: Option<String>) -> Self {
        let mut current = STORAGE_ACCOUNT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self(std::mem::replace(&mut *current, account))
    }

    fn enter(user_id: &str) -> Self {
        #[cfg(test)]
        if LEGACY_LAYOUT_ACCOUNTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|accounts| accounts.contains(user_id))
        {
            return Self::set(None);
        }
        Self::set(Some(user_id.to_owned()))
    }

    /// The account-less layout of earlier builds, for adopting their groups.
    fn legacy() -> Self {
        Self::set(None)
    }
}

impl Drop for StorageAccountScope {
    fn drop(&mut self) {
        *STORAGE_ACCOUNT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.0.take();
    }
}

fn current_storage_account() -> Option<String> {
    STORAGE_ACCOUNT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}
/// Database files whose HashReference migration completed cleanly in this
/// process. Legacy rows are only written by older builds, so one clean pass
/// per file per process is enough; a failed pass is not recorded.
static MIGRATED_DATABASES: std::sync::Mutex<Option<std::collections::HashSet<std::path::PathBuf>>> =
    std::sync::Mutex::new(None);

/// The file a pool's migration result is remembered under; `None` for an
/// in-memory database, which is always scanned.
fn migration_marker(db: &SqlitePool) -> Option<std::path::PathBuf> {
    let options = db.connect_options();
    let path = options.get_filename();
    let text = path.to_string_lossy();
    (!text.is_empty() && !text.starts_with(":memory:")).then(|| path.to_path_buf())
}

fn install_storage_key(key: Zeroizing<[u8; 32]>) -> Result<(), String> {
    match STORAGE_KEY.set(key) {
        Ok(()) => Ok(()),
        Err(key) if STORAGE_KEY.get().is_some_and(|stored| &**stored == &*key) => Ok(()),
        Err(_) => Err("The local encryption key changed while GhostChat was running.".into()),
    }
}

fn get_storage_key() -> Result<Zeroizing<[u8; 32]>, String> {
    #[cfg(test)]
    {
        Ok(Zeroizing::new([0xA7; 32]))
    }
    #[cfg(not(test))]
    {
        crate::windows_credential::get_or_create_crypto_storage_key()
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePreparation {
    pub device_id: String,
    pub label: String,
    pub package_ref: String,
    pub payload: String,
    pub identity_fingerprint: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyPackageInspection {
    pub identity_fingerprint: String,
}

#[derive(serde::Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DirectGroupStart {
    pub welcome_id: String,
    pub welcome_payload: String,
    #[serde(default)]
    pub welcome_recipients: Vec<String>,
    pub peer_identity_fingerprint: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPeerKeyPackage {
    pub user_id: String,
    pub device_id: String,
    pub payload: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupMembershipUpdate {
    pub operation_id: String,
    #[serde(default = "default_membership_action")]
    pub action: String,
    pub member_ids: Vec<String>,
    pub expected_member_ids: Vec<String>,
    pub commit_payload: String,
    pub welcome_payload: String,
    /// The server's commit sequence this commit was built on (the client's
    /// synced value when it was staged). It is saved with the staged commit
    /// and sent with every attempt, so the server can refuse a commit built
    /// on a group state it has already moved past. `None` for a server
    /// without the sequence and for updates staged by an earlier version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_commit_seq: Option<i64>,
}

fn checked_commit_seq(expected_commit_seq: Option<i64>) -> Result<Option<i64>, String> {
    match expected_commit_seq {
        Some(value) if value < 0 => Err("The encrypted group commit sequence is invalid.".into()),
        other => Ok(other),
    }
}

fn default_membership_action() -> String {
    "add".into()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedApplicationMessage {
    pub payload: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CryptoEventResult {
    pub kind: String,
    pub content: Option<String>,
    pub peer_identity_fingerprint: Option<String>,
}

async fn prepare_crypto_storage(db: &SqlitePool) -> Result<(), String> {
    install_storage_key(get_storage_key()?)?;
    if migration_marker(db).is_some_and(|path| startup_copy_failed(&path)) {
        return Err(LOCAL_BACKUP_FAILED.into());
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_identities (\
            user_id TEXT PRIMARY KEY NOT NULL,\
            device_id TEXT NOT NULL UNIQUE,\
            signing_public_key BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its local encryption database.".to_string())?;
    prepare_text_cache(db).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_application_outbox (\
            client_message_id TEXT PRIMARY KEY NOT NULL,\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            encrypted_payload BLOB NOT NULL\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its encrypted application outbox.".to_string())?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_membership_outbox (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            payload BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),\
            PRIMARY KEY(user_id,conversation_id)\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its encrypted membership outbox.".to_string())?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_event_cursors (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            sequence INTEGER NOT NULL CHECK(sequence >= 0),\
            PRIMARY KEY(user_id,conversation_id)\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its encrypted event cursor storage.".to_string())?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_bootstrap (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            payload BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),\
            PRIMARY KEY(user_id,conversation_id)\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its pending encrypted setup storage.".to_string())?;

    // The server's setup generation this account's local group state of a
    // conversation belongs to (no row: generation 0).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_setup_generations (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            generation INTEGER NOT NULL CHECK(generation >= 0),\
            PRIMARY KEY(user_id,conversation_id)\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its encrypted setup storage.".to_string())?;

    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection)
        .run_migrations()
        .map_err(|_| "GhostChat could not initialize its encrypted MLS storage.".to_string())?;
    drop(connection);
    migrate_hash_reference_keys(db).await
}

/// Shown when the one-time safety copy of the local database cannot be
/// written. Encrypted storage stays untouched until a restart succeeds.
pub const LOCAL_BACKUP_FAILED: &str = "Encrypted chats are paused because GhostChat could not write a safety copy of its local data before upgrading it. Free some disk space, then restart GhostChat.";

/// Database files whose startup copy failed in this process: their
/// encrypted storage is not prepared (and so not rewritten) until a restart.
static STARTUP_COPY_FAILED: std::sync::Mutex<
    Option<std::collections::HashSet<std::path::PathBuf>>,
> = std::sync::Mutex::new(None);

fn startup_copy_failed(path: &std::path::Path) -> bool {
    STARTUP_COPY_FAILED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .is_some_and(|paths| paths.contains(path))
}

fn account_binding_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".pre-account-binding.bak");
    std::path::PathBuf::from(name)
}

/// `VACUUM INTO` a `.tmp` file next to `backup`, renamed once complete: a
/// consistent snapshot of the WAL database as this connection sees it.
async fn vacuum_copy(
    connection: &mut sqlx::SqliteConnection,
    backup: &std::path::Path,
) -> Result<(), String> {
    let mut partial = backup.as_os_str().to_owned();
    partial.push(".tmp");
    let partial = std::path::PathBuf::from(partial);
    if partial.exists() {
        std::fs::remove_file(&partial).map_err(|_| LOCAL_BACKUP_FAILED.to_string())?;
    }
    sqlx::query("VACUUM INTO ?")
        .bind(partial.to_string_lossy().into_owned())
        .execute(&mut *connection)
        .await
        .map_err(|_| LOCAL_BACKUP_FAILED.to_string())?;
    std::fs::rename(&partial, backup).map_err(|_| LOCAL_BACKUP_FAILED.to_string())
}

/// Whether the text cache still has the layout of builds before the account
/// binding (no table yet is a fresh profile, not an old one).
async fn text_cache_needs_rebuild(
    connection: &mut sqlx::SqliteConnection,
) -> Result<bool, sqlx::Error> {
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('ghostchat_crypto_text_cache')",
    )
    .fetch_all(&mut *connection)
    .await?;
    Ok(!columns.is_empty() && !columns.iter().any(|name| name == "sender_id"))
}

/// Opens the app's local database, which the built-in server and the MLS
/// store share. A profile an earlier build wrote is copied once to
/// `<database>.pre-account-binding.bak` before anything opens or migrates
/// it, so this copy and the server's `pre-<version>.bak` backup both hold the
/// state before the upgrade. If the copy fails the app still starts, but its
/// encrypted storage is left untouched and reports LOCAL_BACKUP_FAILED.
pub async fn open_local_database(path: &str) -> Result<SqlitePool, LocalDatabaseError> {
    let file = std::path::Path::new(path);
    if file.exists() && !account_binding_backup_path(file).exists() {
        match copy_profile_before_opening(file).await {
            Ok(Some(report)) => {
                eprintln!("{}", report.describe(&account_binding_backup_path(file)))
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "GhostChat could not copy its local database before upgrading it: {error}"
                );
                STARTUP_COPY_FAILED
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get_or_insert_with(Default::default)
                    .insert(file.to_path_buf());
            }
        }
    }
    ghostchat_server::open_database(path)
        .await
        .map_err(|error| {
            let text = format!("Could not open GhostChat's local database at {path}: {error}");
            if ghostchat_server::is_pre_upgrade_backup_error(&error) {
                LocalDatabaseError::BackupFailed(text)
            } else {
                LocalDatabaseError::Other(text)
            }
        })
}

/// Why the local database could not be opened at startup.
#[derive(Debug)]
pub enum LocalDatabaseError {
    /// The built-in server's pre-upgrade backup could not be written, so it
    /// migrated nothing.
    BackupFailed(String),
    Other(String),
}

impl std::fmt::Display for LocalDatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackupFailed(text) | Self::Other(text) => f.write_str(text),
        }
    }
}

/// Title and text of the startup dialog for `LocalDatabaseError::BackupFailed`,
/// in Turkish when the first preferred system language is Turkish. (The
/// language chosen in the app lives in the WebView's storage, which does not
/// exist yet at this point.)
pub fn startup_backup_failure_dialog(languages: &[String]) -> (&'static str, &'static str) {
    let turkish = languages
        .first()
        .is_some_and(|language| language.to_ascii_lowercase().starts_with("tr"));
    if turkish {
        (
            "GhostChat başlatılamadı",
            "GhostChat, yükseltmeden önce yerel verilerinin güvenlik kopyasını yazamadı. Hiçbir şey değiştirilmedi. Diskte yer aç, ardından GhostChat’i yeniden başlat.",
        )
    } else {
        (
            "GhostChat could not start",
            "GhostChat could not write a safety copy of its local data before upgrading it. Nothing was changed. Free some disk space, then start GhostChat again.",
        )
    }
}

/// What the startup copy wrote. It runs before the window can paint, so
/// a large profile shows as a slow start; the log says why.
struct StartupCopy {
    bytes: u64,
    elapsed: std::time::Duration,
}

impl StartupCopy {
    fn describe(&self, backup: &std::path::Path) -> String {
        format!(
            "GhostChat copied its local database ({:.1} MB) to {} before upgrading it, in {:.1} s",
            self.bytes as f64 / (1024.0 * 1024.0),
            backup.display(),
            self.elapsed.as_secs_f64()
        )
    }
}

/// Copies an earlier build's profile; `None` when the profile needs no copy.
async fn copy_profile_before_opening(
    file: &std::path::Path,
) -> Result<Option<StartupCopy>, String> {
    use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Connection};
    let started = std::time::Instant::now();
    let mut connection = SqliteConnectOptions::new()
        .filename(file)
        .connect()
        .await
        .map_err(|error| error.to_string())?;
    let backup = account_binding_backup_path(file);
    let result = match text_cache_needs_rebuild(&mut connection).await {
        Ok(true) => vacuum_copy(&mut connection, &backup).await.map(|()| {
            Some(StartupCopy {
                bytes: std::fs::metadata(&backup).map_or(0, |metadata| metadata.len()),
                elapsed: started.elapsed(),
            })
        }),
        Ok(false) => Ok(None),
        Err(error) => Err(error.to_string()),
    };
    let _ = connection.close().await;
    result
}

/// Copies the local database once, to `<database>.pre-account-binding.bak`,
/// before this build first rewrites data an earlier build cannot read again
/// (the text-cache rebuild and the adoption of account-less MLS groups).
/// Normally `open_local_database` took the copy at startup; this covers a
/// profile it did not recognize as old (a legacy group with a current cache).
/// An existing copy is kept, an in-memory database is skipped, and a failed
/// copy is an error so the caller changes nothing. The rows keep the field
/// encryption of the original (same storage key), so the copy needs no
/// protection beyond the original's directory.
async fn copy_before_account_binding(db: &SqlitePool) -> Result<(), String> {
    let Some(path) = migration_marker(db) else {
        return Ok(());
    };
    if startup_copy_failed(&path) {
        return Err(LOCAL_BACKUP_FAILED.into());
    }
    let backup = account_binding_backup_path(&path);
    if backup.exists() {
        return Ok(());
    }
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| LOCAL_BACKUP_FAILED.to_string())?;
    vacuum_copy(&mut connection, &backup).await
}

/// The local plaintext cache is keyed like the server's message uniqueness:
/// (account, conversation, sender, client message id). A sender-chosen id can
/// therefore never answer for another sender's, conversation's or account's
/// message. Rows from the old table, keyed by client message id only, are
/// kept as legacy rows (empty account and sender) so earlier history stays
/// readable; they are only read for their own conversation, and a row is
/// moved to its sender once it answers for that sender (`claim_legacy_text`).
async fn prepare_text_cache(db: &SqlitePool) -> Result<(), String> {
    let error = || "GhostChat could not prepare its encrypted message cache.".to_string();
    let mut connection = db.acquire().await.map_err(|_| error())?;
    let old_layout = text_cache_needs_rebuild(&mut connection)
        .await
        .map_err(|_| error())?;
    drop(connection);
    if old_layout {
        copy_before_account_binding(db).await?;
    }
    let mut transaction = db.begin().await.map_err(|_| error())?;
    let legacy_columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('ghostchat_crypto_text_cache')",
    )
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| error())?;
    let needs_rebuild =
        !legacy_columns.is_empty() && !legacy_columns.iter().any(|name| name == "sender_id");
    if needs_rebuild {
        sqlx::query(
            "ALTER TABLE ghostchat_crypto_text_cache RENAME TO ghostchat_crypto_text_cache_legacy",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| error())?;
    }
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_text_cache (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            sender_id TEXT NOT NULL,\
            client_message_id TEXT NOT NULL,\
            encrypted_content BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),\
            PRIMARY KEY(user_id,conversation_id,sender_id,client_message_id)\
        )",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|_| error())?;
    // History rows this device can never decrypt (their epoch is behind the
    // group's), so the client stops retrying them on every sync. The epoch
    // comes from the row's unauthenticated header, so a marker holds the
    // SHA-256 of the payload it was written for and blocks only that payload.
    // Markers from before the hash have an empty one (see
    // `get_cached_text_state`).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_undecryptable_rows (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            sender_id TEXT NOT NULL,\
            client_message_id TEXT NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),\
            payload_hash BLOB NOT NULL DEFAULT X'',\
            PRIMARY KEY(user_id,conversation_id,sender_id,client_message_id)\
        )",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|_| error())?;
    let marker_has_hash = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('ghostchat_crypto_undecryptable_rows') WHERE name='payload_hash')",
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| error())?;
    if marker_has_hash == 0 {
        sqlx::query(
            "ALTER TABLE ghostchat_crypto_undecryptable_rows ADD COLUMN payload_hash BLOB NOT NULL DEFAULT X''",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| error())?;
    }
    if needs_rebuild {
        sqlx::query(
            "INSERT OR IGNORE INTO ghostchat_crypto_text_cache(user_id,conversation_id,sender_id,client_message_id,encrypted_content,created_at) \
             SELECT '',conversation_id,'',client_message_id,encrypted_content,created_at FROM ghostchat_crypto_text_cache_legacy",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| error())?;
        sqlx::query("DROP TABLE ghostchat_crypto_text_cache_legacy")
            .execute(&mut *transaction)
            .await
            .map_err(|_| error())?;
    }
    transaction.commit().await.map_err(|_| error())
}

/// Reads a cached plaintext for exactly this account, conversation and
/// sender. A legacy row of the same conversation (written before the cache
/// was bound to a sender) answers here only for the account's own messages:
/// nobody else can send a row as this account. Another sender's legacy row
/// answers only through `legacy_text_row` once its MLS decode has failed,
/// so a member cannot reuse an old client id to show another sender's text.
async fn cached_text_row<'e, E>(
    executor: E,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: &str,
) -> Result<Option<Vec<u8>>, String>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT encrypted_content FROM ghostchat_crypto_text_cache \
         WHERE conversation_id=? AND client_message_id=? AND ((user_id=? AND sender_id=?) OR (user_id='' AND sender_id='' AND ?)) \
         ORDER BY user_id='' LIMIT 1",
    )
    .bind(conversation_id)
    .bind(client_message_id)
    .bind(user_id)
    .bind(sender_id)
    .bind(user_id == sender_id)
    .fetch_optional(executor)
    .await
    .map_err(|_| "GhostChat could not load the encrypted message cache.".to_string())
}

/// A legacy cache row of this conversation and client id, for a history row
/// of another sender. It is only used when MLS cannot decode that row (its
/// secrets are spent or its epoch is gone), see `claim_legacy_text`.
async fn legacy_text_row<'e, E>(
    executor: E,
    conversation_id: &str,
    client_message_id: &str,
) -> Result<Option<Vec<u8>>, String>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT encrypted_content FROM ghostchat_crypto_text_cache \
         WHERE user_id='' AND sender_id='' AND conversation_id=? AND client_message_id=?",
    )
    .bind(conversation_id)
    .bind(client_message_id)
    .fetch_optional(executor)
    .await
    .map_err(|_| "GhostChat could not load the encrypted message cache.".to_string())
}

/// Answers a history row that MLS cannot decode from its legacy cache row and
/// binds that row to this account and sender, so it never answers for another
/// sender's message with the same client id. Nothing is written on a dry run.
async fn claim_legacy_text(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: &str,
    encrypted_content: Vec<u8>,
    dry_run: bool,
) -> Result<CryptoEventResult, String> {
    let error = || "GhostChat could not save the encrypted local message cache.".to_string();
    let content: String = EncryptedJsonCodec::from_slice(&encrypted_content)
        .map_err(|_| "The local encrypted message cache could not be decrypted.".to_string())?;
    if !dry_run {
        let mut transaction = db.begin().await.map_err(|_| error())?;
        sqlx::query("INSERT OR IGNORE INTO ghostchat_crypto_text_cache(user_id,conversation_id,sender_id,client_message_id,encrypted_content) VALUES(?,?,?,?,?)")
            .bind(user_id)
            .bind(conversation_id)
            .bind(sender_id)
            .bind(client_message_id)
            .bind(&encrypted_content)
            .execute(&mut *transaction)
            .await
            .map_err(|_| error())?;
        sqlx::query("DELETE FROM ghostchat_crypto_text_cache WHERE user_id='' AND sender_id='' AND conversation_id=? AND client_message_id=?")
            .bind(conversation_id)
            .bind(client_message_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| error())?;
        transaction.commit().await.map_err(|_| error())?;
    }
    Ok(CryptoEventResult {
        kind: "application".into(),
        content: Some(content),
        peer_identity_fingerprint: None,
    })
}

fn normalized_uuid(value: &str, label: &str) -> Result<String, String> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| format!("The {label} ID is invalid."))
}

pub async fn get_crypto_event_cursor(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<i64>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    sqlx::query_scalar::<_, i64>(
        "SELECT sequence FROM ghostchat_crypto_event_cursors WHERE user_id=? AND conversation_id=?",
    )
    .bind(user_id)
    .bind(conversation_id)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not read the local encrypted event cursor.".to_string())
}

pub async fn advance_crypto_event_cursor(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sequence: i64,
) -> Result<i64, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    if sequence < 0 {
        return Err("The encrypted event sequence is invalid.".into());
    }
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not advance the local encrypted event cursor.".to_string())?;
    sqlx::query(
        "INSERT INTO ghostchat_crypto_event_cursors(user_id,conversation_id,sequence) VALUES(?,?,?) \
         ON CONFLICT(user_id,conversation_id) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(sequence)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the local encrypted event cursor.".to_string())?;
    let saved_sequence = sqlx::query_scalar::<_, i64>(
        "SELECT sequence FROM ghostchat_crypto_event_cursors WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not read the saved encrypted event cursor.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the local encrypted event cursor.".to_string())?;
    Ok(saved_sequence)
}

fn account_id_from_credential(content: &[u8]) -> Option<String> {
    let identity = std::str::from_utf8(content).ok()?;
    let mut parts = identity.split(':');
    if parts.next()? != "ghostchat" || parts.next()? != "v1" {
        return None;
    }
    let user_id = Uuid::parse_str(parts.next()?).ok()?.to_string();
    Uuid::parse_str(parts.next()?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(user_id)
}

fn group_account_identities(
    group: &MlsGroup,
) -> Option<std::collections::HashMap<String, (Vec<u8>, Vec<u8>)>> {
    let mut identities = std::collections::HashMap::new();
    for member in group.members() {
        if member.credential.credential_type() != CredentialType::Basic {
            return None;
        }
        let user_id = account_id_from_credential(member.credential.serialized_content())?;
        if identities
            .insert(
                user_id,
                (
                    member.credential.serialized_content().to_vec(),
                    member.signature_key.as_slice().to_vec(),
                ),
            )
            .is_some()
        {
            return None;
        }
    }
    Some(identities)
}

/// The MLS roster must contain every server member, and any extra leaf must
/// belong to an account the server recorded as deleted from this group.
fn roster_matches_allowing_deleted(
    actual_member_ids: &std::collections::HashSet<String>,
    expected_member_ids: &std::collections::HashSet<String>,
    deleted_member_ids: &std::collections::HashSet<String>,
) -> bool {
    expected_member_ids
        .iter()
        .all(|user_id| actual_member_ids.contains(user_id))
        && actual_member_ids.iter().all(|user_id| {
            expected_member_ids.contains(user_id) || deleted_member_ids.contains(user_id)
        })
}

fn group_roster_matches(
    group: &MlsGroup,
    expected_member_ids: &std::collections::HashSet<String>,
    deleted_member_ids: &std::collections::HashSet<String>,
    own_identity: &str,
) -> bool {
    let Some(identities) = group_account_identities(group) else {
        return false;
    };
    let actual_member_ids = identities.keys().cloned().collect();
    roster_matches_allowing_deleted(&actual_member_ids, expected_member_ids, deleted_member_ids)
        && identities
            .values()
            .any(|(credential, _)| credential == own_identity.as_bytes())
}

fn normalized_deleted_member_ids(
    deleted_member_ids: &[String],
) -> Result<std::collections::HashSet<String>, String> {
    if deleted_member_ids.len() > 64 {
        return Err("The deleted group member list is invalid.".into());
    }
    deleted_member_ids
        .iter()
        .map(|member_id| normalized_uuid(member_id, "deleted group member"))
        .collect()
}

fn existing_group_identities_unchanged(
    group: &MlsGroup,
    existing: &std::collections::HashMap<String, (Vec<u8>, Vec<u8>)>,
) -> bool {
    let Some(updated) = group_account_identities(group) else {
        return false;
    };
    // A member that left or was removed is absent from the new roster; that is
    // checked against the server roster separately. A member that is still
    // present must keep exactly the credential and signature key it had.
    existing.iter().all(|(user_id, identity)| {
        updated
            .get(user_id)
            .is_none_or(|updated_identity| updated_identity == identity)
    })
}

fn group_id_for_conversation(conversation_id: &str) -> GroupId {
    GroupId::from_slice(conversation_id.as_bytes())
}

fn group_create_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .use_ratchet_tree_extension(true)
        .build()
}

fn is_mls_storage_lookup_key(type_name: &str) -> bool {
    // openmls_sqlx_storage 0.3.0 routes both serialized Key values and Entity
    // values through Codec::to_vec, so the type names are the only way to keep
    // SQL lookup keys stable while leaving MLS entities on randomized AEAD.
    // Recheck this list whenever the pinned OpenMLS storage dependency changes.
    matches!(
        type_name,
        "openmls_basic_credential::StorageId"
            | "openmls::group::GroupId"
            | "openmls::group::GroupEpoch"
    )
}

fn is_decodable_mls_storage_lookup_key(type_name: &str) -> bool {
    // HashReference is both a lookup key (KeyPackage refs, proposal deletion)
    // and a value the storage crate decodes again (openmls_proposal.proposal_ref
    // is read back when a group with queued proposals is loaded). A one-way
    // digest made every such group unloadable, so it uses deterministic AEAD.
    type_name == "openmls::ciphersuite::hash_ref::HashReference"
}

const LOOKUP_KEY_V1_PREFIX: &[u8; 4] = b"GCK\x01";
const DECODABLE_LOOKUP_KEY_PREFIX: &[u8; 4] = b"GCK\x02";

fn lookup_key_v1(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoCodecError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
    mac.update(b"GhostChat MLS storage lookup key v1\0");
    mac.update(plaintext);
    let digest = mac.finalize().into_bytes();
    let mut encoded = Vec::with_capacity(LOOKUP_KEY_V1_PREFIX.len() + digest.len());
    encoded.extend_from_slice(LOOKUP_KEY_V1_PREFIX);
    encoded.extend_from_slice(&digest);
    Ok(encoded)
}

/// The storage lookup key of a GroupId for one account (see STORAGE_ACCOUNT).
fn account_group_lookup_key(
    key: &[u8; 32],
    account: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoCodecError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
    mac.update(b"GhostChat MLS account group lookup key v1\0");
    mac.update(account.as_bytes());
    mac.update(b"\0");
    mac.update(plaintext);
    let digest = mac.finalize().into_bytes();
    let mut encoded = Vec::with_capacity(LOOKUP_KEY_V1_PREFIX.len() + digest.len());
    encoded.extend_from_slice(LOOKUP_KEY_V1_PREFIX);
    encoded.extend_from_slice(&digest);
    Ok(encoded)
}

/// Moves a group an earlier build stored without an account binding to this
/// account's storage key, if this account owns it (its own leaf is this
/// account's device). A legacy group of another account on the profile is
/// left alone. Runs inside the caller's CRYPTO_OPERATION_LOCK.
async fn adopt_legacy_group(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<(), String> {
    let error = || "GhostChat could not load the local MLS group state.".to_string();
    let key = get_storage_key()?;
    let group_id = group_id_for_conversation(conversation_id);
    let plaintext = Zeroizing::new(serde_json::to_vec(&group_id).map_err(|_| error())?);
    let legacy = lookup_key_v1(&key, &plaintext).map_err(|_| error())?;
    let scoped = account_group_lookup_key(&key, user_id, &plaintext).map_err(|_| error())?;
    let mut transaction = db.begin().await.map_err(|_| error())?;
    let (legacy_exists, scoped_exists) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT EXISTS(SELECT 1 FROM openmls_group_data WHERE group_id=?),EXISTS(SELECT 1 FROM openmls_group_data WHERE group_id=?)",
    )
    .bind(&legacy)
    .bind(&scoped)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| error())?;
    if !legacy_exists || scoped_exists {
        return Ok(());
    }
    let Some(device_id) = sqlx::query_scalar::<_, String>(
        "SELECT device_id FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| error())?
    else {
        return Ok(());
    };
    let own_identity = format!("ghostchat:v1:{user_id}:{device_id}");
    let owned = {
        let _legacy = StorageAccountScope::legacy();
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        MlsGroup::load(&storage, &group_id)
            .ok()
            .flatten()
            .and_then(|group| {
                group
                    .own_leaf_node()
                    .map(|leaf| leaf.credential().serialized_content() == own_identity.as_bytes())
            })
            .unwrap_or(false)
    };
    if !owned {
        return Ok(());
    }
    // VACUUM INTO cannot run inside a transaction: end this read-only one,
    // take the copy, then check again under the same operation lock.
    drop(transaction);
    copy_before_account_binding(db).await?;
    let mut transaction = db.begin().await.map_err(|_| error())?;
    let (legacy_exists, scoped_exists) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT EXISTS(SELECT 1 FROM openmls_group_data WHERE group_id=?),EXISTS(SELECT 1 FROM openmls_group_data WHERE group_id=?)",
    )
    .bind(&legacy)
    .bind(&scoped)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| error())?;
    if !legacy_exists || scoped_exists {
        return Ok(());
    }
    for table in [
        "openmls_group_data",
        "openmls_proposal",
        "openmls_own_leaf_node",
        "openmls_epoch_key_pairs",
    ] {
        sqlx::query(&format!("UPDATE {table} SET group_id=? WHERE group_id=?"))
            .bind(&scoped)
            .bind(&legacy)
            .execute(&mut *transaction)
            .await
            .map_err(|_| error())?;
    }
    transaction.commit().await.map_err(|_| error())
}

fn deterministic_subkey(
    key: &[u8; 32],
    label: &[u8],
) -> Result<Zeroizing<[u8; 32]>, CryptoCodecError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
    mac.update(label);
    let mut subkey = Zeroizing::new([0u8; 32]);
    subkey.copy_from_slice(&mac.finalize().into_bytes());
    Ok(subkey)
}

/// Deterministic, authenticated encryption (synthetic nonce) for lookup keys
/// that must also be decodable. Equal plaintexts give equal bytes, which only
/// reveals equality of these public references.
fn encrypt_decodable_lookup_key(
    key: &[u8; 32],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoCodecError> {
    // Deterministic, SIV-like AEAD: the nonce is a keyed MAC of the
    // plaintext under a separate subkey, so equal references encrypt to
    // equal bytes (SQL equality lookups work) and different references get
    // different nonces. Reusing a nonce therefore only ever happens for the
    // identical plaintext, which reveals equality (intended for a lookup
    // key) but never the AES-GCM keystream of two different messages.
    // decrypt_decodable_lookup_key does not recompute the MAC: it relies on
    // the AEAD tag (prefix as AAD) for integrity of what it decodes.
    let nonce_key = deterministic_subkey(key, b"GhostChat MLS storage lookup nonce v2\0")?;
    let encryption_key = deterministic_subkey(key, b"GhostChat MLS storage lookup key v2\0")?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&*nonce_key)
        .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
    mac.update(plaintext);
    let nonce_bytes = mac.finalize().into_bytes();
    let nonce = Nonce::from_slice(&nonce_bytes[..12]);
    let ciphertext = Aes256Gcm::new_from_slice(&*encryption_key)
        .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: DECODABLE_LOOKUP_KEY_PREFIX,
            },
        )
        .map_err(|_| CryptoCodecError("Could not encrypt local MLS data".into()))?;
    let mut encoded = Vec::with_capacity(DECODABLE_LOOKUP_KEY_PREFIX.len() + 12 + ciphertext.len());
    encoded.extend_from_slice(DECODABLE_LOOKUP_KEY_PREFIX);
    encoded.extend_from_slice(nonce);
    encoded.extend_from_slice(&ciphertext);
    Ok(encoded)
}

fn decrypt_decodable_lookup_key(
    key: &[u8; 32],
    slice: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoCodecError> {
    let prefix = DECODABLE_LOOKUP_KEY_PREFIX.len();
    if slice.len() < prefix + 12 + 16 {
        return Err(CryptoCodecError(
            "Encrypted MLS data has an invalid format".into(),
        ));
    }
    let encryption_key = deterministic_subkey(key, b"GhostChat MLS storage lookup key v2\0")?;
    Aes256Gcm::new_from_slice(&*encryption_key)
        .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?
        .decrypt(
            Nonce::from_slice(&slice[prefix..prefix + 12]),
            Payload {
                msg: &slice[prefix + 12..],
                aad: DECODABLE_LOOKUP_KEY_PREFIX,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoCodecError("Could not decrypt local MLS data".into()))
}

/// Older builds stored HashReference values as one-way `GCK\x01` digests.
/// Rewrite KeyPackage and queued-proposal rows to the decodable encoding,
/// recomputing each reference from its decrypted entity. A row whose
/// recomputed digest does not match its stored key is left untouched.
async fn migrate_hash_reference_keys(db: &SqlitePool) -> Result<(), String> {
    let marker = migration_marker(db);
    if let Some(path) = &marker {
        let done = MIGRATED_DATABASES.lock().unwrap_or_else(|e| e.into_inner());
        if done.as_ref().is_some_and(|set| set.contains(path)) {
            return Ok(());
        }
    }
    migrate_hash_reference_keys_pass(db).await?;
    if let Some(path) = marker {
        MIGRATED_DATABASES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Default::default)
            .insert(path);
    }
    Ok(())
}

async fn migrate_hash_reference_keys_pass(db: &SqlitePool) -> Result<(), String> {
    let error = || "GhostChat could not upgrade its encrypted MLS storage.".to_string();
    let key = STORAGE_KEY.get().ok_or_else(error)?;
    let crypto = RustCrypto::default();
    let mut transaction = db.begin().await.map_err(|_| error())?;
    let key_packages = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT key_package_ref,key_package FROM openmls_key_package WHERE substr(key_package_ref,1,4)=?",
    )
    .bind(LOOKUP_KEY_V1_PREFIX.as_slice())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| error())?;
    for (old_ref, bundle) in key_packages {
        let Ok(bundle) =
            EncryptedJsonCodec::from_slice::<openmls::prelude::KeyPackageBundle>(&bundle)
        else {
            continue;
        };
        let Ok(hash_ref) = bundle.key_package().hash_ref(&crypto) else {
            continue;
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&hash_ref).map_err(|_| error())?);
        if lookup_key_v1(key, &plaintext).map_err(|_| error())? != old_ref {
            continue;
        }
        let new_ref = encrypt_decodable_lookup_key(key, &plaintext).map_err(|_| error())?;
        // If a decodable row for the same reference already exists, the
        // update is ignored and the duplicate legacy row is dropped below, so
        // a conflict cannot fail every later crypto call. Dropping it loses
        // nothing: the legacy key was just checked to be the digest of this
        // row's own reference, and a KeyPackage reference is the hash of the
        // KeyPackage itself, so both rows hold the same KeyPackage.
        sqlx::query(
            "UPDATE OR IGNORE openmls_key_package SET key_package_ref=? WHERE key_package_ref=?",
        )
        .bind(new_ref)
        .bind(&old_ref)
        .execute(&mut *transaction)
        .await
        .map_err(|_| error())?;
        sqlx::query("DELETE FROM openmls_key_package WHERE key_package_ref=?")
            .bind(&old_ref)
            .execute(&mut *transaction)
            .await
            .map_err(|_| error())?;
    }
    let proposals = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Vec<u8>)>(
        "SELECT group_id,proposal_ref,proposal FROM openmls_proposal WHERE substr(proposal_ref,1,4)=?",
    )
    .bind(LOOKUP_KEY_V1_PREFIX.as_slice())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| error())?;
    for (group_id, old_ref, proposal) in proposals {
        let Ok(proposal) =
            EncryptedJsonCodec::from_slice::<openmls::prelude::QueuedProposal>(&proposal)
        else {
            continue;
        };
        let plaintext = Zeroizing::new(
            serde_json::to_vec(proposal.proposal_reference_ref()).map_err(|_| error())?,
        );
        if lookup_key_v1(key, &plaintext).map_err(|_| error())? != old_ref {
            continue;
        }
        let new_ref = encrypt_decodable_lookup_key(key, &plaintext).map_err(|_| error())?;
        // As for KeyPackages: a proposal reference is the hash of the
        // proposal, so a decodable row with the same group and reference
        // holds the same proposal and the legacy duplicate can be dropped.
        sqlx::query(
            "UPDATE OR IGNORE openmls_proposal SET proposal_ref=? WHERE group_id=? AND proposal_ref=?",
        )
        .bind(new_ref)
        .bind(&group_id)
        .bind(&old_ref)
        .execute(&mut *transaction)
        .await
        .map_err(|_| error())?;
        sqlx::query("DELETE FROM openmls_proposal WHERE group_id=? AND proposal_ref=?")
            .bind(&group_id)
            .bind(&old_ref)
            .execute(&mut *transaction)
            .await
            .map_err(|_| error())?;
    }
    transaction.commit().await.map_err(|_| error())
}

fn key_fingerprint(key: &[u8]) -> String {
    Sha256::digest(key)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.join(""))
        .collect::<Vec<_>>()
        .join(" ")
}

fn group_identity_fingerprint<'a>(
    members: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
) -> String {
    let mut identities = members
        .into_iter()
        .map(|(credential, signature_key)| {
            let mut identity = Vec::with_capacity(16 + credential.len() + signature_key.len());
            identity.extend_from_slice(&(credential.len() as u64).to_be_bytes());
            identity.extend_from_slice(credential);
            identity.extend_from_slice(&(signature_key.len() as u64).to_be_bytes());
            identity.extend_from_slice(signature_key);
            identity
        })
        .collect::<Vec<_>>();
    identities.sort();
    let mut digest = Sha256::new();
    digest.update(b"GhostChat MLS group identity v1");
    for identity in identities {
        digest.update(identity);
    }
    key_fingerprint(&digest.finalize())
}

fn decode_key_package(
    provider: &DesktopMlsProvider<'_>,
    peer_user_id: &str,
    peer_device_id: &str,
    payload: &str,
) -> Result<(openmls::prelude::KeyPackage, String), String> {
    if payload.len() > 128 * 1024 {
        return Err("The other device's key package is too large.".into());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|_| "The other device's key package has invalid encoding.".to_string())?;
    let message = MlsMessageIn::tls_deserialize_exact(bytes.as_slice())
        .map_err(|_| "The other device's key package could not be read.".to_string())?;
    let key_package = match message.extract() {
        MlsMessageBodyIn::KeyPackage(key_package) => key_package,
        _ => return Err("The server returned a message that is not a key package.".into()),
    };
    let credential = key_package.unverified_credential();
    if credential.credential.credential_type() != CredentialType::Basic {
        return Err("The other device uses an unsupported MLS identity.".into());
    }
    let expected_identity = format!("ghostchat:v1:{peer_user_id}:{peer_device_id}");
    if credential.credential.serialized_content() != expected_identity.as_bytes() {
        return Err(
            "The key package identity does not match the selected account and device.".into(),
        );
    }
    let fingerprint = key_fingerprint(credential.signature_key.as_slice());
    let key_package = key_package
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .map_err(|_| {
            "The other device's key package failed MLS signature or lifetime checks.".to_string()
        })?;
    Ok((key_package, fingerprint))
}

pub async fn prepare_device(db: &SqlitePool, user_id: &str) -> Result<DevicePreparation, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = Uuid::parse_str(user_id)
        .map_err(|_| "The signed-in account ID is invalid.".to_string())?
        .to_string();

    install_storage_key(get_storage_key()?)?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_identities (\
            user_id TEXT PRIMARY KEY NOT NULL,\
            device_id TEXT NOT NULL UNIQUE,\
            signing_public_key BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its local encryption database.".to_string())?;

    {
        let mut connection = db
            .acquire()
            .await
            .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
        let mut storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection);
        storage
            .run_migrations()
            .map_err(|_| "GhostChat could not initialize its encrypted MLS storage.".to_string())?;
    }
    migrate_hash_reference_keys(db).await?;

    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start a local encryption transaction.".to_string())?;
    let existing = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this account's local device identity.".to_string())?;

    let (device_id, stored_public_key) = match existing {
        Some((device_id, public_key)) => {
            Uuid::parse_str(&device_id)
                .map_err(|_| "The saved encryption device identity is invalid.".to_string())?;
            (device_id, Some(public_key))
        }
        None => (Uuid::new_v4().to_string(), None),
    };

    let mut public_key_to_save = None;
    let (package_ref, payload, identity_fingerprint) = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = if let Some(public_key) = stored_public_key.as_deref() {
            SignatureKeyPair::read(
                provider.storage(),
                public_key,
                MLS_CIPHERSUITE.signature_algorithm(),
            )
            .ok_or_else(|| {
                "The saved encryption identity is missing or cannot be decrypted. No replacement was created.".to_string()
            })?
        } else {
            let signer = SignatureKeyPair::new(MLS_CIPHERSUITE.signature_algorithm())
                .map_err(|_| "GhostChat could not create a device signing key.".to_string())?;
            signer.store(provider.storage()).map_err(|_| {
                "GhostChat could not securely save the device signing key.".to_string()
            })?;
            public_key_to_save = Some(signer.public().to_vec());
            signer
        };

        let credential =
            BasicCredential::new(format!("ghostchat:v1:{user_id}:{device_id}").into_bytes());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.to_public_vec().into(),
        };
        let bundle = KeyPackage::builder()
            .build(MLS_CIPHERSUITE, &provider, &signer, credential_with_key)
            .map_err(|_| "GhostChat could not create an MLS key package.".to_string())?;
        let package_reference = bundle
            .key_package()
            .hash_ref(provider.crypto())
            .map_err(|_| "GhostChat could not identify the MLS key package.".to_string())?;
        let ref_bytes = package_reference.tls_serialize_detached().map_err(|_| {
            "GhostChat could not serialize the MLS key package reference.".to_string()
        })?;
        let package_bytes = MlsMessageOut::from(bundle.key_package().clone())
            .tls_serialize_detached()
            .map_err(|_| "GhostChat could not serialize the MLS key package.".to_string())?;
        let fingerprint = Sha256::digest(signer.public())
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .chunks(4)
            .map(|chunk| chunk.join(""))
            .collect::<Vec<_>>()
            .join(" ");

        (
            URL_SAFE_NO_PAD.encode(ref_bytes),
            URL_SAFE_NO_PAD.encode(package_bytes),
            fingerprint,
        )
    };

    if let Some(public_key) = public_key_to_save {
        sqlx::query(
            "INSERT INTO ghostchat_crypto_identities(user_id,device_id,signing_public_key) VALUES(?,?,?)",
        )
        .bind(&user_id)
        .bind(&device_id)
        .bind(public_key)
        .execute(&mut *transaction)
        .await
        .map_err(|_| "GhostChat could not save the local device identity.".to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the encrypted MLS state.".to_string())?;

    Ok(DevicePreparation {
        device_id,
        label: "GhostChat desktop".into(),
        package_ref,
        payload,
        identity_fingerprint,
    })
}

pub async fn inspect_key_package(
    db: &SqlitePool,
    peer_user_id: &str,
    peer_device_id: &str,
    payload: &str,
) -> Result<KeyPackageInspection, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let peer_user_id = normalized_uuid(peer_user_id, "account")?;
    let peer_device_id = normalized_uuid(peer_device_id, "device")?;
    prepare_crypto_storage(db).await?;
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection);
    let provider = DesktopMlsProvider {
        crypto: RustCrypto::default(),
        storage,
    };
    let (_, identity_fingerprint) =
        decode_key_package(&provider, &peer_user_id, &peer_device_id, payload)?;
    Ok(KeyPackageInspection {
        identity_fingerprint,
    })
}

pub async fn create_direct_group(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    peer_user_id: &str,
    peer_device_id: &str,
    peer_package_payload: &str,
) -> Result<DirectGroupStart, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let peer_user_id = normalized_uuid(peer_user_id, "peer account")?;
    let peer_device_id = normalized_uuid(peer_device_id, "peer device")?;
    if user_id == peer_user_id {
        return Err("An encrypted direct conversation needs two different accounts.".into());
    }
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;

    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start an encrypted conversation.".to_string())?;
    let (device_id, public_key) = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let group_id = group_id_for_conversation(&conversation_id);

    let (welcome_payload, peer_identity_fingerprint) = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let basic_identity = format!("ghostchat:v1:{user_id}:{device_id}").into_bytes();
        let credential = BasicCredential::new(basic_identity);
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.to_public_vec().into(),
        };
        let (peer_key_package, peer_identity_fingerprint) = decode_key_package(
            &provider,
            &peer_user_id,
            &peer_device_id,
            peer_package_payload,
        )?;

        if MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())?
            .is_some()
        {
            return Err("Encryption is already set up for this conversation.".into());
        }

        let mut group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &group_create_config(),
            group_id,
            credential_with_key,
        )
        .map_err(|_| "GhostChat could not create the MLS conversation.".to_string())?;
        let (_commit, welcome, _) = group
            .add_members(&provider, &signer, std::slice::from_ref(&peer_key_package))
            .map_err(|_| {
                "GhostChat could not add the verified device to the MLS conversation.".to_string()
            })?;
        group
            .merge_pending_commit(&provider)
            .map_err(|_| "GhostChat could not save the new MLS conversation state.".to_string())?;
        let welcome_payload = URL_SAFE_NO_PAD.encode(
            welcome
                .tls_serialize_detached()
                .map_err(|_| "GhostChat could not serialize the MLS invitation.".to_string())?,
        );
        (welcome_payload, peer_identity_fingerprint)
    };
    let bootstrap = DirectGroupStart {
        welcome_id: Uuid::new_v4().to_string(),
        welcome_payload,
        welcome_recipients: vec![peer_user_id],
        peer_identity_fingerprint,
    };
    let encrypted_bootstrap = EncryptedJsonCodec::to_vec(&bootstrap)
        .map_err(|_| "GhostChat could not protect its pending encrypted setup.".to_string())?;
    sqlx::query(
        "INSERT INTO ghostchat_crypto_bootstrap(user_id,conversation_id,payload) VALUES(?,?,?)",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(encrypted_bootstrap)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the pending encrypted setup.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not persist the encrypted conversation.".to_string())?;

    Ok(bootstrap)
}

pub async fn create_group(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    peers: &[GroupPeerKeyPackage],
) -> Result<DirectGroupStart, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    if peers.is_empty() || peers.len() > 15 {
        return Err("An encrypted group needs 1 to 15 other members.".into());
    }
    let mut peer_ids = std::collections::HashSet::new();
    let normalized_peers = peers
        .iter()
        .map(|peer| {
            let peer_user_id = normalized_uuid(&peer.user_id, "peer account")?;
            let peer_device_id = normalized_uuid(&peer.device_id, "peer device")?;
            if peer_user_id == user_id || !peer_ids.insert(peer_user_id.clone()) {
                return Err("Each encrypted group member must be a different account.".into());
            }
            Ok((peer_user_id, peer_device_id, peer.payload.as_str()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;

    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start an encrypted group.".to_string())?;
    let (device_id, public_key) = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let group_id = group_id_for_conversation(&conversation_id);

    let (welcome_payload, group_fingerprint) = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let credential =
            BasicCredential::new(format!("ghostchat:v1:{user_id}:{device_id}").into_bytes());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.to_public_vec().into(),
        };
        let mut key_packages = Vec::with_capacity(normalized_peers.len());
        for (peer_user_id, peer_device_id, payload) in &normalized_peers {
            let (key_package, _) =
                decode_key_package(&provider, peer_user_id, peer_device_id, payload)?;
            key_packages.push(key_package);
        }
        if MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())?
            .is_some()
        {
            return Err("Encryption is already set up for this conversation.".into());
        }
        let mut group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &group_create_config(),
            group_id,
            credential_with_key,
        )
        .map_err(|_| "GhostChat could not create the MLS group.".to_string())?;
        let (_commit, welcome, _) = group
            .add_members(&provider, &signer, &key_packages)
            .map_err(|_| {
                "GhostChat could not add the verified members to the MLS group.".to_string()
            })?;
        group
            .merge_pending_commit(&provider)
            .map_err(|_| "GhostChat could not save the new MLS group state.".to_string())?;
        let actual_members = group.members().collect::<Vec<_>>();
        if actual_members.len() != peers.len() + 1 {
            return Err("The MLS group member list does not match the conversation.".into());
        }
        let group_fingerprint = group_identity_fingerprint(actual_members.iter().map(|member| {
            (
                member.credential.serialized_content(),
                member.signature_key.as_slice(),
            )
        }));
        let welcome_payload =
            URL_SAFE_NO_PAD.encode(welcome.tls_serialize_detached().map_err(|_| {
                "GhostChat could not serialize the MLS group invitation.".to_string()
            })?);
        (welcome_payload, group_fingerprint)
    };
    let mut welcome_recipients: Vec<String> = normalized_peers
        .iter()
        .map(|(peer_user_id, _, _)| peer_user_id.clone())
        .collect();
    welcome_recipients.sort();
    let bootstrap = DirectGroupStart {
        welcome_id: Uuid::new_v4().to_string(),
        welcome_payload,
        welcome_recipients,
        peer_identity_fingerprint: group_fingerprint,
    };
    let encrypted_bootstrap = EncryptedJsonCodec::to_vec(&bootstrap)
        .map_err(|_| "GhostChat could not protect its pending encrypted setup.".to_string())?;
    sqlx::query(
        "INSERT INTO ghostchat_crypto_bootstrap(user_id,conversation_id,payload) VALUES(?,?,?)",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(encrypted_bootstrap)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the pending encrypted setup.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not persist the encrypted MLS group.".to_string())?;
    Ok(bootstrap)
}

pub async fn get_pending_group_membership_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<GroupMembershipUpdate>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    let payload = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not load the pending encrypted membership update.".to_string())?;
    payload
        .map(|payload| {
            EncryptedJsonCodec::from_slice(&payload).map_err(|_| {
                "The pending encrypted membership update could not be decrypted.".to_string()
            })
        })
        .transpose()
}

/// Drops a staged membership commit that the server definitively refused
/// (or that another owner/admin's accepted commit made moot): the pending
/// MLS commit is cleared, so the group stays in its current epoch and can
/// apply the commit that was accepted instead, and the outbox row is
/// removed so it is not retried. Only the operation in the outbox matches.
pub async fn discard_group_membership_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    operation_id: &str,
) -> Result<(), String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let operation_id = normalized_uuid(operation_id, "membership operation")?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let error = || "GhostChat could not drop the refused membership update.".to_string();
    let mut transaction = db.begin().await.map_err(|_| error())?;
    let Some(payload) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| error())?
    else {
        return Ok(());
    };
    let pending: GroupMembershipUpdate =
        EncryptedJsonCodec::from_slice(&payload).map_err(|_| error())?;
    if pending.operation_id != operation_id {
        return Err("The pending MLS membership operation does not match.".into());
    }
    if !matches!(
        pending.action.as_str(),
        "add" | "remove" | "remove_deleted" | "leave"
    ) {
        return Err("Only a staged membership commit can be dropped.".into());
    }
    {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        if let Some(mut group) =
            MlsGroup::load(&storage, &group_id_for_conversation(&conversation_id))
                .map_err(|_| error())?
        {
            group.clear_pending_commit(&storage).map_err(|_| error())?;
        }
    }
    sqlx::query(
        "DELETE FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .execute(&mut *transaction)
    .await
    .map_err(|_| error())?;
    transaction.commit().await.map_err(|_| error())
}

pub async fn prepare_group_membership_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    peers: &[GroupPeerKeyPackage],
    expected_commit_seq: Option<i64>,
) -> Result<GroupMembershipUpdate, String> {
    prepare_group_membership_change(
        db,
        user_id,
        conversation_id,
        expected_member_ids,
        "add",
        &[],
        &[],
        peers,
        &[],
        expected_commit_seq,
    )
    .await?
    .ok_or_else(|| "There is no encrypted membership update to prepare.".to_string())
}

pub async fn prepare_group_removal_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    removed_member_ids: &[String],
    expected_commit_seq: Option<i64>,
) -> Result<GroupMembershipUpdate, String> {
    prepare_group_membership_change(
        db,
        user_id,
        conversation_id,
        expected_member_ids,
        "remove",
        removed_member_ids,
        &[],
        &[],
        &[],
        expected_commit_seq,
    )
    .await?
    .ok_or_else(|| "There is no encrypted membership update to prepare.".to_string())
}

/// Stages an MLS Commit removing one leaf of an account the server recorded
/// as deleted from this group (`deleted_member_ids`), or returns `None` when
/// no such leaf is left. A pending membership update is returned as is.
/// The local MLS roster must equal the server roster plus only recorded
/// deleted accounts.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn prepare_deleted_member_removal(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    deleted_member_ids: &[String],
    expected_commit_seq: Option<i64>,
) -> Result<Option<GroupMembershipUpdate>, String> {
    prepare_deleted_member_removal_excluding(
        db,
        user_id,
        conversation_id,
        expected_member_ids,
        deleted_member_ids,
        &[],
        expected_commit_seq,
    )
    .await
}

/// Like `prepare_deleted_member_removal`, but never stages the removal of a
/// leaf in `excluded_member_ids` (deleted accounts whose removal the server
/// refused), so the next deleted leaf is staged instead of the refused one.
pub async fn prepare_deleted_member_removal_excluding(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    deleted_member_ids: &[String],
    excluded_member_ids: &[String],
    expected_commit_seq: Option<i64>,
) -> Result<Option<GroupMembershipUpdate>, String> {
    prepare_group_membership_change(
        db,
        user_id,
        conversation_id,
        expected_member_ids,
        "remove_deleted",
        &[],
        deleted_member_ids,
        &[],
        excluded_member_ids,
        expected_commit_seq,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_group_membership_change(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    action: &str,
    selected_member_ids: &[String],
    deleted_member_ids: &[String],
    peers: &[GroupPeerKeyPackage],
    excluded_member_ids: &[String],
    expected_commit_seq: Option<i64>,
) -> Result<Option<GroupMembershipUpdate>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let expected_commit_seq = checked_commit_seq(expected_commit_seq)?;
    if action == "add" && (peers.is_empty() || peers.len() > 15 || !selected_member_ids.is_empty())
    {
        return Err("Choose 1 to 15 friends to add to the encrypted group.".into());
    }
    if action == "remove" && (selected_member_ids.len() != 1 || !peers.is_empty()) {
        return Err("Choose exactly one encrypted group member to remove.".into());
    }
    if action == "remove_deleted" && (!selected_member_ids.is_empty() || !peers.is_empty()) {
        return Err("Unsupported encrypted membership operation.".into());
    }
    if !matches!(action, "add" | "remove" | "remove_deleted") {
        return Err("Unsupported encrypted membership operation.".into());
    }
    let deleted_member_ids = normalized_deleted_member_ids(deleted_member_ids)?;
    let excluded_member_ids = excluded_member_ids
        .iter()
        .map(|member_id| normalized_uuid(member_id, "excluded group member"))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;

    let mut expected_member_ids = expected_member_ids
        .iter()
        .map(|member_id| normalized_uuid(member_id, "current group member"))
        .collect::<Result<Vec<_>, _>>()?;
    let expected_count = expected_member_ids.len();
    expected_member_ids.sort();
    expected_member_ids.dedup();
    if expected_member_ids.len() != expected_count || !(2..=16).contains(&expected_count) {
        return Err("The current encrypted group roster is invalid.".into());
    }

    let mut peer_ids = std::collections::HashSet::new();
    let mut normalized_peers = peers
        .iter()
        .map(|peer| {
            let peer_user_id = normalized_uuid(&peer.user_id, "peer account")?;
            let peer_device_id = normalized_uuid(&peer.device_id, "peer device")?;
            if peer_user_id == user_id || !peer_ids.insert(peer_user_id.clone()) {
                return Err("Each encrypted group member must be a different account.".into());
            }
            Ok((peer_user_id, peer_device_id, peer.payload.as_str()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    normalized_peers.sort_by(|left, right| left.0.cmp(&right.0));
    let mut member_ids = if action == "add" {
        normalized_peers
            .iter()
            .map(|(peer_user_id, _, _)| peer_user_id.clone())
            .collect::<Vec<_>>()
    } else {
        selected_member_ids
            .iter()
            .map(|member_id| normalized_uuid(member_id, "group member"))
            .collect::<Result<Vec<_>, _>>()?
    };
    let affected_count = member_ids.len();
    member_ids.sort();
    member_ids.dedup();
    if member_ids.len() != affected_count {
        return Err("Each encrypted group member can only be selected once.".into());
    }
    let current_set = expected_member_ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    let invalid_add = action == "add"
        && (member_ids
            .iter()
            .any(|member_id| current_set.contains(member_id))
            || expected_member_ids.len() + member_ids.len() > 16);
    let invalid_remove = action == "remove"
        && (member_ids
            .iter()
            .any(|member_id| !current_set.contains(member_id) || member_id == &user_id)
            || expected_member_ids.len().saturating_sub(member_ids.len()) < 1);
    if invalid_add || invalid_remove {
        return Err("The encrypted group roster or selected friends are invalid.".into());
    }

    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start an encrypted membership update.".to_string())?;
    if let Some(payload) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not check for a pending membership update.".to_string())?
    {
        let pending: GroupMembershipUpdate = EncryptedJsonCodec::from_slice(&payload).map_err(|_| {
            "The pending encrypted membership update could not be decrypted.".to_string()
        })?;
        transaction
            .rollback()
            .await
            .map_err(|_| "GhostChat could not close the local encryption transaction.".to_string())?;
        return Ok(Some(pending));
    }

    let (device_id, public_key) = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let group_id = group_id_for_conversation(&conversation_id);

    let (commit_payload, welcome_payload) = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let mut group = MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the encrypted MLS group.".to_string())?
            .ok_or_else(|| {
                "Encryption has not been set up for this group on this device.".to_string()
            })?;

        let members = group.members().collect::<Vec<_>>();
        let actual_member_ids = members
            .iter()
            .filter(|member| member.credential.credential_type() == CredentialType::Basic)
            .filter_map(|member| account_id_from_credential(member.credential.serialized_content()))
            .collect::<std::collections::HashSet<_>>();
        let own_identity = format!("ghostchat:v1:{user_id}:{device_id}");
        let current_set = current_set
            .into_iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        // Only a deleted-account removal tolerates extra, recorded leaves.
        let tolerated = if action == "remove_deleted" {
            deleted_member_ids
        } else {
            std::collections::HashSet::new()
        };
        if actual_member_ids.len() != members.len()
            || !roster_matches_allowing_deleted(&actual_member_ids, &current_set, &tolerated)
            || !members
                .iter()
                .any(|member| member.credential.serialized_content() == own_identity.as_bytes())
        {
            return Err("The local MLS group does not match the current server roster.".into());
        }
        if action == "remove_deleted" {
            // A leaf whose removal the server refused is left to a later
            // roster change; the next deleted leaf is staged instead.
            let mut leftover = actual_member_ids
                .difference(&current_set)
                .filter(|member_id| !excluded_member_ids.contains(*member_id))
                .cloned()
                .collect::<Vec<_>>();
            leftover.sort();
            let Some(target) = leftover.into_iter().next() else {
                return Ok(None);
            };
            member_ids = vec![target];
        }

        let (commit, welcome) = if action == "add" {
            let mut key_packages = Vec::with_capacity(normalized_peers.len());
            for (peer_user_id, peer_device_id, payload) in &normalized_peers {
                let (key_package, _) =
                    decode_key_package(&provider, peer_user_id, peer_device_id, payload)?;
                key_packages.push(key_package);
            }
            let (commit, welcome, _) = group
                .add_members(&provider, &signer, &key_packages)
                .map_err(|_| {
                    "GhostChat could not create the authenticated MLS membership update."
                        .to_string()
                })?;
            (commit, Some(welcome))
        } else {
            let remove_indices = members
                .iter()
                .filter(|member| {
                    account_id_from_credential(member.credential.serialized_content())
                        .is_some_and(|account_id| member_ids.contains(&account_id))
                })
                .map(|member| member.index)
                .collect::<Vec<_>>();
            if remove_indices.len() != member_ids.len() {
                return Err(
                    "The selected account does not have exactly one MLS device in this group."
                        .into(),
                );
            }
            let (commit, welcome, _) = group
                .remove_members(&provider, &signer, &remove_indices)
                .map_err(|_| {
                    "GhostChat could not create the authenticated MLS removal.".to_string()
                })?;
            (commit, welcome)
        };
        let commit_payload =
            URL_SAFE_NO_PAD.encode(commit.tls_serialize_detached().map_err(|_| {
                "GhostChat could not serialize the MLS membership update.".to_string()
            })?);
        let welcome_payload = if let Some(welcome) = welcome {
            URL_SAFE_NO_PAD.encode(
                welcome
                    .tls_serialize_detached()
                    .map_err(|_| "GhostChat could not serialize the MLS invitation.".to_string())?,
            )
        } else {
            String::new()
        };
        (commit_payload, welcome_payload)
    };

    let update = GroupMembershipUpdate {
        operation_id: Uuid::new_v4().to_string(),
        action: action.into(),
        member_ids,
        expected_member_ids,
        commit_payload,
        welcome_payload,
        expected_commit_seq,
    };
    let encrypted_update = EncryptedJsonCodec::to_vec(&update).map_err(|_| {
        "GhostChat could not protect the pending MLS membership update.".to_string()
    })?;
    sqlx::query(
        "INSERT INTO ghostchat_crypto_membership_outbox(user_id,conversation_id,payload) VALUES(?,?,?)",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(encrypted_update)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the pending MLS membership update.".to_string())?;
    transaction.commit().await.map_err(|_| {
        "GhostChat could not persist the pending MLS membership update.".to_string()
    })?;
    Ok(Some(update))
}

pub async fn prepare_group_leave_proposal(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
) -> Result<GroupMembershipUpdate, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let mut expected_member_ids = expected_member_ids
        .iter()
        .map(|member_id| normalized_uuid(member_id, "group member"))
        .collect::<Result<Vec<_>, _>>()?;
    let expected_count = expected_member_ids.len();
    expected_member_ids.sort();
    expected_member_ids.dedup();
    if expected_member_ids.len() != expected_count
        || !(2..=16).contains(&expected_count)
        || !expected_member_ids.contains(&user_id)
    {
        return Err("The current encrypted group roster is invalid.".into());
    }

    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start the encrypted leave request.".to_string())?;
    let mut discard_stale_leave_proposal = false;
    if let Some(payload) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not check for a pending membership update.".to_string())?
    {
        let pending: GroupMembershipUpdate = EncryptedJsonCodec::from_slice(&payload).map_err(|_| {
            "The pending encrypted membership update could not be decrypted.".to_string()
        })?;
        if pending.action == "leave_request"
            && pending.member_ids.len() == 1
            && pending.member_ids[0] == user_id
        {
            if pending.expected_member_ids == expected_member_ids {
                transaction.rollback().await.map_err(|_| {
                    "GhostChat could not close the local encryption transaction.".to_string()
                })?;
                return Ok(pending);
            }
            discard_stale_leave_proposal = true;
            sqlx::query(
                "DELETE FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
            )
            .bind(&user_id)
            .bind(&conversation_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| "GhostChat could not reset the stale leave request.".to_string())?;
        } else {
            transaction.rollback().await.map_err(|_| {
                "GhostChat could not close the local encryption transaction.".to_string()
            })?;
            return Err("Finish the pending encrypted membership update before leaving.".into());
        }
    }

    let (device_id, public_key) = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let group_id = group_id_for_conversation(&conversation_id);
    let proposal_payload = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let mut group = MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the encrypted MLS group.".to_string())?
            .ok_or_else(|| "The encrypted MLS group is missing on this device.".to_string())?;
        if discard_stale_leave_proposal {
            let pending_proposals = group.pending_proposals().collect::<Vec<_>>();
            let only_own_leave_proposals = pending_proposals.iter().all(|queued| {
                let Sender::Member(sender_index) = queued.sender() else {
                    return false;
                };
                matches!(queued.proposal(), Proposal::Remove(remove) if remove.removed() == *sender_index)
                    && group.members().any(|member| {
                        member.index == *sender_index
                            && account_id_from_credential(member.credential.serialized_content())
                                == Some(user_id.clone())
                    })
            });
            if !only_own_leave_proposals {
                return Err("The local MLS group has other pending proposals; ask its owner to finish those changes first.".into());
            }
            group
                .clear_pending_proposals(provider.storage())
                .map_err(|_| {
                    "GhostChat could not reset the old local MLS leave proposal.".to_string()
                })?;
        }
        let identities = group_account_identities(&group).ok_or_else(|| {
            "The local MLS group contains invalid or duplicate identities.".to_string()
        })?;
        let own_identity = format!("ghostchat:v1:{user_id}:{device_id}").into_bytes();
        if identities.len() != expected_member_ids.len()
            || identities
                .keys()
                .any(|member_id| !expected_member_ids.contains(member_id))
            || identities
                .get(&user_id)
                .map(|identity| identity.0.as_slice())
                != Some(own_identity.as_slice())
            || identities
                .get(&user_id)
                .map(|identity| identity.1.as_slice())
                != Some(signer.public())
        {
            return Err("The local MLS group does not match the current server roster.".into());
        }
        let proposal = group.leave_group(&provider, &signer).map_err(|_| {
            "GhostChat could not create the authenticated MLS leave proposal.".to_string()
        })?;
        URL_SAFE_NO_PAD.encode(
            proposal
                .tls_serialize_detached()
                .map_err(|_| "GhostChat could not serialize the MLS leave proposal.".to_string())?,
        )
    };
    let update = GroupMembershipUpdate {
        operation_id: Uuid::new_v4().to_string(),
        action: "leave_request".into(),
        member_ids: vec![user_id.clone()],
        expected_member_ids,
        commit_payload: proposal_payload,
        welcome_payload: String::new(),
        // A proposal is not a commit; the owner's commit of it is stamped.
        expected_commit_seq: None,
    };
    let encrypted_update = EncryptedJsonCodec::to_vec(&update)
        .map_err(|_| "GhostChat could not protect the pending leave request.".to_string())?;
    sqlx::query(
        "INSERT INTO ghostchat_crypto_membership_outbox(user_id,conversation_id,payload) VALUES(?,?,?)",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(encrypted_update)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the pending leave request.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not persist the pending leave request.".to_string())?;
    Ok(update)
}

pub async fn prepare_group_leave_commit(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    operation_id: &str,
    leaving_user_id: &str,
    expected_member_ids: &[String],
    expected_commit_seq: Option<i64>,
) -> Result<GroupMembershipUpdate, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let expected_commit_seq = checked_commit_seq(expected_commit_seq)?;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let operation_id = normalized_uuid(operation_id, "leave operation")?;
    let leaving_user_id = normalized_uuid(leaving_user_id, "leaving account")?;
    let mut expected_member_ids = expected_member_ids
        .iter()
        .map(|member_id| normalized_uuid(member_id, "group member"))
        .collect::<Result<Vec<_>, _>>()?;
    let expected_count = expected_member_ids.len();
    expected_member_ids.sort();
    expected_member_ids.dedup();
    if expected_member_ids.len() != expected_count
        || !(2..=16).contains(&expected_count)
        || !expected_member_ids.contains(&user_id)
        || !expected_member_ids.contains(&leaving_user_id)
        || user_id == leaving_user_id
    {
        return Err("The encrypted group leave request does not match its roster.".into());
    }

    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start the encrypted leave commit.".to_string())?;
    if let Some(payload) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not check for a pending membership update.".to_string())?
    {
        let pending: GroupMembershipUpdate = EncryptedJsonCodec::from_slice(&payload).map_err(|_| {
            "The pending encrypted membership update could not be decrypted.".to_string()
        })?;
        transaction
            .rollback()
            .await
            .map_err(|_| "GhostChat could not close the local encryption transaction.".to_string())?;
        if pending.action == "leave"
            && pending.operation_id == operation_id
            && pending.member_ids.len() == 1
            && pending.member_ids[0] == leaving_user_id
        {
            return Ok(pending);
        }
        return Err("Finish the pending encrypted membership update before committing this leave.".into());
    }

    let (device_id, public_key) = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let group_id = group_id_for_conversation(&conversation_id);
    let commit_payload = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let mut group = MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the encrypted MLS group.".to_string())?
            .ok_or_else(|| "The encrypted MLS group is missing on this device.".to_string())?;
        let identities = group_account_identities(&group).ok_or_else(|| {
            "The local MLS group contains invalid or duplicate identities.".to_string()
        })?;
        let own_identity = format!("ghostchat:v1:{user_id}:{device_id}").into_bytes();
        if identities.len() != expected_member_ids.len()
            || identities
                .keys()
                .any(|member_id| !expected_member_ids.contains(member_id))
            || identities
                .get(&user_id)
                .map(|identity| identity.0.as_slice())
                != Some(own_identity.as_slice())
            || identities
                .get(&user_id)
                .map(|identity| identity.1.as_slice())
                != Some(signer.public())
        {
            return Err("The local MLS group does not match the current server roster.".into());
        }
        let pending = group.pending_proposals().collect::<Vec<_>>();
        if pending.len() != 1 {
            return Err(
                "The owner must resolve other pending MLS proposals before this leave can finish."
                    .into(),
            );
        }
        let sender_index = match pending[0].sender() {
            Sender::Member(index) => *index,
            _ => return Err("Only an MLS group member can request to leave.".into()),
        };
        if !matches!(pending[0].proposal(), Proposal::Remove(remove) if remove.removed() == sender_index)
            || !group.members().any(|member| {
                member.index == sender_index
                    && account_id_from_credential(member.credential.serialized_content())
                        == Some(leaving_user_id.clone())
            })
        {
            return Err("The pending MLS proposal is not a matching self-leave request.".into());
        }
        let (commit, welcome, _) = group
            .commit_to_pending_proposals(&provider, &signer)
            .map_err(|_| "GhostChat could not commit the MLS leave request.".to_string())?;
        if welcome.is_some() {
            return Err("An MLS leave commit unexpectedly produced a Welcome message.".into());
        }
        URL_SAFE_NO_PAD.encode(
            commit
                .tls_serialize_detached()
                .map_err(|_| "GhostChat could not serialize the MLS leave commit.".to_string())?,
        )
    };
    let update = GroupMembershipUpdate {
        operation_id,
        action: "leave".into(),
        member_ids: vec![leaving_user_id],
        expected_member_ids,
        commit_payload,
        welcome_payload: String::new(),
        expected_commit_seq,
    };
    let encrypted_update = EncryptedJsonCodec::to_vec(&update)
        .map_err(|_| "GhostChat could not protect the pending MLS leave commit.".to_string())?;
    sqlx::query(
        "INSERT INTO ghostchat_crypto_membership_outbox(user_id,conversation_id,payload) VALUES(?,?,?)",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(encrypted_update)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the pending MLS leave commit.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not persist the pending MLS leave commit.".to_string())?;
    Ok(update)
}

pub async fn forget_group_state(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<(), String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start local group cleanup.".to_string())?;
    let group_id = group_id_for_conversation(&conversation_id);
    {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        if let Some(mut group) = MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the local MLS group for cleanup.".to_string())?
        {
            group
                .delete(provider.storage())
                .map_err(|_| "GhostChat could not remove the local MLS group state.".to_string())?;
        }
    }
    sqlx::query(
        "DELETE FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not clear the local membership outbox.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save local group cleanup.".to_string())
}

/// Brings this account's local state of a group conversation to the server's
/// setup generation. A takeover of an unfinished setup starts a new
/// generation and is always a reset, so when `generation` is newer than the
/// stored one the local MLS group, the staged membership update and the
/// pending bootstrap of the older setup are dropped, in one transaction with
/// the new generation. Returns whether that happened. An equal or older
/// value (a stale read) changes nothing, so a group joined in the current
/// generation is never dropped. The decrypted text cache is kept.
pub async fn apply_setup_generation(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    generation: i64,
) -> Result<bool, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    if generation < 0 {
        return Err("The encrypted setup generation is invalid.".into());
    }
    let error = || "GhostChat could not reset the superseded encrypted setup.".to_string();
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db.begin().await.map_err(|_| error())?;
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT generation FROM ghostchat_crypto_setup_generations WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| error())?
    .unwrap_or(0);
    if generation <= stored {
        return Ok(false);
    }
    let group_id = group_id_for_conversation(&conversation_id);
    {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        if let Some(mut group) =
            MlsGroup::load(provider.storage(), &group_id).map_err(|_| error())?
        {
            group.delete(provider.storage()).map_err(|_| error())?;
        }
    }
    for table in [
        "ghostchat_crypto_membership_outbox",
        "ghostchat_crypto_bootstrap",
    ] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE user_id=? AND conversation_id=?"
        ))
        .bind(&user_id)
        .bind(&conversation_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| error())?;
    }
    sqlx::query(
        "INSERT INTO ghostchat_crypto_setup_generations(user_id,conversation_id,generation) VALUES(?,?,?) \
         ON CONFLICT(user_id,conversation_id) DO UPDATE SET generation=excluded.generation",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(generation)
    .execute(&mut *transaction)
    .await
    .map_err(|_| error())?;
    transaction.commit().await.map_err(|_| error())?;
    Ok(true)
}

pub async fn finalize_group_membership_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    operation_id: &str,
) -> Result<String, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let operation_id = normalized_uuid(operation_id, "membership operation")?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not finalize the MLS membership update.".to_string())?;
    let payload = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load the pending MLS membership update.".to_string())?
    .ok_or_else(|| "There is no pending MLS membership update on this device.".to_string())?;
    let pending: GroupMembershipUpdate = EncryptedJsonCodec::from_slice(&payload)
        .map_err(|_| "The pending MLS membership update could not be decrypted.".to_string())?;
    if pending.operation_id != operation_id {
        return Err("The pending MLS membership operation does not match.".into());
    }
    let (device_id, public_key) = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT device_id,signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;

    let fingerprint = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let mut group = MlsGroup::load(
            provider.storage(),
            &group_id_for_conversation(&conversation_id),
        )
        .map_err(|_| "GhostChat could not load the encrypted MLS group.".to_string())?
        .ok_or_else(|| "The encrypted MLS group is missing on this device.".to_string())?;
        // A deleted-account removal leaves any other recorded deleted leaves
        // in place, so its result is the roster before the commit minus the
        // removed leaf, which must still contain every server member.
        let roster_before = group_account_identities(&group)
            .map(|identities| {
                identities
                    .into_keys()
                    .collect::<std::collections::HashSet<_>>()
            })
            .ok_or_else(|| {
                "The local MLS group contains invalid or duplicate identities.".to_string()
            })?;
        group.merge_pending_commit(&provider).map_err(|_| {
            "GhostChat could not confirm the local MLS membership update.".to_string()
        })?;
        let members = group.members().collect::<Vec<_>>();
        let mut expected = pending
            .expected_member_ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        match pending.action.as_str() {
            "add" => expected.extend(pending.member_ids.iter().cloned()),
            "remove" | "leave" => {
                for member_id in &pending.member_ids {
                    if !expected.remove(member_id) {
                        return Err("The removal does not match the saved MLS roster.".into());
                    }
                }
            }
            "remove_deleted" => {
                let mut result = roster_before.clone();
                for member_id in &pending.member_ids {
                    if expected.contains(member_id) || !result.remove(member_id) {
                        return Err("The removal does not match the saved MLS roster.".into());
                    }
                }
                if !expected.is_subset(&result) {
                    return Err("The removal does not match the saved MLS roster.".into());
                }
                expected = result;
            }
            _ => return Err("The saved MLS membership action is not supported.".into()),
        }
        let actual = members
            .iter()
            .filter(|member| member.credential.credential_type() == CredentialType::Basic)
            .filter_map(|member| account_id_from_credential(member.credential.serialized_content()))
            .collect::<std::collections::HashSet<_>>();
        let own_identity = format!("ghostchat:v1:{user_id}:{device_id}");
        if actual.len() != members.len()
            || actual != expected
            || !members.iter().any(|member| {
                member.credential.serialized_content() == own_identity.as_bytes()
                    && member.signature_key.as_slice() == signer.public()
            })
        {
            return Err("The resulting MLS group does not match the requested membership.".into());
        }
        group_identity_fingerprint(members.iter().map(|member| {
            (
                member.credential.serialized_content(),
                member.signature_key.as_slice(),
            )
        }))
    };
    sqlx::query(
        "DELETE FROM ghostchat_crypto_membership_outbox WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not clear the completed MLS membership update.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the updated MLS group.".to_string())?;
    Ok(fingerprint)
}

pub async fn get_pending_bootstrap(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<DirectGroupStart>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    let payload = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload FROM ghostchat_crypto_bootstrap WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not load the pending encrypted setup.".to_string())?;
    payload
        .map(|payload| {
            EncryptedJsonCodec::from_slice(&payload)
                .map_err(|_| "The pending encrypted setup could not be unlocked.".to_string())
        })
        .transpose()
}

pub async fn clear_pending_bootstrap(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<(), String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    sqlx::query("DELETE FROM ghostchat_crypto_bootstrap WHERE user_id=? AND conversation_id=?")
        .bind(user_id)
        .bind(conversation_id)
        .execute(db)
        .await
        .map_err(|_| "GhostChat could not clear the completed encrypted setup.".to_string())?;
    Ok(())
}

pub async fn encrypt_text(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    client_message_id: &str,
    content: &str,
) -> Result<EncryptedApplicationMessage, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let client_message_id = normalized_uuid(client_message_id, "message")?;
    if content.trim().is_empty() || content.len() > 2500 {
        return Err("Encrypted text must contain 1 to 2.5 KB of content.".into());
    }
    let cached_content = EncryptedJsonCodec::to_vec(&content.to_string())
        .map_err(|_| "GhostChat could not encrypt the local message cache.".to_string())?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start local encryption.".to_string())?;
    let public_key = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;

    let existing = cached_text_row(
        &mut *transaction,
        &user_id,
        &conversation_id,
        &user_id,
        &client_message_id,
    )
    .await?;
    if let Some(existing) = existing {
        let existing: String = EncryptedJsonCodec::from_slice(&existing)
            .map_err(|_| "The local encrypted message cache could not be decrypted.".to_string())?;
        if existing != content {
            return Err("This message ID is already associated with different content.".into());
        }
        let saved = sqlx::query_as::<_, (String, String, Vec<u8>)>(
            "SELECT user_id,conversation_id,encrypted_payload FROM ghostchat_crypto_application_outbox WHERE client_message_id=?",
        )
        .bind(&client_message_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| "GhostChat could not load the encrypted application outbox.".to_string())?
        .ok_or_else(|| "This older message has no safe retry record. Send it with a new message ID.".to_string())?;
        if saved.0 != user_id || saved.1 != conversation_id {
            return Err("This message ID belongs to another account or conversation.".into());
        }
        let payload = EncryptedJsonCodec::from_slice::<String>(&saved.2)
            .map_err(|_| "The encrypted application outbox could not be decrypted.".to_string())?;
        transaction
            .rollback()
            .await
            .map_err(|_| "GhostChat could not close the encrypted retry lookup.".to_string())?;
        return Ok(EncryptedApplicationMessage { payload });
    } else {
        let orphaned_outbox = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM ghostchat_crypto_application_outbox WHERE client_message_id=?)",
        )
        .bind(&client_message_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| "GhostChat could not validate the encrypted application outbox.".to_string())?;
        if orphaned_outbox != 0 {
            return Err("This message's encrypted retry record is incomplete.".into());
        }
        sqlx::query("INSERT INTO ghostchat_crypto_text_cache(user_id,conversation_id,sender_id,client_message_id,encrypted_content) VALUES(?,?,?,?,?)")
            .bind(&user_id)
            .bind(&conversation_id)
            .bind(&user_id)
            .bind(&client_message_id)
            .bind(cached_content)
            .execute(&mut *transaction)
            .await
            .map_err(|_| "GhostChat could not save the encrypted local message cache.".to_string())?;
    }

    let payload = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let group_id = group_id_for_conversation(&conversation_id);
        let mut group = MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the encrypted conversation.".to_string())?
            .ok_or_else(|| "Encryption has not been set up for this conversation.".to_string())?;
        let message = group
            .create_message(&provider, &signer, content.as_bytes())
            .map_err(|_| "GhostChat could not encrypt this message.".to_string())?;
        URL_SAFE_NO_PAD.encode(
            message
                .tls_serialize_detached()
                .map_err(|_| "GhostChat could not serialize the encrypted message.".to_string())?,
        )
    };
    let encrypted_payload = EncryptedJsonCodec::to_vec(&payload)
        .map_err(|_| "GhostChat could not protect the application outbox.".to_string())?;
    sqlx::query("INSERT INTO ghostchat_crypto_application_outbox(client_message_id,user_id,conversation_id,encrypted_payload) VALUES(?,?,?,?)")
        .bind(&client_message_id)
        .bind(&user_id)
        .bind(&conversation_id)
        .bind(encrypted_payload)
        .execute(&mut *transaction)
        .await
        .map_err(|_| "GhostChat could not save the encrypted application outbox.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the local encryption state.".to_string())?;
    Ok(EncryptedApplicationMessage { payload })
}

/// Encrypts a queued message again for the group's current epoch. The server
/// refused the earlier ciphertext because a membership commit was accepted
/// after it was made (devices that applied the commit could not read it).
/// The text comes from this account's own saved copy; the outbox keeps the
/// new ciphertext under the same message id, so a later retry sends it.
pub async fn reencrypt_text(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    client_message_id: &str,
) -> Result<EncryptedApplicationMessage, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let client_message_id = normalized_uuid(client_message_id, "message")?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start local encryption.".to_string())?;
    let public_key = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM ghostchat_crypto_application_outbox WHERE client_message_id=? AND user_id=? AND conversation_id=?)",
    )
    .bind(&client_message_id)
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load the encrypted application outbox.".to_string())?;
    let cached = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT encrypted_content FROM ghostchat_crypto_text_cache WHERE user_id=? AND conversation_id=? AND sender_id=? AND client_message_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(&user_id)
    .bind(&client_message_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load the encrypted local message cache.".to_string())?;
    let (true, Some(cached)) = (queued != 0, cached) else {
        return Err("This message is not waiting to be sent from this device.".into());
    };
    let content: String = EncryptedJsonCodec::from_slice(&cached)
        .map_err(|_| "The local encrypted message cache could not be decrypted.".to_string())?;
    let payload = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let signer = SignatureKeyPair::read(
            provider.storage(),
            &public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| {
            "The saved encryption identity is missing or cannot be decrypted.".to_string()
        })?;
        let group_id = group_id_for_conversation(&conversation_id);
        let mut group = MlsGroup::load(provider.storage(), &group_id)
            .map_err(|_| "GhostChat could not load the encrypted conversation.".to_string())?
            .ok_or_else(|| "Encryption has not been set up for this conversation.".to_string())?;
        let message = group
            .create_message(&provider, &signer, content.as_bytes())
            .map_err(|_| "GhostChat could not encrypt this message.".to_string())?;
        URL_SAFE_NO_PAD.encode(
            message
                .tls_serialize_detached()
                .map_err(|_| "GhostChat could not serialize the encrypted message.".to_string())?,
        )
    };
    let encrypted_payload = EncryptedJsonCodec::to_vec(&payload)
        .map_err(|_| "GhostChat could not protect the application outbox.".to_string())?;
    sqlx::query(
        "UPDATE ghostchat_crypto_application_outbox SET encrypted_payload=? WHERE client_message_id=? AND user_id=? AND conversation_id=?",
    )
    .bind(encrypted_payload)
    .bind(&client_message_id)
    .bind(&user_id)
    .bind(&conversation_id)
    .execute(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not save the encrypted application outbox.".to_string())?;
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the local encryption state.".to_string())?;
    Ok(EncryptedApplicationMessage { payload })
}

pub async fn get_cached_sent_text(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: &str,
) -> Result<Option<String>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let sender_id = normalized_uuid(sender_id, "sender account")?;
    let client_message_id = normalized_uuid(client_message_id, "message")?;
    prepare_crypto_storage(db).await?;
    let encrypted_content = cached_text_row(
        db,
        &user_id,
        &conversation_id,
        &sender_id,
        &client_message_id,
    )
    .await?;
    encrypted_content
        .map(|bytes| {
            EncryptedJsonCodec::from_slice(&bytes).map_err(|_| {
                "The local encrypted message cache could not be decrypted.".to_string()
            })
        })
        .transpose()
}

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CachedTextState {
    pub content: Option<String>,
    pub undecryptable: bool,
}

/// The cached plaintext of a row, or whether the row was found to be
/// permanently undecryptable on this device. With the row's `payload`, a
/// marker counts only for the payload it was written for, so a later
/// delivery of another payload for the same row is decoded normally.
pub async fn get_cached_text_state(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: &str,
    payload: Option<&str>,
) -> Result<CachedTextState, String> {
    let content =
        get_cached_sent_text(db, user_id, conversation_id, sender_id, client_message_id).await?;
    if content.is_some() {
        return Ok(CachedTextState {
            content,
            undecryptable: false,
        });
    }
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let error = || "GhostChat could not load the encrypted message cache.".to_string();
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let sender_id = normalized_uuid(sender_id, "sender account")?;
    let client_message_id = normalized_uuid(client_message_id, "message")?;
    let marker_hash = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload_hash FROM ghostchat_crypto_undecryptable_rows WHERE user_id=? AND conversation_id=? AND sender_id=? AND client_message_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .bind(&sender_id)
    .bind(&client_message_id)
    .fetch_optional(db)
    .await
    .map_err(|_| error())?;
    let undecryptable = match (marker_hash, payload) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(marker_hash), Some(payload)) => {
            let bytes = URL_SAFE_NO_PAD
                .decode(payload.as_bytes())
                .unwrap_or_default();
            let payload_hash = Sha256::digest(&bytes).to_vec();
            if !marker_hash.is_empty() {
                marker_hash == payload_hash
            } else {
                // A marker from before markers held a hash: it was written
                // because the row's payload named a past epoch. It keeps that
                // effect for a payload that still does, now bound to it, and
                // is dropped for one that does not.
                let past =
                    history_payload_is_past_epoch(db, &user_id, &conversation_id, &bytes).await?;
                let query = if past {
                    sqlx::query("UPDATE ghostchat_crypto_undecryptable_rows SET payload_hash=? WHERE user_id=? AND conversation_id=? AND sender_id=? AND client_message_id=?")
                        .bind(payload_hash)
                } else {
                    sqlx::query("DELETE FROM ghostchat_crypto_undecryptable_rows WHERE user_id=? AND conversation_id=? AND sender_id=? AND client_message_id=?")
                };
                query
                    .bind(&user_id)
                    .bind(&conversation_id)
                    .bind(&sender_id)
                    .bind(&client_message_id)
                    .execute(db)
                    .await
                    .map_err(|_| error())?;
                past
            }
        }
    };
    Ok(CachedTextState {
        content: None,
        undecryptable,
    })
}

/// Whether a history payload names an epoch that this device's group for the
/// conversation has left: the condition a past-epoch marker records.
async fn history_payload_is_past_epoch(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    bytes: &[u8],
) -> Result<bool, String> {
    let Ok(message) = MlsMessageIn::tls_deserialize_exact(bytes) else {
        return Ok(false);
    };
    let Ok(protocol_message) = message.try_into_protocol_message() else {
        return Ok(false);
    };
    let group_id = group_id_for_conversation(conversation_id);
    if protocol_message.group_id().as_slice() != group_id.as_slice() {
        return Ok(false);
    }
    let _storage_account = StorageAccountScope::enter(user_id);
    adopt_legacy_group(db, user_id, conversation_id).await?;
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())?;
    let provider = DesktopMlsProvider {
        crypto: RustCrypto::default(),
        storage: SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection),
    };
    let group = MlsGroup::load(provider.storage(), &group_id)
        .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())?;
    Ok(group.is_some_and(|group| protocol_message.epoch() < group.epoch()))
}

#[allow(clippy::too_many_arguments)]
pub async fn process_crypto_event(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: Option<&str>,
    event_sequence: Option<i64>,
    conversation_is_group: bool,
    expected_member_ids: &[String],
    payload: &str,
) -> Result<CryptoEventResult, String> {
    process_crypto_event_with_deleted(
        db,
        user_id,
        conversation_id,
        sender_id,
        client_message_id,
        event_sequence,
        conversation_is_group,
        expected_member_ids,
        &[],
        payload,
    )
    .await
}

/// `deleted_member_ids` are accounts the server recorded as deleted while
/// members of this MLS-locked group. Their leaves may remain in the MLS
/// roster on top of `expected_member_ids` until an owner commits their
/// removal; any other roster mismatch is still rejected.
#[allow(clippy::too_many_arguments)]
pub async fn process_crypto_event_with_deleted(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: Option<&str>,
    event_sequence: Option<i64>,
    conversation_is_group: bool,
    expected_member_ids: &[String],
    deleted_member_ids: &[String],
    payload: &str,
) -> Result<CryptoEventResult, String> {
    process_crypto_event_core(
        db,
        user_id,
        conversation_id,
        sender_id,
        client_message_id,
        event_sequence,
        conversation_is_group,
        Some(expected_member_ids),
        deleted_member_ids,
        payload,
        false,
    )
    .await
}

/// Decrypts an encrypted history row before the client applies pending
/// relay commits, so text sent in the current local epoch is cached before
/// that epoch is left (OpenMLS keeps no past-epoch secrets). The server
/// roster may already reflect the pending membership change, so the row is
/// checked against the roster this device accepted for its current epoch
/// (validated against the server roster when that epoch was entered)
/// instead. Only application messages from history are accepted, and the
/// relay cursor is never moved.
pub async fn decode_history_before_pending_commits(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: &str,
    conversation_is_group: bool,
    payload: &str,
) -> Result<CryptoEventResult, String> {
    process_crypto_event_core(
        db,
        user_id,
        conversation_id,
        sender_id,
        Some(client_message_id),
        None,
        conversation_is_group,
        None,
        &[],
        payload,
        false,
    )
    .await
}

/// Runs every check `process_crypto_event_with_deleted` runs on a relay event
/// (decryption, sender authentication, roster checks after the merge) inside
/// a transaction that is always rolled back: nothing is applied, cached or
/// moved. The client uses it to learn that the next relay event is a valid
/// commit from a current member before it decodes pending history rows.
#[allow(clippy::too_many_arguments)]
pub async fn inspect_crypto_event(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    event_sequence: i64,
    conversation_is_group: bool,
    expected_member_ids: &[String],
    deleted_member_ids: &[String],
    payload: &str,
) -> Result<CryptoEventResult, String> {
    process_crypto_event_core(
        db,
        user_id,
        conversation_id,
        sender_id,
        None,
        Some(event_sequence),
        conversation_is_group,
        Some(expected_member_ids),
        deleted_member_ids,
        payload,
        true,
    )
    .await
}

/// `expected_member_ids` is `None` only for
/// [`decode_history_before_pending_commits`].
#[allow(clippy::too_many_arguments)]
async fn process_crypto_event_core(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: Option<&str>,
    event_sequence: Option<i64>,
    conversation_is_group: bool,
    expected_member_ids: Option<&[String]>,
    deleted_member_ids: &[String],
    payload: &str,
    dry_run: bool,
) -> Result<CryptoEventResult, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let sender_id = normalized_uuid(sender_id, "sender account")?;
    // Crypto-relay events carry their relay sequence, which drives the local
    // cursor. Encrypted text from ordinary message history has no relay
    // sequence: it may only be an application message, and it never moves the
    // cursor (MLS itself rejects a replayed application ciphertext).
    if event_sequence.is_some_and(|sequence| sequence <= 0) {
        return Err("The encrypted event sequence is invalid.".into());
    }
    let from_history = event_sequence.is_none();
    let local_epoch_roster = expected_member_ids.is_none();
    if local_epoch_roster && !from_history {
        return Err("Only message history may be decoded before pending commits.".into());
    }
    let expected_member_ids = expected_member_ids
        .unwrap_or_default()
        .iter()
        .map(|member_id| normalized_uuid(member_id, "conversation member"))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    if !local_epoch_roster && (expected_member_ids.len() < 2 || expected_member_ids.len() > 16) {
        return Err("The expected MLS conversation roster is invalid.".into());
    }
    let deleted_member_ids = normalized_deleted_member_ids(deleted_member_ids)?;
    let client_message_id = client_message_id
        .map(|message_id| normalized_uuid(message_id, "message"))
        .transpose()?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|_| "The encrypted event has invalid encoding.".to_string())?;
    if bytes.is_empty() || bytes.len() > 3 * 1024 * 1024 {
        return Err("The encrypted event has an invalid size.".into());
    }
    let message = MlsMessageIn::tls_deserialize_exact(bytes.as_slice())
        .map_err(|_| "The encrypted event could not be decoded as MLS data.".to_string())?;
    let is_welcome = matches!(message.clone().extract(), MlsMessageBodyIn::Welcome(_));
    if from_history && is_welcome {
        return Err("An MLS invitation must arrive through the encrypted event stream.".into());
    }
    let group_id = group_id_for_conversation(&conversation_id);
    let mut transaction = db
        .begin()
        .await
        .map_err(|_| "GhostChat could not start local decryption.".to_string())?;
    let last_sequence = sqlx::query_scalar::<_, i64>(
        "SELECT sequence FROM ghostchat_crypto_event_cursors WHERE user_id=? AND conversation_id=?",
    )
    .bind(&user_id)
    .bind(&conversation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not check the local encrypted event cursor.".to_string())?;
    if last_sequence.is_some_and(|last| event_sequence.is_some_and(|sequence| sequence <= last)) {
        transaction.rollback().await.map_err(|_| {
            "GhostChat could not close the duplicate event transaction.".to_string()
        })?;
        return Ok(CryptoEventResult {
            kind: "duplicate".into(),
            content: None,
            peer_identity_fingerprint: None,
        });
    }
    let device_id = sqlx::query_scalar::<_, String>(
        "SELECT device_id FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?
    .ok_or_else(|| "Prepare this device for encryption in Settings first.".to_string())?;
    let own_identity = format!("ghostchat:v1:{user_id}:{device_id}");
    // Another sender's history row may have a legacy cache row; it answers
    // only when MLS cannot decode the row.
    let legacy_text = match client_message_id.as_deref() {
        Some(client_message_id) if from_history && sender_id != user_id => {
            legacy_text_row(&mut *transaction, &conversation_id, client_message_id).await?
        }
        _ => None,
    };
    let mut answer_from_legacy = false;

    if !is_welcome {
        let group_exists = {
            let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
            let provider = DesktopMlsProvider {
                crypto: RustCrypto::default(),
                storage,
            };
            MlsGroup::load(provider.storage(), &group_id)
                .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())?
                .is_some()
        };
        if !group_exists && from_history {
            if let (Some(legacy_text), Some(client_message_id)) =
                (legacy_text, client_message_id.as_deref())
            {
                transaction.rollback().await.map_err(|_| {
                    "GhostChat could not close the local decryption transaction.".to_string()
                })?;
                return claim_legacy_text(
                    db,
                    &user_id,
                    &conversation_id,
                    &sender_id,
                    client_message_id,
                    legacy_text,
                    dry_run,
                )
                .await;
            }
            return Err("Encryption has not been set up for this conversation.".into());
        }
        if !group_exists && dry_run {
            return Ok(CryptoEventResult {
                kind: "pending_group".into(),
                content: None,
                peer_identity_fingerprint: None,
            });
        }
        if !group_exists {
            sqlx::query(
                "INSERT INTO ghostchat_crypto_event_cursors(user_id,conversation_id,sequence) VALUES(?,?,?) \
                 ON CONFLICT(user_id,conversation_id) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
            )
            .bind(&user_id)
            .bind(&conversation_id)
            .bind(event_sequence)
            .execute(&mut *transaction)
            .await
            .map_err(|_| "GhostChat could not advance the local encrypted event cursor.".to_string())?;
            transaction.commit().await.map_err(|_| {
                "GhostChat could not save the local encrypted event cursor.".to_string()
            })?;
            return Ok(CryptoEventResult {
                kind: "pending_group".into(),
                content: None,
                peer_identity_fingerprint: None,
            });
        }
    }

    let mut cache_content = None;
    let result = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *transaction);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        match message.clone().extract() {
            MlsMessageBodyIn::Welcome(welcome) => {
                if MlsGroup::load(provider.storage(), &group_id)
                    .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())?
                    .is_some()
                {
                    CryptoEventResult {
                        kind: "welcome_duplicate".into(),
                        content: None,
                        peer_identity_fingerprint: None,
                    }
                } else {
                    let staged = StagedWelcome::new_from_welcome(
                        &provider,
                        &MlsGroupJoinConfig::default(),
                        welcome,
                        None,
                    )
                    .map_err(|_| {
                        "This MLS invitation is invalid or does not belong to this device."
                            .to_string()
                    })?;
                    if staged.group_context().group_id().as_slice() != group_id.as_slice() {
                        return Err(
                            "The MLS invitation belongs to a different conversation.".into()
                        );
                    }
                    let expected_sender_prefix = format!("ghostchat:v1:{sender_id}:");
                    let members = staged.members().collect::<Vec<_>>();
                    let local_member = members.iter().any(|member| {
                        member.credential.serialized_content() == own_identity.as_bytes()
                    });
                    let actual_member_ids = members
                        .iter()
                        .filter(|member| {
                            member.credential.credential_type() == CredentialType::Basic
                        })
                        .filter_map(|member| {
                            account_id_from_credential(member.credential.serialized_content())
                        })
                        .collect::<std::collections::HashSet<_>>();
                    let roster_matches = members.len() == expected_member_ids.len()
                        && actual_member_ids.len() == expected_member_ids.len()
                        && actual_member_ids == expected_member_ids;
                    let sender_member = members.iter().find(|member| {
                        member.credential.credential_type() == CredentialType::Basic
                            && member
                                .credential
                                .serialized_content()
                                .starts_with(expected_sender_prefix.as_bytes())
                    });
                    let Some(sender_member) = sender_member else {
                        return Err(
                            "The MLS invitation was not created by the account that sent it."
                                .into(),
                        );
                    };
                    if account_id_from_credential(sender_member.credential.serialized_content())
                        != Some(sender_id.clone())
                        || !local_member
                        || !roster_matches
                    {
                        return Err(
                            "This MLS Welcome does not match the current conversation roster."
                                .into(),
                        );
                    }
                    let peer_identity_fingerprint = if conversation_is_group {
                        Some(group_identity_fingerprint(members.iter().map(|member| {
                            (
                                member.credential.serialized_content(),
                                member.signature_key.as_slice(),
                            )
                        })))
                    } else {
                        Some(key_fingerprint(&sender_member.signature_key))
                    };
                    staged.into_group(&provider).map_err(|_| {
                        "GhostChat could not join the encrypted direct conversation.".to_string()
                    })?;
                    CryptoEventResult {
                        kind: "welcome".into(),
                        content: None,
                        peer_identity_fingerprint,
                    }
                }
            }
            _ => 'event: {
                let Some(mut group) =
                    MlsGroup::load(provider.storage(), &group_id).map_err(|_| {
                        "GhostChat could not load the encrypted conversation.".to_string()
                    })?
                else {
                    return Err(
                        "The local MLS group state disappeared while processing an event.".into(),
                    );
                };
                let existing_group_identities =
                    group_account_identities(&group).ok_or_else(|| {
                        "The local MLS group contains invalid or duplicate identities.".to_string()
                    })?;
                // A deleted account's leaf may stay in the group or be removed
                // by a commit, but never be added: only leaves already present
                // before this event are tolerated.
                let deleted_present_before = deleted_member_ids
                    .iter()
                    .filter(|user_id| existing_group_identities.contains_key(*user_id))
                    .cloned()
                    .collect::<std::collections::HashSet<_>>();
                let protocol_message = message.try_into_protocol_message().map_err(|_| {
                    "This MLS event is not a processable group message.".to_string()
                })?;
                if protocol_message.group_id().as_slice() != group_id.as_slice() {
                    return Err("The MLS event belongs to a different conversation.".into());
                }
                // OpenMLS keeps no past-epoch secrets: a history row from an
                // epoch this group has left can never be decrypted here.
                if from_history && protocol_message.epoch() < group.epoch() {
                    answer_from_legacy = legacy_text.is_some();
                    break 'event CryptoEventResult {
                        kind: "past_epoch".into(),
                        content: None,
                        peer_identity_fingerprint: None,
                    };
                }
                let processed = match group.process_message(&provider, protocol_message) {
                    Ok(processed) => processed,
                    // The row's message secret was already used up on this
                    // device: it was decoded before, and only its legacy
                    // cache row still holds its text.
                    Err(ProcessMessageError::ValidationError(
                        ValidationError::UnableToDecrypt(MessageDecryptionError::SecretTreeError(
                            SecretTreeError::SecretReuseError
                            | SecretTreeError::TooDistantInThePast,
                        )),
                    )) if legacy_text.is_some() => {
                        answer_from_legacy = true;
                        break 'event CryptoEventResult {
                            kind: "legacy_cache".into(),
                            content: None,
                            peer_identity_fingerprint: None,
                        };
                    }
                    Err(_) => {
                        return Err(
                            "GhostChat could not authenticate or decrypt this MLS event.".into(),
                        )
                    }
                };
                if account_id_from_credential(processed.credential().serialized_content())
                    != Some(sender_id.clone())
                {
                    return Err("The MLS sender identity does not match the relay account.".into());
                }
                if from_history
                    && !matches!(
                        processed.content(),
                        ProcessedMessageContent::ApplicationMessage(_)
                    )
                {
                    return Err(
                        "Only encrypted text may arrive through the message history.".into(),
                    );
                }
                match processed.into_content() {
                    ProcessedMessageContent::ApplicationMessage(application) => {
                        let roster_ok = if local_epoch_roster {
                            group_account_identities(&group).is_some_and(|identities| {
                                identities
                                    .values()
                                    .any(|(credential, _)| credential == own_identity.as_bytes())
                            })
                        } else {
                            group_roster_matches(
                                &group,
                                &expected_member_ids,
                                &deleted_member_ids,
                                &own_identity,
                            )
                        };
                        if !roster_ok
                            || !existing_group_identities_unchanged(
                                &group,
                                &existing_group_identities,
                            )
                        {
                            return Err(
                                "The encrypted group does not match the current server roster."
                                    .into(),
                            );
                        }
                        let content =
                            String::from_utf8(application.into_bytes()).map_err(|_| {
                                "The encrypted message is not valid UTF-8 text.".to_string()
                            })?;
                        cache_content = Some(content.clone());
                        CryptoEventResult {
                            kind: "application".into(),
                            content: Some(content),
                            peer_identity_fingerprint: None,
                        }
                    }
                    ProcessedMessageContent::StagedCommitMessage(commit) => {
                        group.merge_staged_commit(&provider, *commit).map_err(|_| {
                            "GhostChat could not apply the authenticated MLS membership update."
                                .to_string()
                        })?;
                        if !group_roster_matches(
                            &group,
                            &expected_member_ids,
                            &deleted_present_before,
                            &own_identity,
                        ) || !existing_group_identities_unchanged(
                            &group,
                            &existing_group_identities,
                        ) {
                            return Err(
                                "The MLS commit does not match the current server roster.".into()
                            );
                        }
                        CryptoEventResult {
                            kind: "commit".into(),
                            content: None,
                            peer_identity_fingerprint: None,
                        }
                    }
                    ProcessedMessageContent::OwnPendingCommit => {
                        group.merge_pending_commit(&provider).map_err(|_| {
                            "GhostChat could not confirm the local MLS membership update."
                                .to_string()
                        })?;
                        if !group_roster_matches(
                            &group,
                            &expected_member_ids,
                            &deleted_present_before,
                            &own_identity,
                        ) {
                            return Err(
                                "The MLS commit does not match the current server roster.".into()
                            );
                        }
                        CryptoEventResult {
                            kind: "commit".into(),
                            content: None,
                            peer_identity_fingerprint: None,
                        }
                    }
                    ProcessedMessageContent::OwnPrivateMessage => CryptoEventResult {
                        kind: "own_message".into(),
                        content: None,
                        peer_identity_fingerprint: None,
                    },
                    ProcessedMessageContent::ProposalMessage(proposal) => {
                        let Some(operation_id) = client_message_id.as_deref() else {
                            return Err(
                                "The MLS leave proposal is missing its operation ID.".into()
                            );
                        };
                        normalized_uuid(operation_id, "leave operation")?;
                        let sender_index = match proposal.sender() {
                            Sender::Member(index) => *index,
                            _ => {
                                return Err("Only an MLS group member can request to leave.".into())
                            }
                        };
                        let is_self_removal = matches!(
                            (proposal.proposal(), proposal.sender()),
                            (Proposal::Remove(remove), Sender::Member(index))
                                if remove.removed() == *index
                        );
                        let sender_is_rostered = group.members().any(|member| {
                            member.index == sender_index
                                && account_id_from_credential(
                                    member.credential.serialized_content(),
                                ) == Some(sender_id.clone())
                        });
                        let local_roster = existing_group_identities
                            .keys()
                            .cloned()
                            .collect::<std::collections::HashSet<_>>();
                        let mut server_roster_with_leaver = expected_member_ids.clone();
                        server_roster_with_leaver.insert(sender_id.clone());
                        let roster_matches_leave_timing = expected_member_ids == local_roster
                            || server_roster_with_leaver == local_roster;
                        if !conversation_is_group
                            || !is_self_removal
                            || !sender_is_rostered
                            || !expected_member_ids.contains(&user_id)
                            || !roster_matches_leave_timing
                        {
                            return Err(
                                "This MLS proposal is not a valid self-leave request.".into()
                            );
                        }
                        group
                            .store_pending_proposal(provider.storage(), *proposal)
                            .map_err(|_| {
                                "GhostChat could not save the pending MLS leave request."
                                    .to_string()
                            })?;
                        CryptoEventResult {
                            kind: "leave_request".into(),
                            content: Some(operation_id.to_string()),
                            peer_identity_fingerprint: None,
                        }
                    }
                    _ => {
                        return Err(
                            "This MLS proposal type is not supported by GhostChat yet.".into()
                        )
                    }
                }
            }
        }
    };
    if answer_from_legacy {
        if let (Some(legacy_text), Some(client_message_id)) =
            (legacy_text, client_message_id.as_deref())
        {
            // Nothing of the failed MLS decode is kept.
            transaction.rollback().await.map_err(|_| {
                "GhostChat could not close the local decryption transaction.".to_string()
            })?;
            return claim_legacy_text(
                db,
                &user_id,
                &conversation_id,
                &sender_id,
                client_message_id,
                legacy_text,
                dry_run,
            )
            .await;
        }
    }
    if result.kind == "past_epoch" {
        if let (false, Some(client_message_id)) = (dry_run, client_message_id.as_deref()) {
            sqlx::query(
                "INSERT INTO ghostchat_crypto_undecryptable_rows(user_id,conversation_id,sender_id,client_message_id,payload_hash) VALUES(?,?,?,?,?) \
                 ON CONFLICT(user_id,conversation_id,sender_id,client_message_id) DO UPDATE SET payload_hash=excluded.payload_hash",
            )
                .bind(&user_id)
                .bind(&conversation_id)
                .bind(&sender_id)
                .bind(client_message_id)
                .bind(Sha256::digest(&bytes).to_vec())
                .execute(&mut *transaction)
                .await
                .map_err(|_| "GhostChat could not save the encrypted message cache.".to_string())?;
            transaction
                .commit()
                .await
                .map_err(|_| "GhostChat could not save the encrypted message cache.".to_string())?;
        }
        return Err("This message is from an earlier encryption epoch and can no longer be decrypted on this device.".into());
    }
    if dry_run {
        transaction
            .rollback()
            .await
            .map_err(|_| "GhostChat could not close the inspection transaction.".to_string())?;
        return Ok(result);
    }
    if let (Some(client_message_id), Some(content)) =
        (client_message_id.as_deref(), cache_content.as_deref())
    {
        let encrypted_content = EncryptedJsonCodec::to_vec(content)
            .map_err(|_| "GhostChat could not encrypt the local message cache.".to_string())?;
        sqlx::query("INSERT OR IGNORE INTO ghostchat_crypto_text_cache(user_id,conversation_id,sender_id,client_message_id,encrypted_content) VALUES(?,?,?,?,?)")
            .bind(&user_id)
            .bind(&conversation_id)
            .bind(&sender_id)
            .bind(client_message_id)
            .bind(encrypted_content)
            .execute(&mut *transaction)
            .await
            .map_err(|_| "GhostChat could not save the encrypted local message cache.".to_string())?;
    }
    if let Some(event_sequence) = event_sequence {
        sqlx::query(
            "INSERT INTO ghostchat_crypto_event_cursors(user_id,conversation_id,sequence) VALUES(?,?,?) \
             ON CONFLICT(user_id,conversation_id) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
        )
        .bind(&user_id)
        .bind(&conversation_id)
        .bind(event_sequence)
        .execute(&mut *transaction)
        .await
        .map_err(|_| "GhostChat could not save the local encrypted event cursor.".to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the local MLS state.".to_string())?;
    Ok(result)
}

pub async fn has_direct_group(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<bool, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let local_identity_exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_one(db)
    .await
    .map_err(|_| "GhostChat could not check this device's encryption identity.".to_string())?;
    if local_identity_exists == 0 {
        return Err("Prepare this device for encryption in Settings first.".into());
    }
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection);
    let provider = DesktopMlsProvider {
        crypto: RustCrypto::default(),
        storage,
    };
    MlsGroup::load(
        provider.storage(),
        &group_id_for_conversation(&conversation_id),
    )
    .map(|group| group.is_some())
    .map_err(|_| "GhostChat could not load the local MLS group state.".to_string())
}

pub async fn direct_peer_fingerprint(
    db: &SqlitePool,
    user_id: &str,
    peer_user_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let peer_user_id = normalized_uuid(peer_user_id, "peer account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    if user_id == peer_user_id {
        return Err("An encrypted direct conversation needs two different accounts.".into());
    }
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let device_id = sqlx::query_scalar::<_, String>(
        "SELECT device_id FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?;
    let Some(device_id) = device_id else {
        return Ok(None);
    };
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection);
    let provider = DesktopMlsProvider {
        crypto: RustCrypto::default(),
        storage,
    };
    let Some(group) = MlsGroup::load(
        provider.storage(),
        &group_id_for_conversation(&conversation_id),
    )
    .map_err(|_| "GhostChat could not load the encrypted conversation.".to_string())?
    else {
        return Ok(None);
    };
    let expected_local = format!("ghostchat:v1:{user_id}:{device_id}");
    let expected_peer_prefix = format!("ghostchat:v1:{peer_user_id}:");
    let members = group.members().collect::<Vec<_>>();
    if members.len() != 2
        || !members
            .iter()
            .any(|member| member.credential.serialized_content() == expected_local.as_bytes())
    {
        return Err("This MLS group does not match the expected direct conversation.".into());
    }
    let Some(peer) = members.iter().find(|member| {
        member.credential.credential_type() == CredentialType::Basic
            && member
                .credential
                .serialized_content()
                .starts_with(expected_peer_prefix.as_bytes())
    }) else {
        return Err("The peer identity is missing from this MLS conversation.".into());
    };
    Ok(Some(key_fingerprint(&peer.signature_key)))
}

pub async fn group_fingerprint(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    let _storage_account = StorageAccountScope::enter(&user_id);
    adopt_legacy_group(db, &user_id, &conversation_id).await?;
    let device_id = sqlx::query_scalar::<_, String>(
        "SELECT device_id FROM ghostchat_crypto_identities WHERE user_id=?",
    )
    .bind(&user_id)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not load this device's encryption identity.".to_string())?;
    let Some(device_id) = device_id else {
        return Ok(None);
    };
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection);
    let provider = DesktopMlsProvider {
        crypto: RustCrypto::default(),
        storage,
    };
    let Some(group) = MlsGroup::load(
        provider.storage(),
        &group_id_for_conversation(&conversation_id),
    )
    .map_err(|_| "GhostChat could not load the encrypted MLS group.".to_string())?
    else {
        return Ok(None);
    };
    let members = group.members().collect::<Vec<_>>();
    if members.len() < 2
        || !members.iter().any(|member| {
            member.credential.serialized_content()
                == format!("ghostchat:v1:{user_id}:{device_id}").as_bytes()
        })
    {
        return Err("The local MLS group has an invalid member list.".into());
    }
    Ok(Some(group_identity_fingerprint(members.iter().map(
        |member| {
            (
                member.credential.serialized_content(),
                member.signature_key.as_slice(),
            )
        },
    ))))
}

struct DesktopMlsProvider<'a> {
    crypto: RustCrypto,
    storage: SqliteStorageProvider<'a, EncryptedJsonCodec>,
}

impl<'a> OpenMlsProvider for DesktopMlsProvider<'a> {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = SqliteStorageProvider<'a, EncryptedJsonCodec>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

#[derive(Debug)]
struct CryptoCodecError(String);

impl fmt::Display for CryptoCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CryptoCodecError {}

#[derive(Default)]
struct EncryptedJsonCodec;

impl Codec for EncryptedJsonCodec {
    type Error = CryptoCodecError;

    fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Self::Error> {
        let key = STORAGE_KEY
            .get()
            .ok_or_else(|| CryptoCodecError("Local storage key is unavailable".into()))?;
        let mut plaintext = serde_json::to_vec(value)
            .map_err(|error| CryptoCodecError(format!("Could not encode MLS data: {error}")))?;
        // OpenMLS uses the same Codec for both stored entities and serialized
        // lookup keys. Randomized encryption is correct for entities, but makes
        // SQLite equality lookups miss every time. Store those public lookup
        // references as a keyed, deterministic digest instead; they are never
        // decoded from the database and this does not expose their raw values.
        if std::any::type_name::<T>() == "openmls::group::GroupId" {
            if let Some(account) = current_storage_account() {
                let encoded = account_group_lookup_key(key, &account, &plaintext);
                plaintext.zeroize();
                return encoded;
            }
        }
        if is_mls_storage_lookup_key(std::any::type_name::<T>()) {
            let encoded = lookup_key_v1(key, &plaintext);
            plaintext.zeroize();
            return encoded;
        }
        if is_decodable_mls_storage_lookup_key(std::any::type_name::<T>()) {
            let encoded = encrypt_decodable_lookup_key(key, &plaintext);
            plaintext.zeroize();
            return encoded;
        }
        let cipher = Aes256Gcm::new_from_slice(&**key)
            .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = match cipher.encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_ref(),
                aad: b"GC\x01",
            },
        ) {
            Ok(ciphertext) => ciphertext,
            Err(_) => {
                plaintext.zeroize();
                return Err(CryptoCodecError("Could not encrypt local MLS data".into()));
            }
        };
        plaintext.zeroize();
        let mut encoded = Vec::with_capacity(3 + nonce.len() + ciphertext.len());
        encoded.extend_from_slice(b"GC\x01");
        encoded.extend_from_slice(&nonce);
        encoded.extend_from_slice(&ciphertext);
        Ok(encoded)
    }

    fn from_slice<T: DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        if slice.starts_with(DECODABLE_LOOKUP_KEY_PREFIX) {
            let key = STORAGE_KEY
                .get()
                .ok_or_else(|| CryptoCodecError("Local storage key is unavailable".into()))?;
            let plaintext = decrypt_decodable_lookup_key(key, slice)?;
            return serde_json::from_slice(&plaintext)
                .map_err(|error| CryptoCodecError(format!("Could not decode MLS data: {error}")));
        }
        if slice.len() < 3 + 12 + 16 || &slice[..3] != b"GC\x01" {
            return Err(CryptoCodecError(
                "Encrypted MLS data has an invalid format".into(),
            ));
        }
        let key = STORAGE_KEY
            .get()
            .ok_or_else(|| CryptoCodecError("Local storage key is unavailable".into()))?;
        let cipher = Aes256Gcm::new_from_slice(&**key)
            .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
        let nonce = Nonce::from_slice(&slice[3..15]);
        let mut plaintext = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &slice[15..],
                    aad: b"GC\x01",
                },
            )
            .map_err(|_| CryptoCodecError("Could not decrypt local MLS data".into()))?;
        let decoded = serde_json::from_slice(&plaintext)
            .map_err(|error| CryptoCodecError(format!("Could not decode MLS data: {error}")));
        plaintext.zeroize();
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
        SqlitePool,
    };
    use std::path::{Path, PathBuf};

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn open_database(path: &Path) -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
            .expect("test database should open")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_account_direct_mls_survives_restart_and_deduplicates_replayed_events() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_path = root.join("alice.sqlite");
        let bob_path = root.join("bob.sqlite");
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let expected_members = vec![alice_id.clone(), bob_id.clone()];

        let alice_db = open_database(&alice_path).await;
        let bob_db = open_database(&bob_path).await;
        let _alice_device = prepare_device(&alice_db, &alice_id)
            .await
            .expect("Alice should prepare an MLS identity");
        let bob_device = prepare_device(&bob_db, &bob_id)
            .await
            .expect("Bob should prepare an MLS identity");
        let alice_public_key = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT signing_public_key FROM ghostchat_crypto_identities WHERE user_id=?",
        )
        .bind(&alice_id)
        .fetch_one(&alice_db)
        .await
        .expect("Alice's public key should be saved");
        let stored_signer_bytes = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT signature_key FROM openmls_signature_key LIMIT 1",
        )
        .fetch_one(&alice_db)
        .await
        .expect("Alice's signer should be saved in MLS storage");
        let stored_signer: SignatureKeyPair = EncryptedJsonCodec::from_slice(&stored_signer_bytes)
            .expect("Alice's stored signer should decrypt");
        assert_eq!(stored_signer.public(), alice_public_key);
        assert_eq!(
            stored_signer.signature_scheme(),
            MLS_CIPHERSUITE.signature_algorithm()
        );
        let mut alice_connection = alice_db
            .acquire()
            .await
            .expect("Alice's MLS database should be available");
        let alice_storage =
            SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *alice_connection);
        assert!(SignatureKeyPair::read(
            &alice_storage,
            &alice_public_key,
            MLS_CIPHERSUITE.signature_algorithm(),
        )
        .is_some());
        drop(alice_storage);
        drop(alice_connection);

        let welcome = create_direct_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            &bob_device.device_id,
            &bob_device.payload,
        )
        .await
        .expect("Alice should create a direct MLS group");
        let welcome_result = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            false,
            &expected_members,
            &welcome.welcome_payload,
        )
        .await
        .expect("Bob should accept the matching MLS Welcome");
        assert_eq!(welcome_result.kind, "welcome");
        assert!(welcome_result.peer_identity_fingerprint.is_some());

        let first_id = Uuid::new_v4().to_string();
        let first_text = "two-account MLS round trip";
        let first = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &first_id,
            first_text,
        )
        .await
        .expect("Alice should encrypt a direct message");
        assert!(!first.payload.contains(first_text));

        drop(alice_db);
        drop(bob_db);
        let alice_db = open_database(&alice_path).await;
        let bob_db = open_database(&bob_path).await;
        let retried = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &first_id,
            first_text,
        )
        .await
        .expect("A retry after restart should reuse the committed ciphertext");
        assert_eq!(retried.payload, first.payload);
        assert!(encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &first_id,
            "different content",
        )
        .await
        .is_err());
        assert!(encrypt_text(
            &alice_db,
            &alice_id,
            &Uuid::new_v4().to_string(),
            &first_id,
            first_text,
        )
        .await
        .is_err());
        assert_eq!(
            get_cached_sent_text(&alice_db, &alice_id, &conversation_id, &alice_id, &first_id)
                .await
                .expect("Alice's encrypted cache should reopen")
                .as_deref(),
            Some(first_text)
        );
        let first_received = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&first_id),
            Some(2),
            false,
            &expected_members,
            &first.payload,
        )
        .await
        .expect("Bob should decrypt Alice's message after reopening");
        assert_eq!(first_received.kind, "application");
        assert_eq!(first_received.content.as_deref(), Some(first_text));
        assert_eq!(
            get_crypto_event_cursor(&bob_db, &bob_id, &conversation_id)
                .await
                .expect("Bob's event cursor should be readable"),
            Some(2)
        );

        drop(bob_db);
        let bob_db = open_database(&bob_path).await;
        let replayed = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&first_id),
            Some(2),
            false,
            &expected_members,
            &first.payload,
        )
        .await
        .expect("A replayed event should be recognized after restart");
        assert_eq!(replayed.kind, "duplicate");
        assert!(replayed.content.is_none());

        drop(alice_db);
        let alice_db = open_database(&alice_path).await;
        let second_id = Uuid::new_v4().to_string();
        let second_text = "MLS state also survives restart";
        let second = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &second_id,
            second_text,
        )
        .await
        .expect("Alice should keep sending after her MLS state reopens");
        let second_received = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&second_id),
            Some(3),
            false,
            &expected_members,
            &second.payload,
        )
        .await
        .expect("Bob should decrypt the next message after restart and replay");
        assert_eq!(second_received.kind, "application");
        assert_eq!(second_received.content.as_deref(), Some(second_text));

        // A failed outbox write must roll back the MLS sender state and cache too.
        sqlx::query("CREATE TRIGGER fail_application_outbox BEFORE INSERT ON ghostchat_crypto_application_outbox BEGIN SELECT RAISE(ABORT, 'injected outbox failure'); END")
            .execute(&alice_db).await.unwrap();
        let third_id = Uuid::new_v4().to_string();
        assert!(encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &third_id,
            "atomic retry"
        )
        .await
        .is_err());
        assert!(
            get_cached_sent_text(&alice_db, &alice_id, &conversation_id, &alice_id, &third_id)
                .await
                .unwrap()
                .is_none()
        );
        sqlx::query("DROP TRIGGER fail_application_outbox")
            .execute(&alice_db)
            .await
            .unwrap();
        let third = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &third_id,
            "atomic retry",
        )
        .await
        .expect("Retry after rollback should encrypt successfully");
        let third_received = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&third_id),
            Some(4),
            false,
            &expected_members,
            &third.payload,
        )
        .await
        .expect("Receiver should decrypt after the failed transaction");
        assert_eq!(third_received.content.as_deref(), Some("atomic retry"));

        let legacy_id = Uuid::new_v4().to_string();
        // A legacy row (from before the cache was keyed by account and
        // sender) has an empty account and sender.
        sqlx::query("INSERT INTO ghostchat_crypto_text_cache(user_id,conversation_id,sender_id,client_message_id,encrypted_content) VALUES('',?,'',?,?)")
            .bind(&conversation_id).bind(&legacy_id)
            .bind(EncryptedJsonCodec::to_vec(&"legacy message".to_string()).unwrap())
            .execute(&alice_db).await.unwrap();
        let legacy_error = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &legacy_id,
            "legacy message",
        )
        .await
        .err()
        .expect("Legacy entries without ciphertext cannot be regenerated safely");
        assert!(legacy_error.contains("no safe retry record"));

        sqlx::query("UPDATE ghostchat_crypto_application_outbox SET encrypted_payload=? WHERE client_message_id=?")
            .bind(vec![0_u8; 16]).bind(&third_id).execute(&alice_db).await.unwrap();
        assert!(encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &third_id,
            "atomic retry"
        )
        .await
        .is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_accounts_on_one_profile_both_join_a_group_and_legacy_groups_still_load() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-profile-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        // Bob and Carol use the same desktop profile (one local database).
        let shared_db = open_database(&root.join("shared.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();

        // A group joined by Bob with the storage layout of earlier builds.
        let legacy_conversation = Uuid::new_v4().to_string();
        let mut pair = vec![alice_id.clone(), bob_id.clone()];
        pair.sort();
        use_legacy_storage_layout(&bob_id, true);
        let bob_device = prepare_device(&shared_db, &bob_id).await.unwrap();
        let legacy_start = create_direct_group(
            &alice_db,
            &alice_id,
            &legacy_conversation,
            &bob_id,
            &bob_device.device_id,
            &bob_device.payload,
        )
        .await
        .unwrap();
        process_crypto_event(
            &shared_db,
            &bob_id,
            &legacy_conversation,
            &alice_id,
            None,
            Some(1),
            false,
            &pair,
            &legacy_start.welcome_payload,
        )
        .await
        .unwrap();
        use_legacy_storage_layout(&bob_id, false);
        let old_text = encrypt_text(
            &alice_db,
            &alice_id,
            &legacy_conversation,
            &Uuid::new_v4().to_string(),
            "from an earlier build",
        )
        .await
        .unwrap();
        assert!(has_direct_group(&shared_db, &bob_id, &legacy_conversation)
            .await
            .unwrap());
        let decoded = process_crypto_event(
            &shared_db,
            &bob_id,
            &legacy_conversation,
            &alice_id,
            None,
            Some(2),
            false,
            &pair,
            &old_text.payload,
        )
        .await
        .expect("a group stored by an earlier build stays usable");
        assert_eq!(decoded.content.as_deref(), Some("from an earlier build"));

        // Bob and Carol, both on the shared profile, join the same group.
        let conversation_id = Uuid::new_v4().to_string();
        let bob_package = prepare_device(&shared_db, &bob_id).await.unwrap();
        let carol_package = prepare_device(&shared_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_package.device_id.clone(),
                    payload: bob_package.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_package.device_id.clone(),
                    payload: carol_package.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        for id in [&bob_id, &carol_id] {
            let joined = process_crypto_event(
                &shared_db,
                id,
                &conversation_id,
                &alice_id,
                None,
                Some(1),
                true,
                &roster,
                &start.welcome_payload,
            )
            .await
            .unwrap();
            assert_eq!(
                joined.kind, "welcome",
                "account {id} joins on the shared profile"
            );
        }
        let sent = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &Uuid::new_v4().to_string(),
            "hello both",
        )
        .await
        .unwrap();
        for id in [&bob_id, &carol_id] {
            let received = process_crypto_event(
                &shared_db,
                id,
                &conversation_id,
                &alice_id,
                None,
                Some(2),
                true,
                &roster,
                &sent.payload,
            )
            .await
            .unwrap();
            assert_eq!(received.content.as_deref(), Some("hello both"));
        }
        // Carol never sees Bob's legacy group as hers.
        assert!(
            !has_direct_group(&shared_db, &carol_id, &legacy_conversation)
                .await
                .unwrap()
        );
    }

    /// The one-time copies taken before the account binding, by file name.
    fn binding_backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".bak"))
            .collect();
        names.sort();
        names
    }

    async fn legacy_group_rows(db: &SqlitePool, conversation_id: &str) -> i64 {
        let plaintext = serde_json::to_vec(&group_id_for_conversation(conversation_id)).unwrap();
        let legacy = lookup_key_v1(&get_storage_key().unwrap(), &plaintext).unwrap();
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM openmls_group_data WHERE group_id=?")
            .bind(legacy)
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn adopting_a_legacy_group_copies_the_profile_once_and_not_without_a_copy() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-adopt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut pair = vec![alice_id.clone(), bob_id.clone()];
        pair.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        use_legacy_storage_layout(&bob_id, true);
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let start = create_direct_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            &bob_device.device_id,
            &bob_device.payload,
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            false,
            &pair,
            &start.welcome_payload,
        )
        .await
        .unwrap();
        use_legacy_storage_layout(&bob_id, false);
        assert!(
            binding_backups(&root).is_empty(),
            "a fresh profile gets no copy"
        );
        let legacy_rows = legacy_group_rows(&bob_db, &conversation_id).await;
        assert!(legacy_rows > 0);

        // A copy that cannot be written stops the adoption: nothing moves.
        let blocker = root.join("bob.sqlite.pre-account-binding.bak.tmp");
        std::fs::create_dir_all(blocker.join("blocker")).unwrap();
        assert!(has_direct_group(&bob_db, &bob_id, &conversation_id)
            .await
            .is_err());
        assert_eq!(
            legacy_group_rows(&bob_db, &conversation_id).await,
            legacy_rows
        );
        assert!(binding_backups(&root).is_empty());
        std::fs::remove_dir_all(&blocker).unwrap();

        assert!(has_direct_group(&bob_db, &bob_id, &conversation_id)
            .await
            .unwrap());
        assert_eq!(legacy_group_rows(&bob_db, &conversation_id).await, 0);
        assert!(has_direct_group(&bob_db, &bob_id, &conversation_id)
            .await
            .unwrap());
        assert_eq!(
            binding_backups(&root),
            vec!["bob.sqlite.pre-account-binding.bak"]
        );
        let backup = open_database(&root.join("bob.sqlite.pre-account-binding.bak")).await;
        assert_eq!(
            legacy_group_rows(&backup, &conversation_id).await,
            legacy_rows,
            "the copy holds the group as the earlier build stored it"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pending_bootstrap_works_on_a_fresh_profile_before_device_setup() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-fresh-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let db = open_database(&root.join("fresh.sqlite")).await;
        let user_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        assert!(get_pending_bootstrap(&db, &user_id, &conversation_id)
            .await
            .expect("a fresh profile has no pending setup, not an error")
            .is_none());
        clear_pending_bootstrap(&db, &user_id, &conversation_id)
            .await
            .expect("clearing a missing setup on a fresh profile succeeds");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_cache_is_bound_to_account_conversation_and_sender() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-cache-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let mallory_db = open_database(&root.join("mallory.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let mallory_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), mallory_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let mut groups = Vec::new();
        for _ in 0..2 {
            let conversation_id = Uuid::new_v4().to_string();
            let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
            let mallory_device = prepare_device(&mallory_db, &mallory_id).await.unwrap();
            let start = create_group(
                &alice_db,
                &alice_id,
                &conversation_id,
                &[
                    GroupPeerKeyPackage {
                        user_id: bob_id.clone(),
                        device_id: bob_device.device_id.clone(),
                        payload: bob_device.payload.clone(),
                    },
                    GroupPeerKeyPackage {
                        user_id: mallory_id.clone(),
                        device_id: mallory_device.device_id.clone(),
                        payload: mallory_device.payload.clone(),
                    },
                ],
            )
            .await
            .unwrap();
            for (db, id) in [(&bob_db, &bob_id), (&mallory_db, &mallory_id)] {
                process_crypto_event(
                    db,
                    id,
                    &conversation_id,
                    &alice_id,
                    None,
                    Some(1),
                    true,
                    &roster,
                    &start.welcome_payload,
                )
                .await
                .unwrap();
            }
            groups.push(conversation_id);
        }
        let (first, second) = (&groups[0], &groups[1]);

        // Mallory learns the id of Alice's next message and pre-seeds Bob's
        // cache with it, in the same conversation and in another one.
        let message_id = Uuid::new_v4().to_string();
        for conversation_id in [first, second] {
            // A malicious client is not bound by its own local retry records.
            sqlx::query("DELETE FROM ghostchat_crypto_application_outbox")
                .execute(&mallory_db)
                .await
                .unwrap();
            let forged = encrypt_text(
                &mallory_db,
                &mallory_id,
                conversation_id,
                &message_id,
                "forged",
            )
            .await
            .unwrap();
            process_crypto_event(
                &bob_db,
                &bob_id,
                conversation_id,
                &mallory_id,
                Some(&message_id),
                None,
                true,
                &roster,
                &forged.payload,
            )
            .await
            .unwrap();
        }
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, first, &alice_id, &message_id)
                .await
                .unwrap(),
            None,
            "another sender's row must not answer for Alice's message"
        );
        let real = encrypt_text(&alice_db, &alice_id, first, &message_id, "real")
            .await
            .unwrap();
        let received = process_crypto_event(
            &bob_db,
            &bob_id,
            first,
            &alice_id,
            Some(&message_id),
            None,
            true,
            &roster,
            &real.payload,
        )
        .await
        .unwrap();
        assert_eq!(received.content.as_deref(), Some("real"));
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, first, &alice_id, &message_id)
                .await
                .unwrap()
                .as_deref(),
            Some("real")
        );
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, second, &mallory_id, &message_id)
                .await
                .unwrap()
                .as_deref(),
            Some("forged"),
            "each conversation keeps its own entry"
        );
        assert_eq!(
            get_cached_sent_text(&bob_db, &mallory_id, first, &mallory_id, &message_id)
                .await
                .unwrap(),
            None,
            "another account on this profile cannot read Bob's entry"
        );
        // Alice's own sent text is cached under her account as the sender.
        assert_eq!(
            get_cached_sent_text(&alice_db, &alice_id, first, &alice_id, &message_id)
                .await
                .unwrap()
                .as_deref(),
            Some("real")
        );

        // An existing profile's cache (keyed by client message id only) is
        // rebuilt; its rows stay readable, but only in their conversation.
        // A plain read answers from a legacy row only for the account's own
        // messages; another sender's legacy row answers once MLS cannot
        // decode that row (see legacy_text_cache_rows_answer_only_for_their_own_message).
        let legacy_id = Uuid::new_v4().to_string();
        sqlx::query("DROP TABLE ghostchat_crypto_text_cache")
            .execute(&bob_db)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE ghostchat_crypto_text_cache (client_message_id TEXT PRIMARY KEY NOT NULL, conversation_id TEXT NOT NULL, encrypted_content BLOB NOT NULL, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')))")
            .execute(&bob_db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO ghostchat_crypto_text_cache(client_message_id,conversation_id,encrypted_content) VALUES(?,?,?)")
            .bind(&legacy_id)
            .bind(first)
            .bind(EncryptedJsonCodec::to_vec(&"older text".to_string()).unwrap())
            .execute(&bob_db)
            .await
            .unwrap();
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, first, &bob_id, &legacy_id)
                .await
                .unwrap()
                .as_deref(),
            Some("older text")
        );
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, second, &bob_id, &legacy_id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, first, &alice_id, &legacy_id)
                .await
                .unwrap(),
            None,
            "nothing shows that this legacy row is Alice's"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_text_cache_rows_answer_only_for_their_own_message() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-legacy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let mallory_db = open_database(&root.join("mallory.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let mallory_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), mallory_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let mallory_device = prepare_device(&mallory_db, &mallory_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: mallory_id.clone(),
                    device_id: mallory_device.device_id.clone(),
                    payload: mallory_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        for (db, id) in [(&bob_db, &bob_id), (&mallory_db, &mallory_id)] {
            process_crypto_event(
                db,
                id,
                &conversation_id,
                &alice_id,
                None,
                Some(1),
                true,
                &roster,
                &start.welcome_payload,
            )
            .await
            .unwrap();
        }
        // Bob decodes a history row of `sender` with client id `id`.
        let history = |members: &[String], sender: &str, id: &str, payload: &str| {
            let (bob_db, bob_id, conversation_id) =
                (bob_db.clone(), bob_id.clone(), conversation_id.clone());
            let (members, sender, id, payload) = (
                members.to_vec(),
                sender.to_string(),
                id.to_string(),
                payload.to_string(),
            );
            async move {
                process_crypto_event(
                    &bob_db,
                    &bob_id,
                    &conversation_id,
                    &sender,
                    Some(&id),
                    None,
                    true,
                    &members,
                    &payload,
                )
                .await
                .map(|result| result.content)
            }
        };
        // Turns Bob's cached row into a row of the cache from before it was
        // bound to the account and the sender.
        let make_legacy = |id: &str| {
            let (bob_db, id) = (bob_db.clone(), id.to_string());
            async move {
                sqlx::query("UPDATE ghostchat_crypto_text_cache SET user_id='',sender_id='' WHERE client_message_id=?")
                    .bind(&id)
                    .execute(&bob_db)
                    .await
                    .unwrap();
            }
        };
        let old_id = Uuid::new_v4().to_string();
        let old = encrypt_text(&alice_db, &alice_id, &conversation_id, &old_id, "alice old")
            .await
            .unwrap();
        assert_eq!(
            history(&roster, &alice_id, &old_id, &old.payload)
                .await
                .unwrap()
                .as_deref(),
            Some("alice old")
        );
        make_legacy(&old_id).await;

        // Mallory reuses the client id of Alice's old message, and Bob reads
        // Mallory's message before he reads Alice's old one again.
        sqlx::query("DELETE FROM ghostchat_crypto_application_outbox")
            .execute(&mallory_db)
            .await
            .unwrap();
        let reused = encrypt_text(
            &mallory_db,
            &mallory_id,
            &conversation_id,
            &old_id,
            "mallory",
        )
        .await
        .unwrap();
        let state = get_cached_text_state(
            &bob_db,
            &bob_id,
            &conversation_id,
            &mallory_id,
            &old_id,
            Some(&reused.payload),
        )
        .await
        .unwrap();
        assert_eq!(
            state.content, None,
            "a legacy row must not answer for another sender's message with the same id"
        );
        assert_eq!(
            history(&roster, &mallory_id, &old_id, &reused.payload)
                .await
                .unwrap()
                .as_deref(),
            Some("mallory"),
            "Mallory's message shows her own text"
        );

        // Alice's old message cannot be decrypted again, so its legacy row
        // answers for it and is bound to her.
        assert_eq!(
            history(&roster, &alice_id, &old_id, &old.payload)
                .await
                .unwrap()
                .as_deref(),
            Some("alice old"),
            "an honestly cached old message keeps displaying"
        );
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, &conversation_id, &alice_id, &old_id)
                .await
                .unwrap()
                .as_deref(),
            Some("alice old")
        );
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, &conversation_id, &mallory_id, &old_id)
                .await
                .unwrap()
                .as_deref(),
            Some("mallory")
        );

        // A legacy row of a message from an epoch this device has left keeps
        // displaying too: the cache is its only copy.
        let past_id = Uuid::new_v4().to_string();
        let past = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &past_id,
            "alice past",
        )
        .await
        .unwrap();
        assert_eq!(
            history(&roster, &alice_id, &past_id, &past.payload)
                .await
                .unwrap()
                .as_deref(),
            Some("alice past")
        );
        make_legacy(&past_id).await;
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let removal = prepare_group_removal_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &roster,
            std::slice::from_ref(&mallory_id),
            None,
        )
        .await
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &removal.commit_payload,
        )
        .await
        .unwrap();
        assert_eq!(
            history(&remaining, &alice_id, &past_id, &past.payload)
                .await
                .unwrap()
                .as_deref(),
            Some("alice past")
        );
        let state = get_cached_text_state(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            &past_id,
            Some(&past.payload),
        )
        .await
        .unwrap();
        assert_eq!(state.content.as_deref(), Some("alice past"));
        assert!(!state.undecryptable);
    }

    async fn seed_legacy_text_cache(db: &SqlitePool, client_message_id: &str) {
        sqlx::query("CREATE TABLE ghostchat_crypto_text_cache (client_message_id TEXT PRIMARY KEY NOT NULL, conversation_id TEXT NOT NULL, encrypted_content BLOB NOT NULL, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')))")
            .execute(db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO ghostchat_crypto_text_cache(client_message_id,conversation_id,encrypted_content) VALUES(?,?,x'00')")
            .bind(client_message_id)
            .bind(Uuid::new_v4().to_string())
            .execute(db)
            .await
            .unwrap();
    }

    async fn text_cache_layout(db: &SqlitePool) -> (bool, Vec<String>) {
        let columns = sqlx::query_scalar::<_, String>(
            "SELECT name FROM pragma_table_info('ghostchat_crypto_text_cache')",
        )
        .fetch_all(db)
        .await
        .unwrap();
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT client_message_id FROM ghostchat_crypto_text_cache",
        )
        .fetch_all(db)
        .await
        .unwrap();
        (columns.iter().any(|name| name == "sender_id"), ids)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_existing_text_cache_is_copied_once_before_it_is_rebuilt() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-copy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let fresh = open_database(&root.join("fresh.sqlite")).await;
        prepare_device(&fresh, &Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(
            binding_backups(&root).is_empty(),
            "a fresh profile gets no copy"
        );

        let db = open_database(&root.join("profile.sqlite")).await;
        seed_legacy_text_cache(&db, "m-old").await;
        prepare_crypto_storage(&db).await.unwrap();
        prepare_crypto_storage(&db).await.unwrap();
        assert_eq!(
            binding_backups(&root),
            vec!["profile.sqlite.pre-account-binding.bak"]
        );
        assert_eq!(text_cache_layout(&db).await, (true, vec!["m-old".into()]));
        let backup = open_database(&root.join("profile.sqlite.pre-account-binding.bak")).await;
        assert_eq!(
            text_cache_layout(&backup).await,
            (false, vec!["m-old".into()]),
            "the copy holds the cache as the earlier build stored it"
        );
    }

    /// A local database as an earlier build left it: the built-in server
    /// migrated only up to `last_version` and the text cache in its old layout.
    async fn earlier_build_profile(path: &Path, last_version: i64) {
        let all = sqlx::migrate!("../server/migrations");
        let db = open_database(path).await;
        sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                all.iter()
                    .filter(|migration| migration.version <= last_version)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        }
        .run(&db)
        .await
        .unwrap();
        seed_legacy_text_cache(&db, "m-old").await;
        db.close().await;
    }

    async fn migration_level(db: &SqlitePool) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_startup_backups_both_hold_the_profile_before_the_upgrade() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-startup-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let fresh = open_local_database(&root.join("fresh.sqlite").to_string_lossy())
            .await
            .unwrap();
        prepare_device(&fresh, &Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(
            binding_backups(&root).is_empty(),
            "a fresh profile gets no copy"
        );

        let path = root.join("ghostchat.sqlite");
        earlier_build_profile(&path, 24).await;
        let db = open_local_database(&path.to_string_lossy()).await.unwrap();
        prepare_crypto_storage(&db).await.unwrap();
        assert_eq!(
            binding_backups(&root),
            vec![
                "ghostchat.sqlite.pre-0025.bak",
                "ghostchat.sqlite.pre-account-binding.bak"
            ]
        );
        for name in [
            "ghostchat.sqlite.pre-0025.bak",
            "ghostchat.sqlite.pre-account-binding.bak",
        ] {
            let backup = open_database(&root.join(name)).await;
            assert_eq!(migration_level(&backup).await, 24, "{name} is pre-upgrade");
            assert_eq!(
                text_cache_layout(&backup).await,
                (false, vec!["m-old".into()]),
                "{name} holds the old cache"
            );
        }
    }

    /// The startup copy runs before any window can paint, so it reports what
    /// it copied and how long it took for the startup log.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_startup_copy_reports_its_size_and_duration() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-copytime-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let path = root.join("ghostchat.sqlite");
        earlier_build_profile(&path, 24).await;
        let report = copy_profile_before_opening(&path)
            .await
            .unwrap()
            .expect("an earlier build's profile is copied");
        assert_eq!(
            report.bytes,
            std::fs::metadata(account_binding_backup_path(&path))
                .unwrap()
                .len()
        );
        assert!(report.bytes > 0);
        assert!(report.elapsed < std::time::Duration::from_secs(60));
        let message = report.describe(&account_binding_backup_path(&path));
        assert!(message.contains("pre-account-binding.bak"), "{message}");
        assert!(message.contains(" s"), "{message}");
        let fresh = root.join("fresh.sqlite");
        let db = open_local_database(&fresh.to_string_lossy()).await.unwrap();
        db.close().await;
        assert!(
            copy_profile_before_opening(&fresh).await.unwrap().is_none(),
            "a current profile is not copied"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_backup_that_cannot_be_written_stops_startup_with_its_own_error() {
        let root =
            std::env::temp_dir().join(format!("ghostchat-mls-serverfail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let path = root.join("ghostchat.sqlite");
        earlier_build_profile(&path, 24).await;
        std::fs::create_dir_all(root.join("ghostchat.sqlite.pre-0025.bak.tmp/blocker")).unwrap();
        match open_local_database(&path.to_string_lossy()).await {
            Err(LocalDatabaseError::BackupFailed(_)) => {}
            Err(LocalDatabaseError::Other(error)) => {
                panic!("not reported as a backup failure: {error}")
            }
            Ok(_) => panic!("startup must stop without a backup"),
        }
        let db = open_database(&path).await;
        assert_eq!(migration_level(&db).await, 24, "nothing was migrated");
    }

    #[test]
    fn the_startup_backup_dialog_follows_the_system_language() {
        let (_, english) = startup_backup_failure_dialog(&["en-US".into(), "tr-TR".into()]);
        let (_, turkish) = startup_backup_failure_dialog(&["tr-TR".into()]);
        let (_, fallback) = startup_backup_failure_dialog(&[]);
        assert!(english.contains("Nothing was changed") && english.contains("disk space"));
        assert!(turkish.contains("Hiçbir şey değiştirilmedi") && turkish.contains("Diskte yer aç"));
        assert_eq!(fallback, english);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_startup_copy_pauses_encrypted_storage_with_its_own_error() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-startfail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let path = root.join("ghostchat.sqlite");
        earlier_build_profile(&path, 24).await;
        std::fs::create_dir_all(root.join("ghostchat.sqlite.pre-account-binding.bak.tmp/blocker"))
            .unwrap();
        let db = open_local_database(&path.to_string_lossy())
            .await
            .expect("the app still starts");
        assert_eq!(
            prepare_crypto_storage(&db).await.unwrap_err(),
            LOCAL_BACKUP_FAILED
        );
        assert!(
            include_str!("../ui/main.js").contains(LOCAL_BACKUP_FAILED),
            "the UI recognizes and translates this exact text"
        );
        assert_eq!(text_cache_layout(&db).await, (false, vec!["m-old".into()]));
        assert!(!root
            .join("ghostchat.sqlite.pre-account-binding.bak")
            .exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_text_cache_that_cannot_be_copied_is_not_rebuilt() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-nocopy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let db = open_database(&root.join("profile.sqlite")).await;
        seed_legacy_text_cache(&db, "m-old").await;
        std::fs::create_dir_all(root.join("profile.sqlite.pre-account-binding.bak.tmp/blocker"))
            .unwrap();
        assert!(prepare_crypto_storage(&db).await.is_err());
        assert_eq!(text_cache_layout(&db).await, (false, vec!["m-old".into()]));
        assert!(binding_backups(&root).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn group_self_leave_proposal_keeps_group_loadable_and_owner_can_commit() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-leave-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();

        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .expect("Alice should create the encrypted group");
        for (db, id) in [(&bob_db, &bob_id), (&carol_db, &carol_id)] {
            let joined = process_crypto_event(
                db,
                id,
                &conversation_id,
                &alice_id,
                None,
                Some(1),
                true,
                &roster,
                &start.welcome_payload,
            )
            .await
            .expect("Each member should join from the group Welcome");
            assert_eq!(joined.kind, "welcome");
        }

        let leave = prepare_group_leave_proposal(&bob_db, &bob_id, &conversation_id, &roster)
            .await
            .expect("Bob should create a self-leave proposal");
        assert!(
            has_direct_group(&bob_db, &bob_id, &conversation_id)
                .await
                .expect("Bob's group must stay loadable after queuing his own proposal"),
            "Bob keeps the group until the owner commits"
        );
        let retried = prepare_group_leave_proposal(&bob_db, &bob_id, &conversation_id, &roster)
            .await
            .expect("A retried leave request should return the saved proposal");
        assert_eq!(retried.operation_id, leave.operation_id);

        let request = process_crypto_event(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            Some(&leave.operation_id),
            Some(2),
            true,
            &roster,
            &leave.commit_payload,
        )
        .await
        .expect("The owner should accept Bob's authenticated leave proposal");
        assert_eq!(request.kind, "leave_request");
        // The relay broadcasts the proposal, so every remaining member queues it.
        let carol_request = process_crypto_event(
            &carol_db,
            &carol_id,
            &conversation_id,
            &bob_id,
            Some(&leave.operation_id),
            Some(2),
            true,
            &roster,
            &leave.commit_payload,
        )
        .await
        .expect("Other members should queue Bob's leave proposal");
        assert_eq!(carol_request.kind, "leave_request");
        assert!(has_direct_group(&carol_db, &carol_id, &conversation_id)
            .await
            .expect("Carol's group must stay loadable with a queued proposal"));
        let commit = prepare_group_leave_commit(
            &alice_db,
            &alice_id,
            &conversation_id,
            &leave.operation_id,
            &bob_id,
            &roster,
            None,
        )
        .await
        .expect("The owner should load the queued proposal and commit it");
        assert_eq!(commit.action, "leave");
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &commit.operation_id,
        )
        .await
        .expect("The owner should merge the confirmed leave commit");

        let remaining = vec![alice_id.clone(), carol_id.clone()];
        let applied = process_crypto_event(
            &carol_db,
            &carol_id,
            &conversation_id,
            &alice_id,
            None,
            Some(3),
            true,
            &remaining,
            &commit.commit_payload,
        )
        .await
        .expect("Carol should apply the owner's leave commit");
        assert_eq!(applied.kind, "commit");
        assert_eq!(
            group_fingerprint(&carol_db, &carol_id, &conversation_id)
                .await
                .unwrap(),
            group_fingerprint(&alice_db, &alice_id, &conversation_id)
                .await
                .unwrap(),
            "Remaining members must agree on the new group roster"
        );
        // A replay of the applied commit is a duplicate, not a state change.
        let replayed = process_crypto_event(
            &carol_db,
            &carol_id,
            &conversation_id,
            &alice_id,
            None,
            Some(3),
            true,
            &remaining,
            &commit.commit_payload,
        )
        .await
        .unwrap();
        assert_eq!(replayed.kind, "duplicate");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn history_message_without_event_sequence_decrypts_without_moving_cursor() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-history-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let members = vec![alice_id.clone(), bob_id.clone()];
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let welcome = create_direct_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            &bob_device.device_id,
            &bob_device.payload,
        )
        .await
        .unwrap();
        // A Welcome must come from the ordered crypto-event stream.
        assert!(process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            None,
            false,
            &members,
            &welcome.welcome_payload,
        )
        .await
        .is_err());
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(7),
            false,
            &members,
            &welcome.welcome_payload,
        )
        .await
        .unwrap();

        // Encrypted text lives in ordinary message history, whose sequence is
        // unrelated to the crypto-event cursor; the UI sends no eventSequence.
        let message_id = Uuid::new_v4().to_string();
        let sent = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &message_id,
            "history text",
        )
        .await
        .unwrap();
        let received = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&message_id),
            None,
            false,
            &members,
            &sent.payload,
        )
        .await
        .expect("A history message should decrypt without a crypto-event sequence");
        assert_eq!(received.kind, "application");
        assert_eq!(received.content.as_deref(), Some("history text"));
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, &conversation_id, &alice_id, &message_id)
                .await
                .unwrap()
                .as_deref(),
            Some("history text")
        );
        assert_eq!(
            get_crypto_event_cursor(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            Some(7),
            "History decryption must not move the crypto-event cursor"
        );
        // MLS rejects a replayed ciphertext and the state stays usable.
        assert!(process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&message_id),
            None,
            false,
            &members,
            &sent.payload,
        )
        .await
        .is_err());
        let next_id = Uuid::new_v4().to_string();
        let next = encrypt_text(&alice_db, &alice_id, &conversation_id, &next_id, "next")
            .await
            .unwrap();
        assert_eq!(
            process_crypto_event(
                &bob_db,
                &bob_id,
                &conversation_id,
                &alice_id,
                Some(&next_id),
                None,
                false,
                &members,
                &next.payload,
            )
            .await
            .unwrap()
            .content
            .as_deref(),
            Some("next")
        );
    }

    /// Legacy rows come from an older build, i.e. before this process ran
    /// its migration pass. Tests that inject them into a database this
    /// process already migrated forget that pass first.
    fn forget_hash_reference_migration(db: &SqlitePool) {
        if let (Some(path), Some(set)) = (
            migration_marker(db),
            MIGRATED_DATABASES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut(),
        ) {
            set.remove(&path);
        }
    }

    fn legacy_ref_for(current_ref: &[u8]) -> Vec<u8> {
        let hash_ref: openmls::prelude::KeyPackageRef =
            EncryptedJsonCodec::from_slice(current_ref).unwrap();
        lookup_key_v1(
            STORAGE_KEY.get().unwrap(),
            &serde_json::to_vec(&hash_ref).unwrap(),
        )
        .unwrap()
    }

    /// Bob in a two-member group with his own leave proposal queued.
    async fn group_with_queued_proposal(root: &Path) -> (SqlitePool, String, String) {
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[GroupPeerKeyPackage {
                user_id: bob_id.clone(),
                device_id: bob_device.device_id.clone(),
                payload: bob_device.payload.clone(),
            }],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();
        prepare_group_leave_proposal(&bob_db, &bob_id, &conversation_id, &roster)
            .await
            .unwrap();
        (bob_db, bob_id, conversation_id)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_digest_proposal_refs_are_migrated_and_group_reloads() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-propmig-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let (bob_db, bob_id, conversation_id) = group_with_queued_proposal(&root).await;
        let current_ref =
            sqlx::query_scalar::<_, Vec<u8>>("SELECT proposal_ref FROM openmls_proposal")
                .fetch_one(&bob_db)
                .await
                .unwrap();
        assert!(current_ref.starts_with(DECODABLE_LOOKUP_KEY_PREFIX));
        sqlx::query("UPDATE openmls_proposal SET proposal_ref=?")
            .bind(legacy_ref_for(&current_ref))
            .execute(&bob_db)
            .await
            .unwrap();
        forget_hash_reference_migration(&bob_db);
        assert!(has_direct_group(&bob_db, &bob_id, &conversation_id)
            .await
            .expect("A proposal saved by an older build should be migrated so the group loads"),);
        let migrated =
            sqlx::query_scalar::<_, Vec<u8>>("SELECT proposal_ref FROM openmls_proposal")
                .fetch_one(&bob_db)
                .await
                .unwrap();
        assert_eq!(migrated, current_ref);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn migration_drops_legacy_rows_that_duplicate_existing_refs() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-dupmig-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let (bob_db, bob_id, conversation_id) = group_with_queued_proposal(&root).await;
        // Bob still holds an unused KeyPackage from prepare_device's second call.
        prepare_device(&bob_db, &bob_id).await.unwrap();
        let (package_ref, package) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "SELECT key_package_ref,key_package FROM openmls_key_package LIMIT 1",
        )
        .fetch_one(&bob_db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO openmls_key_package(key_package_ref,key_package) VALUES(?,?)")
            .bind(legacy_ref_for(&package_ref))
            .bind(&package)
            .execute(&bob_db)
            .await
            .unwrap();
        let (group_id, proposal_ref, proposal) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Vec<u8>)>(
            "SELECT group_id,proposal_ref,proposal FROM openmls_proposal",
        )
        .fetch_one(&bob_db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO openmls_proposal(group_id,proposal_ref,proposal) VALUES(?,?,?)")
            .bind(&group_id)
            .bind(legacy_ref_for(&proposal_ref))
            .bind(&proposal)
            .execute(&bob_db)
            .await
            .unwrap();
        forget_hash_reference_migration(&bob_db);

        assert!(has_direct_group(&bob_db, &bob_id, &conversation_id)
            .await
            .expect("A legacy duplicate must not make every crypto call fail"),);
        for (table, column) in [
            ("openmls_key_package", "key_package_ref"),
            ("openmls_proposal", "proposal_ref"),
        ] {
            let legacy = sqlx::query_scalar::<_, i64>(&format!(
                "SELECT count(*) FROM {table} WHERE substr({column},1,4)=?"
            ))
            .bind(LOOKUP_KEY_V1_PREFIX.as_slice())
            .fetch_one(&bob_db)
            .await
            .unwrap();
            assert_eq!(legacy, 0, "{table} should keep no legacy duplicate");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_digest_key_package_refs_are_migrated_and_still_join() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-migrate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();

        // Rewrite Bob's KeyPackage row to the one-way digest older builds wrote.
        let current_ref =
            sqlx::query_scalar::<_, Vec<u8>>("SELECT key_package_ref FROM openmls_key_package")
                .fetch_one(&bob_db)
                .await
                .unwrap();
        assert!(current_ref.starts_with(DECODABLE_LOOKUP_KEY_PREFIX));
        let hash_ref: openmls::prelude::KeyPackageRef =
            EncryptedJsonCodec::from_slice(&current_ref).unwrap();
        let legacy_ref = lookup_key_v1(
            STORAGE_KEY.get().unwrap(),
            &serde_json::to_vec(&hash_ref).unwrap(),
        )
        .unwrap();
        sqlx::query("UPDATE openmls_key_package SET key_package_ref=?")
            .bind(&legacy_ref)
            .execute(&bob_db)
            .await
            .unwrap();
        forget_hash_reference_migration(&bob_db);

        let welcome = create_direct_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            &bob_device.device_id,
            &bob_device.payload,
        )
        .await
        .unwrap();
        let joined = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            false,
            &[alice_id.clone(), bob_id.clone()],
            &welcome.welcome_payload,
        )
        .await
        .expect("A KeyPackage saved by an older build should still accept its Welcome");
        assert_eq!(joined.kind, "welcome");
    }

    async fn legacy_key_package_rows(db: &SqlitePool) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM openmls_key_package WHERE substr(key_package_ref,1,4)=?",
        )
        .bind(LOOKUP_KEY_V1_PREFIX.as_slice())
        .fetch_one(db)
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hash_reference_migration_runs_once_per_database_after_a_clean_pass() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-once-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let db = open_database(&root.join("alice.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        prepare_device(&db, &alice_id).await.unwrap();
        let current_ref =
            sqlx::query_scalar::<_, Vec<u8>>("SELECT key_package_ref FROM openmls_key_package")
                .fetch_one(&db)
                .await
                .unwrap();
        sqlx::query("UPDATE openmls_key_package SET key_package_ref=?")
            .bind(legacy_ref_for(&current_ref))
            .execute(&db)
            .await
            .unwrap();

        // The clean pass in prepare_device is remembered: no rescan.
        prepare_crypto_storage(&db).await.unwrap();
        assert_eq!(
            legacy_key_package_rows(&db).await,
            1,
            "a clean pass must not rescan"
        );

        // A new database path (the same rows copied) is still migrated.
        let copy = root.join("copy.sqlite");
        sqlx::query("VACUUM INTO ?")
            .bind(copy.to_string_lossy().to_string())
            .execute(&db)
            .await
            .unwrap();
        let copy_db = open_database(&copy).await;
        prepare_crypto_storage(&copy_db).await.unwrap();
        assert_eq!(legacy_key_package_rows(&copy_db).await, 0);

        // A failed pass is not remembered: the next call runs it again.
        let fresh = root.join("fresh.sqlite");
        sqlx::query("VACUUM INTO ?")
            .bind(fresh.to_string_lossy().to_string())
            .execute(&db)
            .await
            .unwrap();
        let fresh_db = open_database(&fresh).await;
        sqlx::query("ALTER TABLE openmls_proposal RENAME TO openmls_proposal_hidden")
            .execute(&fresh_db)
            .await
            .unwrap();
        assert!(migrate_hash_reference_keys(&fresh_db).await.is_err());
        sqlx::query("ALTER TABLE openmls_proposal_hidden RENAME TO openmls_proposal")
            .execute(&fresh_db)
            .await
            .unwrap();
        assert_eq!(legacy_key_package_rows(&fresh_db).await, 1);
        prepare_crypto_storage(&fresh_db).await.unwrap();
        assert_eq!(legacy_key_package_rows(&fresh_db).await, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deleted_account_leaf_is_tolerated_until_the_owner_removes_it() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-deleted-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        for (db, id) in [(&bob_db, &bob_id), (&carol_db, &carol_id)] {
            process_crypto_event(
                db,
                id,
                &conversation_id,
                &alice_id,
                None,
                Some(1),
                true,
                &roster,
                &start.welcome_payload,
            )
            .await
            .expect("Each member should join from the group Welcome");
        }

        // Carol deletes her account: the server roster loses her, MLS does not.
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let deleted = vec![carol_id.clone()];
        let before = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &Uuid::new_v4().to_string(),
            "before removal",
        )
        .await
        .unwrap();
        let unknown = vec![Uuid::new_v4().to_string()];
        assert!(
            process_crypto_event_with_deleted(
                &bob_db,
                &bob_id,
                &conversation_id,
                &alice_id,
                None,
                None,
                true,
                &remaining,
                &unknown,
                &before.payload,
            )
            .await
            .is_err(),
            "a leaf that is not a recorded deleted account is still rejected"
        );
        let decrypted = process_crypto_event_with_deleted(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            None,
            true,
            &remaining,
            &deleted,
            &before.payload,
        )
        .await
        .expect("Bob should still decrypt while Carol's deleted leaf remains");
        assert_eq!(decrypted.content.as_deref(), Some("before removal"));

        assert!(
            prepare_deleted_member_removal(
                &alice_db,
                &alice_id,
                &conversation_id,
                &remaining,
                &[],
                None
            )
            .await
            .is_err(),
            "without the deleted record the roster mismatch is still refused"
        );
        let removal = prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            None,
        )
        .await
        .unwrap()
        .expect("The owner should stage the removal of Carol's deleted leaf");
        assert_eq!(removal.action, "remove_deleted");
        assert_eq!(removal.member_ids, deleted);
        assert_eq!(removal.expected_member_ids, remaining);
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .expect("The owner should merge the confirmed removal");
        let applied = process_crypto_event_with_deleted(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &deleted,
            &removal.commit_payload,
        )
        .await
        .expect("Bob should apply the removal commit");
        assert_eq!(applied.kind, "commit");
        assert_eq!(
            group_fingerprint(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            group_fingerprint(&alice_db, &alice_id, &conversation_id)
                .await
                .unwrap(),
        );
        assert!(prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            None
        )
        .await
        .unwrap()
        .is_none());

        // The rosters now match exactly: no tolerance is needed any more.
        let after = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &Uuid::new_v4().to_string(),
            "after removal",
        )
        .await
        .unwrap();
        let decrypted = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            None,
            true,
            &remaining,
            &after.payload,
        )
        .await
        .expect("Bob should decrypt with the exact server roster");
        assert_eq!(decrypted.content.as_deref(), Some("after removal"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_newer_setup_generation_drops_the_old_group_so_the_new_welcome_is_joined() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-reset-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        let alice_device = prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        prepare_device(&carol_db, &carol_id).await.unwrap();
        let package = |user_id: &str, device: &DevicePreparation| GroupPeerKeyPackage {
            user_id: user_id.to_owned(),
            device_id: device.device_id.clone(),
            payload: device.payload.clone(),
        };
        // Carol starts the setup; only Bob receives her Welcome before her
        // account is deleted.
        let old_setup = create_group(
            &carol_db,
            &carol_id,
            &conversation_id,
            &[
                package(&alice_id, &alice_device),
                package(&bob_id, &bob_device),
            ],
        )
        .await
        .unwrap();
        let joined = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &carol_id,
            None,
            Some(1),
            true,
            &roster,
            &old_setup.welcome_payload,
        )
        .await
        .unwrap();
        assert_eq!(joined.kind, "welcome");

        // Alice, who never joined, takes the setup over (generation 1) and
        // creates the group again for the remaining roster.
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        assert!(
            apply_setup_generation(&alice_db, &alice_id, &conversation_id, 1)
                .await
                .unwrap()
        );
        let bob_fresh = prepare_device(&bob_db, &bob_id).await.unwrap();
        let new_setup = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[package(&bob_id, &bob_fresh)],
        )
        .await
        .unwrap();
        assert!(
            get_pending_bootstrap(&alice_db, &alice_id, &conversation_id)
                .await
                .unwrap()
                .is_some(),
            "the new setup's own pending bootstrap is kept"
        );
        assert!(
            !apply_setup_generation(&alice_db, &alice_id, &conversation_id, 1)
                .await
                .unwrap(),
            "the same generation again changes nothing"
        );
        assert!(has_direct_group(&alice_db, &alice_id, &conversation_id)
            .await
            .unwrap());

        // While Bob still holds the old group, the new Welcome is a duplicate
        // for him and he would stay in a group nobody else is in.
        let stuck = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &new_setup.welcome_payload,
        )
        .await
        .unwrap();
        assert_eq!(stuck.kind, "welcome_duplicate");

        // The newer generation drops the old group; an older (stale) value
        // afterwards does nothing.
        assert!(
            apply_setup_generation(&bob_db, &bob_id, &conversation_id, 1)
                .await
                .unwrap()
        );
        assert!(!has_direct_group(&bob_db, &bob_id, &conversation_id)
            .await
            .unwrap());
        assert!(
            !apply_setup_generation(&bob_db, &bob_id, &conversation_id, 0)
                .await
                .unwrap()
        );
        let rejoined = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(3),
            true,
            &remaining,
            &new_setup.welcome_payload,
        )
        .await
        .expect("Bob should join the new setup's group");
        assert_eq!(rejoined.kind, "welcome");
        assert!(
            !apply_setup_generation(&bob_db, &bob_id, &conversation_id, 1)
                .await
                .unwrap(),
            "the joined group of the current generation is kept"
        );
        let hello = encrypt_text(
            &alice_db,
            &alice_id,
            &conversation_id,
            &Uuid::new_v4().to_string(),
            "after the reset",
        )
        .await
        .unwrap();
        let decrypted = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            None,
            true,
            &remaining,
            &hello.payload,
        )
        .await
        .unwrap();
        assert_eq!(decrypted.content.as_deref(), Some("after the reset"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_staged_commit_keeps_the_commit_sequence_it_was_built_on() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-seq-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();

        // The server's commit sequence the commit was built on is saved with
        // the staged update, so a retry after a restart sends the same value.
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let deleted = vec![carol_id.clone()];
        let staged = prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            Some(4),
        )
        .await
        .unwrap()
        .expect("Carol's leaf is still in the group");
        assert_eq!(staged.expected_commit_seq, Some(4));
        let json = serde_json::to_value(&staged).unwrap();
        assert_eq!(json["expectedCommitSeq"], 4);
        let saved = get_pending_group_membership_update(&alice_db, &alice_id, &conversation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.operation_id, staged.operation_id);
        assert_eq!(saved.expected_commit_seq, Some(4));
        // The pending update was built on the earlier state: asking again
        // after the group moved on must not relabel it.
        let again = prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            Some(9),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(again.operation_id, staged.operation_id);
        assert_eq!(again.expected_commit_seq, Some(4));
        discard_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &staged.operation_id,
        )
        .await
        .unwrap();

        // A server without the sequence gives no value; none is invented.
        let plain = prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(plain.expected_commit_seq, None);
        assert!(serde_json::to_value(&plain)
            .unwrap()
            .get("expectedCommitSeq")
            .is_none());
        discard_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &plain.operation_id,
        )
        .await
        .unwrap();
        assert!(prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            Some(-1),
        )
        .await
        .is_err());

        // A leave commit is stamped the same way.
        let leave = prepare_group_leave_proposal(&bob_db, &bob_id, &conversation_id, &roster)
            .await
            .unwrap();
        process_crypto_event_with_deleted(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            Some(&leave.operation_id),
            Some(2),
            true,
            &roster,
            &[],
            &leave.commit_payload,
        )
        .await
        .unwrap();
        let commit = prepare_group_leave_commit(
            &alice_db,
            &alice_id,
            &conversation_id,
            &leave.operation_id,
            &bob_id,
            &roster,
            Some(6),
        )
        .await
        .unwrap();
        assert_eq!(commit.action, "leave");
        assert_eq!(commit.expected_commit_seq, Some(6));
        let retried = prepare_group_leave_commit(
            &alice_db,
            &alice_id,
            &conversation_id,
            &leave.operation_id,
            &bob_id,
            &roster,
            Some(7),
        )
        .await
        .unwrap();
        assert_eq!(retried.expected_commit_seq, Some(6));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_staged_removal_is_dropped_and_the_accepted_one_applies() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-refused-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let deleted = vec![carol_id.clone()];
        // Owner Alice and admin Bob both stage the removal of deleted Carol;
        // the server accepts Alice's and refuses Bob's as already done.
        let accepted = prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &accepted.operation_id,
        )
        .await
        .unwrap();
        let refused = prepare_deleted_member_removal(
            &bob_db,
            &bob_id,
            &conversation_id,
            &remaining,
            &deleted,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            discard_group_membership_update(
                &bob_db,
                &bob_id,
                &conversation_id,
                &Uuid::new_v4().to_string()
            )
            .await
            .is_err(),
            "only the staged operation itself can be dropped"
        );
        discard_group_membership_update(&bob_db, &bob_id, &conversation_id, &refused.operation_id)
            .await
            .unwrap();
        assert!(
            get_pending_group_membership_update(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap()
                .is_none(),
            "a dropped operation is not retried"
        );
        let applied = process_crypto_event_with_deleted(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &deleted,
            &accepted.commit_payload,
        )
        .await
        .expect("the accepted removal applies after the refused one was dropped");
        assert_eq!(applied.kind, "commit");
        assert_eq!(
            group_fingerprint(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            group_fingerprint(&alice_db, &alice_id, &conversation_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_deleted_leaf_is_skipped_and_the_next_one_is_staged() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-skip-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let mut peers = Vec::new();
        let mut peer_ids = Vec::new();
        for name in ["bob", "carol", "dave"] {
            let db = open_database(&root.join(format!("{name}.sqlite"))).await;
            let id = Uuid::new_v4().to_string();
            let device = prepare_device(&db, &id).await.unwrap();
            peers.push(GroupPeerKeyPackage {
                user_id: id.clone(),
                device_id: device.device_id,
                payload: device.payload,
            });
            peer_ids.push(id);
        }
        create_group(&alice_db, &alice_id, &conversation_id, &peers)
            .await
            .unwrap();
        let mut remaining = vec![alice_id.clone(), peer_ids[0].clone()];
        remaining.sort();
        let mut deleted = vec![peer_ids[1].clone(), peer_ids[2].clone()];
        deleted.sort();
        let (first, second) = (deleted[0].clone(), deleted[1].clone());
        let prepare = |excluded: Vec<String>| {
            let (alice_db, alice_id, conversation_id, remaining, deleted) = (
                alice_db.clone(),
                alice_id.clone(),
                conversation_id.clone(),
                remaining.clone(),
                deleted.clone(),
            );
            async move {
                let update = prepare_deleted_member_removal_excluding(
                    &alice_db,
                    &alice_id,
                    &conversation_id,
                    &remaining,
                    &deleted,
                    &excluded,
                    None,
                )
                .await
                .unwrap();
                if let Some(update) = &update {
                    discard_group_membership_update(
                        &alice_db,
                        &alice_id,
                        &conversation_id,
                        &update.operation_id,
                    )
                    .await
                    .unwrap();
                }
                update.map(|update| update.member_ids)
            }
        };
        assert_eq!(
            prepare(Vec::new()).await,
            Some(vec![first.clone()]),
            "without exclusions the first deleted leaf is staged"
        );
        assert_eq!(
            prepare_deleted_member_removal(
                &alice_db,
                &alice_id,
                &conversation_id,
                &remaining,
                &deleted,
                None
            )
            .await
            .unwrap()
            .map(|update| update.member_ids),
            Some(vec![first.clone()]),
            "the plain call is unchanged"
        );
        discard_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &get_pending_group_membership_update(&alice_db, &alice_id, &conversation_id)
                .await
                .unwrap()
                .unwrap()
                .operation_id,
        )
        .await
        .unwrap();
        assert_eq!(
            prepare(vec![first.clone()]).await,
            Some(vec![second.clone()]),
            "a refused deleted leaf is skipped and the next one is staged"
        );
        assert_eq!(
            prepare(vec![second.clone(), first.clone()]).await,
            None,
            "nothing is staged when every deleted leaf was refused"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn removed_deleted_account_cannot_be_re_added_by_a_commit() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-readd-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let carol_again_db = open_database(&root.join("carol-again.sqlite")).await;
        let dave_db = open_database(&root.join("dave.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let dave_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();

        // Carol is deleted and her leaf is removed by the owner.
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let deleted = vec![carol_id.clone()];
        let removal = prepare_deleted_member_removal(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &deleted,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .unwrap();
        process_crypto_event_with_deleted(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &deleted,
            &removal.commit_payload,
        )
        .await
        .unwrap();

        // A legitimate add of Dave is still accepted while Carol stays listed.
        let dave_device = prepare_device(&dave_db, &dave_id).await.unwrap();
        let add_dave = prepare_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &remaining,
            &[GroupPeerKeyPackage {
                user_id: dave_id.clone(),
                device_id: dave_device.device_id.clone(),
                payload: dave_device.payload.clone(),
            }],
            None,
        )
        .await
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &add_dave.operation_id,
        )
        .await
        .unwrap();
        let mut with_dave = vec![alice_id.clone(), bob_id.clone(), dave_id.clone()];
        with_dave.sort();
        let applied = process_crypto_event_with_deleted(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(3),
            true,
            &with_dave,
            &deleted,
            &add_dave.commit_payload,
        )
        .await
        .expect("a legitimate add commit is accepted");
        assert_eq!(applied.kind, "commit");

        // A commit that adds a new leaf for deleted Carol must be refused,
        // although the server still lists her as a deleted account.
        let carol_again = prepare_device(&carol_again_db, &carol_id).await.unwrap();
        let readd = prepare_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &with_dave,
            &[GroupPeerKeyPackage {
                user_id: carol_id.clone(),
                device_id: carol_again.device_id.clone(),
                payload: carol_again.payload.clone(),
            }],
            None,
        )
        .await
        .unwrap();
        let before = group_fingerprint(&bob_db, &bob_id, &conversation_id)
            .await
            .unwrap();
        assert!(
            process_crypto_event_with_deleted(
                &bob_db,
                &bob_id,
                &conversation_id,
                &alice_id,
                None,
                Some(4),
                true,
                &with_dave,
                &deleted,
                &readd.commit_payload,
            )
            .await
            .is_err(),
            "a deleted account's leaf may stay or be removed, never be added"
        );
        assert_eq!(
            group_fingerprint(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            before,
            "the refused commit must not be merged"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn past_epoch_history_rows_are_marked_undecryptable_once() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-pastepoch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();
        let old_id = Uuid::new_v4().to_string();
        let old = encrypt_text(&alice_db, &alice_id, &conversation_id, &old_id, "old")
            .await
            .unwrap();
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let removal = prepare_group_removal_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &roster,
            std::slice::from_ref(&carol_id),
            None,
        )
        .await
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .unwrap();
        let new_id = Uuid::new_v4().to_string();
        let new = encrypt_text(&alice_db, &alice_id, &conversation_id, &new_id, "new")
            .await
            .unwrap();
        let decode = |id: String, payload: String| {
            let (bob_db, bob_id, conversation_id, alice_id, remaining) = (
                bob_db.clone(),
                bob_id.clone(),
                conversation_id.clone(),
                alice_id.clone(),
                remaining.clone(),
            );
            async move {
                process_crypto_event(
                    &bob_db,
                    &bob_id,
                    &conversation_id,
                    &alice_id,
                    Some(&id),
                    None,
                    true,
                    &remaining,
                    &payload,
                )
                .await
            }
        };
        let state = |id: String| {
            let (bob_db, bob_id, conversation_id, alice_id) = (
                bob_db.clone(),
                bob_id.clone(),
                conversation_id.clone(),
                alice_id.clone(),
            );
            async move {
                get_cached_text_state(&bob_db, &bob_id, &conversation_id, &alice_id, &id, None)
                    .await
                    .unwrap()
            }
        };

        // A row from a later epoch fails now but is not permanent.
        assert!(decode(new_id.clone(), new.payload.clone()).await.is_err());
        assert!(!state(new_id.clone()).await.undecryptable);
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &removal.commit_payload,
        )
        .await
        .unwrap();
        // A row from an epoch this device has left can never be decrypted.
        assert!(decode(old_id.clone(), old.payload.clone()).await.is_err());
        let marked = state(old_id.clone()).await;
        assert!(marked.undecryptable, "a past-epoch row is marked once");
        assert_eq!(marked.content, None);
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, &conversation_id, &alice_id, &old_id)
                .await
                .unwrap(),
            None,
            "a marker is not plaintext"
        );
        let decoded = decode(new_id.clone(), new.payload.clone()).await.unwrap();
        assert_eq!(decoded.content.as_deref(), Some("new"));
        let cached = state(new_id).await;
        assert_eq!(cached.content.as_deref(), Some("new"));
        assert!(!cached.undecryptable);

        // The marker is bound to the payload it was written for: the server
        // first gives a row a past-epoch payload, then the real one.
        let state_for = |id: String, payload: String| {
            let (bob_db, bob_id, conversation_id, alice_id) = (
                bob_db.clone(),
                bob_id.clone(),
                conversation_id.clone(),
                alice_id.clone(),
            );
            async move {
                get_cached_text_state(
                    &bob_db,
                    &bob_id,
                    &conversation_id,
                    &alice_id,
                    &id,
                    Some(&payload),
                )
                .await
                .unwrap()
            }
        };
        let later_id = Uuid::new_v4().to_string();
        let later = encrypt_text(&alice_db, &alice_id, &conversation_id, &later_id, "later")
            .await
            .unwrap();
        assert!(decode(later_id.clone(), old.payload.clone()).await.is_err());
        assert!(
            state_for(later_id.clone(), old.payload.clone())
                .await
                .undecryptable,
            "the payload that was marked stays marked"
        );
        assert!(
            !state_for(later_id.clone(), later.payload.clone())
                .await
                .undecryptable,
            "a different payload for the same row is not marked"
        );
        let decoded = decode(later_id.clone(), later.payload.clone())
            .await
            .unwrap();
        assert_eq!(decoded.content.as_deref(), Some("later"));
        assert_eq!(
            state_for(later_id, later.payload.clone())
                .await
                .content
                .as_deref(),
            Some("later")
        );

        // A marker written before markers held a payload hash keeps its
        // effect for a payload that is still from a past epoch, and does not
        // block a payload that is not.
        let legacy_marker = |id: String| {
            let (bob_db, bob_id, conversation_id, alice_id) = (
                bob_db.clone(),
                bob_id.clone(),
                conversation_id.clone(),
                alice_id.clone(),
            );
            async move {
                sqlx::query("DROP TABLE ghostchat_crypto_undecryptable_rows")
                    .execute(&bob_db)
                    .await
                    .unwrap();
                sqlx::query("CREATE TABLE ghostchat_crypto_undecryptable_rows (user_id TEXT NOT NULL, conversation_id TEXT NOT NULL, sender_id TEXT NOT NULL, client_message_id TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), PRIMARY KEY(user_id,conversation_id,sender_id,client_message_id))")
                    .execute(&bob_db)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO ghostchat_crypto_undecryptable_rows(user_id,conversation_id,sender_id,client_message_id) VALUES(?,?,?,?)")
                    .bind(&bob_id)
                    .bind(&conversation_id)
                    .bind(&alice_id)
                    .bind(&id)
                    .execute(&bob_db)
                    .await
                    .unwrap();
            }
        };
        legacy_marker(old_id.clone()).await;
        assert!(
            state_for(old_id.clone(), old.payload.clone())
                .await
                .undecryptable
        );
        assert!(
            state_for(old_id.clone(), old.payload.clone())
                .await
                .undecryptable,
            "the migration is idempotent"
        );
        let honest_id = Uuid::new_v4().to_string();
        let honest = encrypt_text(&alice_db, &alice_id, &conversation_id, &honest_id, "honest")
            .await
            .unwrap();
        legacy_marker(honest_id.clone()).await;
        assert!(
            !state_for(honest_id.clone(), honest.payload.clone())
                .await
                .undecryptable,
            "an old marker does not block a payload of the current epoch"
        );
        let decoded = decode(honest_id, honest.payload).await.unwrap();
        assert_eq!(decoded.content.as_deref(), Some("honest"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inspecting_a_relay_commit_authenticates_it_without_applying_it() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-inspect-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();
        let missed_id = Uuid::new_v4().to_string();
        let missed = encrypt_text(&alice_db, &alice_id, &conversation_id, &missed_id, "missed")
            .await
            .unwrap();
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let removal = prepare_group_removal_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &roster,
            std::slice::from_ref(&carol_id),
            None,
        )
        .await
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .unwrap();
        let before = group_fingerprint(&bob_db, &bob_id, &conversation_id)
            .await
            .unwrap();

        // A relay event claiming the wrong sender is not an authenticated commit.
        assert!(inspect_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &carol_id,
            2,
            true,
            &remaining,
            &[],
            &removal.commit_payload,
        )
        .await
        .is_err());
        let inspected = inspect_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            2,
            true,
            &remaining,
            &[],
            &removal.commit_payload,
        )
        .await
        .expect("a valid commit from a current member is recognised");
        assert_eq!(inspected.kind, "commit");
        assert_eq!(
            group_fingerprint(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            before,
            "inspection must not apply the commit"
        );
        assert_eq!(
            get_crypto_event_cursor(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            Some(1),
            "inspection must not move the relay cursor"
        );
        // The current epoch is intact: the pending row still decrypts, and
        // the commit then applies for real.
        let decoded = decode_history_before_pending_commits(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            &missed_id,
            true,
            &missed.payload,
        )
        .await
        .unwrap();
        assert_eq!(decoded.content.as_deref(), Some("missed"));
        let applied = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &removal.commit_payload,
        )
        .await
        .unwrap();
        assert_eq!(applied.kind, "commit");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_refused_as_stale_is_encrypted_again_for_the_new_epoch() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-reencrypt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(1),
            true,
            &roster,
            &start.welcome_payload,
        )
        .await
        .unwrap();

        // Bob encrypts while Alice's removal of Carol is accepted first.
        let message_id = Uuid::new_v4().to_string();
        let stale = encrypt_text(&bob_db, &bob_id, &conversation_id, &message_id, "hello")
            .await
            .unwrap();
        let removal = prepare_group_removal_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &roster,
            std::slice::from_ref(&carol_id),
            None,
        )
        .await
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .unwrap();
        assert!(
            process_crypto_event(
                &alice_db,
                &alice_id,
                &conversation_id,
                &bob_id,
                Some(&message_id),
                None,
                true,
                &remaining,
                &stale.payload,
            )
            .await
            .is_err(),
            "the old-epoch ciphertext is unreadable after the commit"
        );
        // The server refuses it; Bob applies the commit and encrypts the
        // same saved text again under the same message id.
        process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &removal.commit_payload,
        )
        .await
        .unwrap();
        let fresh = reencrypt_text(&bob_db, &bob_id, &conversation_id, &message_id)
            .await
            .unwrap();
        assert_ne!(fresh.payload, stale.payload);
        let read = process_crypto_event(
            &alice_db,
            &alice_id,
            &conversation_id,
            &bob_id,
            Some(&message_id),
            None,
            true,
            &remaining,
            &fresh.payload,
        )
        .await
        .expect("the re-encrypted message is readable in the new epoch");
        assert_eq!(read.content.as_deref(), Some("hello"));
        // A later retry sends the new ciphertext, not the stale one.
        let retried = encrypt_text(&bob_db, &bob_id, &conversation_id, &message_id, "hello")
            .await
            .unwrap();
        assert_eq!(retried.payload, fresh.payload);
        // Only a message this account has queued can be encrypted again.
        assert!(reencrypt_text(
            &bob_db,
            &bob_id,
            &conversation_id,
            &Uuid::new_v4().to_string()
        )
        .await
        .is_err());
        assert!(
            reencrypt_text(&alice_db, &alice_id, &conversation_id, &message_id)
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pending_history_message_decrypts_before_a_membership_commit() {
        let root = std::env::temp_dir().join(format!("ghostchat-mls-precommit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let start = create_group(
            &alice_db,
            &alice_id,
            &conversation_id,
            &[
                GroupPeerKeyPackage {
                    user_id: bob_id.clone(),
                    device_id: bob_device.device_id.clone(),
                    payload: bob_device.payload.clone(),
                },
                GroupPeerKeyPackage {
                    user_id: carol_id.clone(),
                    device_id: carol_device.device_id.clone(),
                    payload: carol_device.payload.clone(),
                },
            ],
        )
        .await
        .unwrap();
        for (db, id) in [(&bob_db, &bob_id), (&carol_db, &carol_id)] {
            process_crypto_event(
                db,
                id,
                &conversation_id,
                &alice_id,
                None,
                Some(1),
                true,
                &roster,
                &start.welcome_payload,
            )
            .await
            .unwrap();
        }

        // Alice sends while Bob is elsewhere, then removes Carol.
        let missed_id = Uuid::new_v4().to_string();
        let missed = encrypt_text(&alice_db, &alice_id, &conversation_id, &missed_id, "missed")
            .await
            .unwrap();
        let mut remaining = vec![alice_id.clone(), bob_id.clone()];
        remaining.sort();
        let removal = prepare_group_removal_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &roster,
            std::slice::from_ref(&carol_id),
            None,
        )
        .await
        .unwrap();
        finalize_group_membership_update(
            &alice_db,
            &alice_id,
            &conversation_id,
            &removal.operation_id,
        )
        .await
        .unwrap();

        // Bob opens the group: the server roster already lacks Carol and the
        // removal commit is pending on the relay. Decoding the pending
        // history first, against the local epoch's roster, keeps the text.
        // The ordinary history path compares with the changed server roster.
        assert!(process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&missed_id),
            None,
            true,
            &remaining,
            &missed.payload,
        )
        .await
        .is_err());
        let decoded = decode_history_before_pending_commits(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            &missed_id,
            true,
            &missed.payload,
        )
        .await
        .expect("A message from the current local epoch should decrypt before the commit");
        assert_eq!(decoded.content.as_deref(), Some("missed"));
        assert_eq!(
            get_crypto_event_cursor(&bob_db, &bob_id, &conversation_id)
                .await
                .unwrap(),
            Some(1),
            "history decoding never moves the relay cursor"
        );
        let applied = process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &removal.commit_payload,
        )
        .await
        .unwrap();
        assert_eq!(applied.kind, "commit");
        assert_eq!(
            get_cached_sent_text(&bob_db, &bob_id, &conversation_id, &alice_id, &missed_id)
                .await
                .unwrap()
                .as_deref(),
            Some("missed"),
            "the plaintext stays in the local cache after the epoch moved on"
        );
        // Without the earlier decode the past-epoch ciphertext is unreadable.
        assert!(process_crypto_event(
            &bob_db,
            &bob_id,
            &conversation_id,
            &alice_id,
            Some(&Uuid::new_v4().to_string()),
            None,
            true,
            &remaining,
            &missed.payload,
        )
        .await
        .is_err());
        // The local-epoch mode only decodes history text, never relay events.
        assert!(decode_history_before_pending_commits(
            &carol_db,
            &carol_id,
            &conversation_id,
            &alice_id,
            &Uuid::new_v4().to_string(),
            true,
            &removal.commit_payload,
        )
        .await
        .is_err());
    }
}
