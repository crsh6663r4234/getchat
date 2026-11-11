//! Planner items (polls, events, votes, RSVPs) of encrypted groups.
//!
//! The planner key of an encrypted group is exported from its current MLS
//! epoch (`export_secret`, label [`PLANNER_KEY_LABEL`], context = the
//! conversation id), so every member at the same epoch derives the same key
//! and a commit replaces it. The key never leaves this module: the UI asks
//! for the current epoch and key id, and seals or opens payloads here.
//!
//! OpenMLS keeps no past-epoch secrets, so each device keeps a bounded cache
//! of the planner keys of the epochs it was in ([`PLANNER_KEY_CACHE_LIMIT`]
//! per group), encrypted with the local storage key. A key is cached before
//! and after every merged commit, when a group is created or joined, and
//! whenever a planner command runs. An item written in an epoch this device
//! never had (or one that fell out of the cache) cannot be opened here.

use super::*;
use sqlx::SqliteConnection;

pub(super) const PLANNER_KEY_LABEL: &str = "ghostchat planner v1";
const PLANNER_KEY_CACHE_LIMIT: i64 = 64;
const PLANNER_PLAINTEXT_LIMIT: usize = 8 * 1024;
const PLANNER_SEAL_PREFIX: &[u8] = b"GP\x01";
const PLANNER_NONCE_LEN: usize = 12;
const PLANNER_TAG_LEN: usize = 16;

/// A planner key of one epoch of one group.
pub(super) struct PlannerEpochKey {
    epoch: u64,
    key_id: String,
    key: Zeroizing<[u8; 32]>,
}

/// What the UI learns about a planner key: never the key itself.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlannerKeyInfo {
    pub epoch: u64,
    pub key_id: String,
}

/// A sealed planner payload and the key it was sealed with.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlannerSealed {
    pub ciphertext: String,
    pub epoch: u64,
    pub key_id: String,
}

fn planner_key_id(conversation_id: &str, key: &[u8; 32]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ghostchat planner key id v1\0");
    digest.update(conversation_id.as_bytes());
    digest.update(b"\0");
    digest.update(key);
    digest.finalize()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The planner key of the group's current epoch. A device that is no longer
/// a member (its group is inactive after its removal) cannot export one.
pub(super) fn derive_planner_key(
    group: &MlsGroup,
    crypto: &RustCrypto,
    conversation_id: &str,
) -> Result<PlannerEpochKey, String> {
    let exported = Zeroizing::new(
        group
            .export_secret(crypto, PLANNER_KEY_LABEL, conversation_id.as_bytes(), 32)
            .map_err(|_| {
                "This device is not an active member of the encrypted group.".to_string()
            })?,
    );
    let mut key = Zeroizing::new([0u8; 32]);
    if exported.len() != key.len() {
        return Err("GhostChat could not derive the planner key.".into());
    }
    key.copy_from_slice(&exported);
    Ok(PlannerEpochKey {
        epoch: group.epoch().as_u64(),
        key_id: planner_key_id(conversation_id, &key),
        key,
    })
}

/// [`derive_planner_key`] for the cache hooks: a group this device cannot
/// export from (it was removed) has no key to remember.
pub(super) fn capture_planner_key(
    group: &MlsGroup,
    crypto: &RustCrypto,
    conversation_id: &str,
) -> Option<PlannerEpochKey> {
    derive_planner_key(group, crypto, conversation_id).ok()
}

pub(super) async fn prepare_planner_storage(db: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_planner_keys (\
            user_id TEXT NOT NULL,\
            conversation_id TEXT NOT NULL,\
            key_id TEXT NOT NULL,\
            epoch INTEGER NOT NULL CHECK(epoch >= 0),\
            encrypted_key BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),\
            PRIMARY KEY(user_id,conversation_id,key_id)\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its encrypted planner key storage.".to_string())?;
    Ok(())
}

/// Saves planner keys in the bounded per-group cache (the newest
/// [`PLANNER_KEY_CACHE_LIMIT`] keys of a group stay).
pub(super) async fn store_planner_keys(
    connection: &mut SqliteConnection,
    user_id: &str,
    conversation_id: &str,
    keys: &[PlannerEpochKey],
) -> Result<(), String> {
    if keys.is_empty() {
        return Ok(());
    }
    for key in keys {
        let epoch = i64::try_from(key.epoch)
            .map_err(|_| "The encrypted group epoch is out of range.".to_string())?;
        let raw = Zeroizing::new(key.key.to_vec());
        let encrypted = EncryptedJsonCodec::to_vec(&*raw)
            .map_err(|_| "GhostChat could not protect the planner key.".to_string())?;
        sqlx::query(
            "INSERT OR IGNORE INTO ghostchat_planner_keys(user_id,conversation_id,key_id,epoch,encrypted_key) VALUES(?,?,?,?,?)",
        )
        .bind(user_id)
        .bind(conversation_id)
        .bind(&key.key_id)
        .bind(epoch)
        .bind(encrypted)
        .execute(&mut *connection)
        .await
        .map_err(|_| "GhostChat could not save the planner key.".to_string())?;
    }
    sqlx::query(
        "DELETE FROM ghostchat_planner_keys WHERE user_id=?1 AND conversation_id=?2 AND rowid NOT IN (SELECT rowid FROM ghostchat_planner_keys WHERE user_id=?1 AND conversation_id=?2 ORDER BY rowid DESC LIMIT ?3)",
    )
    .bind(user_id)
    .bind(conversation_id)
    .bind(PLANNER_KEY_CACHE_LIMIT)
    .execute(&mut *connection)
    .await
    .map_err(|_| "GhostChat could not prune the planner key cache.".to_string())?;
    Ok(())
}

async fn cached_planner_key(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    epoch: u64,
    key_id: &str,
) -> Result<Option<Zeroizing<[u8; 32]>>, String> {
    let Ok(epoch) = i64::try_from(epoch) else {
        return Ok(None);
    };
    let Some(encrypted) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT encrypted_key FROM ghostchat_planner_keys WHERE user_id=? AND conversation_id=? AND key_id=? AND epoch=?",
    )
    .bind(user_id)
    .bind(conversation_id)
    .bind(key_id)
    .bind(epoch)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not load the planner key cache.".to_string())?
    else {
        return Ok(None);
    };
    let raw = Zeroizing::new(
        EncryptedJsonCodec::from_slice::<Vec<u8>>(&encrypted)
            .map_err(|_| "The planner key cache could not be decrypted.".to_string())?,
    );
    let mut key = Zeroizing::new([0u8; 32]);
    if raw.len() != key.len() {
        return Err("The planner key cache is damaged.".into());
    }
    key.copy_from_slice(&raw);
    if planner_key_id(conversation_id, &key) != key_id {
        return Err("The planner key cache is damaged.".into());
    }
    Ok(Some(key))
}

/// Derives the current epoch's planner key from the stored MLS group and
/// caches it. `None`: the group is missing or this device was removed.
async fn current_planner_key(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<PlannerEpochKey>, String> {
    let _storage_account = StorageAccountScope::enter(user_id);
    adopt_legacy_group(db, user_id, conversation_id).await?;
    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    let key = {
        let storage = SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection);
        let provider = DesktopMlsProvider {
            crypto: RustCrypto::default(),
            storage,
        };
        let Some(group) = MlsGroup::load(
            provider.storage(),
            &group_id_for_conversation(conversation_id),
        )
        .map_err(|_| "GhostChat could not load the encrypted MLS group.".to_string())?
        else {
            return Ok(None);
        };
        capture_planner_key(&group, provider.crypto(), conversation_id)
    };
    let Some(key) = key else {
        return Ok(None);
    };
    store_planner_keys(
        &mut connection,
        user_id,
        conversation_id,
        std::slice::from_ref(&key),
    )
    .await?;
    Ok(Some(key))
}

fn normalized_planner_kind(kind: &str) -> Result<&'static str, String> {
    match kind {
        "poll" => Ok("poll"),
        "event" => Ok("event"),
        "poll_vote" => Ok("poll_vote"),
        "event_rsvp" => Ok("event_rsvp"),
        _ => Err("The planner item kind is not supported.".into()),
    }
}

/// Polls and events are named by their id; a vote or an RSVP by
/// `<item id>/<account id>`, so a row cannot be moved to another voter.
fn normalized_planner_item_id(kind: &str, item_id: &str) -> Result<String, String> {
    match kind {
        "poll" | "event" => normalized_uuid(item_id, "planner item"),
        _ => {
            let (item, account) = item_id
                .split_once('/')
                .ok_or_else(|| "The planner response id is invalid.".to_string())?;
            Ok(format!(
                "{}/{}",
                normalized_uuid(item, "planner item")?,
                normalized_uuid(account, "account")?
            ))
        }
    }
}

fn planner_aad(conversation_id: &str, kind: &str, item_id: &str, epoch: u64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(96 + item_id.len());
    aad.extend_from_slice(PLANNER_KEY_LABEL.as_bytes());
    aad.push(0);
    aad.extend_from_slice(conversation_id.as_bytes());
    aad.push(0);
    aad.extend_from_slice(kind.as_bytes());
    aad.push(0);
    aad.extend_from_slice(item_id.as_bytes());
    aad.push(0);
    aad.extend_from_slice(&epoch.to_be_bytes());
    aad
}

/// AES-256-GCM with a random 96-bit nonce; the AAD binds the conversation,
/// the item kind, the item id and the epoch.
pub(super) fn seal_planner_payload(
    key: &[u8; 32],
    conversation_id: &str,
    kind: &str,
    item_id: &str,
    epoch: u64,
    plaintext: &str,
) -> Result<String, String> {
    if plaintext.is_empty() || plaintext.len() > PLANNER_PLAINTEXT_LIMIT {
        return Err("A planner item must contain 1 byte to 8 KB of data.".into());
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "GhostChat could not use the planner key.".to_string())?;
    let mut nonce = [0u8; PLANNER_NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let aad = planner_aad(conversation_id, kind, item_id, epoch);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| "GhostChat could not encrypt the planner item.".to_string())?;
    let mut sealed = Vec::with_capacity(PLANNER_SEAL_PREFIX.len() + nonce.len() + ciphertext.len());
    sealed.extend_from_slice(PLANNER_SEAL_PREFIX);
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

pub(super) fn open_planner_payload(
    key: &[u8; 32],
    conversation_id: &str,
    kind: &str,
    item_id: &str,
    epoch: u64,
    ciphertext: &str,
) -> Result<String, String> {
    let sealed = URL_SAFE_NO_PAD
        .decode(ciphertext.as_bytes())
        .map_err(|_| "The encrypted planner item has invalid encoding.".to_string())?;
    let header = PLANNER_SEAL_PREFIX.len() + PLANNER_NONCE_LEN;
    if sealed.len() < header + PLANNER_TAG_LEN
        || sealed.len() > header + PLANNER_TAG_LEN + PLANNER_PLAINTEXT_LIMIT
        || !sealed.starts_with(PLANNER_SEAL_PREFIX)
    {
        return Err("The encrypted planner item has an invalid format.".into());
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "GhostChat could not use the planner key.".to_string())?;
    let aad = planner_aad(conversation_id, kind, item_id, epoch);
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&sealed[PLANNER_SEAL_PREFIX.len()..header]),
                Payload {
                    msg: &sealed[header..],
                    aad: &aad,
                },
            )
            .map_err(|_| "The encrypted planner item could not be authenticated.".to_string())?,
    );
    String::from_utf8(plaintext.to_vec())
        .map_err(|_| "The encrypted planner item is not valid text.".to_string())
}

fn valid_key_id(key_id: &str) -> bool {
    key_id.len() == 32
        && key_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The current planner epoch and key id of an encrypted group on this
/// device (`epoch` = `None`), or whether this device holds the key of a
/// given epoch (`Some`). `None`: no key (no group here, removed, or an
/// epoch this device never had).
pub async fn planner_key(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    epoch: Option<u64>,
) -> Result<Option<PlannerKeyInfo>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    prepare_crypto_storage(db).await?;
    let current = current_planner_key(db, &user_id, &conversation_id).await?;
    match epoch {
        None => Ok(current.map(|key| PlannerKeyInfo {
            epoch: key.epoch,
            key_id: key.key_id,
        })),
        Some(epoch) => {
            let Ok(stored_epoch) = i64::try_from(epoch) else {
                return Ok(None);
            };
            let key_id = sqlx::query_scalar::<_, String>(
                "SELECT key_id FROM ghostchat_planner_keys WHERE user_id=? AND conversation_id=? AND epoch=? ORDER BY rowid DESC LIMIT 1",
            )
            .bind(&user_id)
            .bind(&conversation_id)
            .bind(stored_epoch)
            .fetch_optional(db)
            .await
            .map_err(|_| "GhostChat could not load the planner key cache.".to_string())?;
            Ok(key_id.map(|key_id| PlannerKeyInfo { epoch, key_id }))
        }
    }
}

/// Seals a planner payload with the group's current planner key.
pub async fn planner_seal(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    kind: &str,
    item_id: &str,
    plaintext: &str,
) -> Result<PlannerSealed, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let kind = normalized_planner_kind(kind)?;
    let item_id = normalized_planner_item_id(kind, item_id)?;
    if matches!(kind, "poll_vote" | "event_rsvp") && !item_id.ends_with(&format!("/{user_id}")) {
        return Err("A planner response can only be sealed for this account.".into());
    }
    prepare_crypto_storage(db).await?;
    let key = current_planner_key(db, &user_id, &conversation_id)
        .await?
        .ok_or_else(|| {
            "This device has not joined the encrypted group, so it cannot add planner items."
                .to_string()
        })?;
    let ciphertext = seal_planner_payload(
        &key.key,
        &conversation_id,
        kind,
        &item_id,
        key.epoch,
        plaintext,
    )?;
    Ok(PlannerSealed {
        ciphertext,
        epoch: key.epoch,
        key_id: key.key_id,
    })
}

/// Opens a sealed planner payload. `Ok(None)`: this device does not hold the
/// key of that epoch (the UI shows the item as locked). An authentication
/// failure (tampered payload or wrong item binding) is an error.
#[allow(clippy::too_many_arguments)]
pub async fn planner_open(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    kind: &str,
    item_id: &str,
    epoch: u64,
    key_id: &str,
    ciphertext: &str,
) -> Result<Option<String>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let kind = normalized_planner_kind(kind)?;
    let item_id = normalized_planner_item_id(kind, item_id)?;
    if !valid_key_id(key_id) {
        return Err("The planner key id is invalid.".into());
    }
    prepare_crypto_storage(db).await?;
    let mut key = cached_planner_key(db, &user_id, &conversation_id, epoch, key_id).await?;
    if key.is_none() {
        // A device that joined or moved epochs without running a planner
        // command caches its current key here.
        if let Some(current) = current_planner_key(db, &user_id, &conversation_id).await? {
            if current.epoch == epoch && current.key_id == key_id {
                key = Some(current.key);
            }
        }
    }
    let Some(key) = key else {
        return Ok(None);
    };
    open_planner_payload(&key, &conversation_id, kind, &item_id, epoch, ciphertext).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
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

    #[test]
    fn sealed_payload_round_trips_and_binds_its_aad() {
        let key = [7u8; 32];
        let conversation = Uuid::new_v4().to_string();
        let item = Uuid::new_v4().to_string();
        let sealed =
            seal_planner_payload(&key, &conversation, "poll", &item, 3, "{\"q\":\"When?\"}")
                .unwrap();
        assert!(!sealed.contains("When"));
        assert_eq!(
            open_planner_payload(&key, &conversation, "poll", &item, 3, &sealed).unwrap(),
            "{\"q\":\"When?\"}"
        );
        let again =
            seal_planner_payload(&key, &conversation, "poll", &item, 3, "{\"q\":\"When?\"}")
                .unwrap();
        assert_ne!(sealed, again, "every write uses a fresh nonce");
        let other = Uuid::new_v4().to_string();
        for (conversation_id, kind, item_id, epoch) in [
            (other.as_str(), "poll", item.as_str(), 3),
            (conversation.as_str(), "event", item.as_str(), 3),
            (conversation.as_str(), "poll", other.as_str(), 3),
            (conversation.as_str(), "poll", item.as_str(), 4),
        ] {
            assert!(
                open_planner_payload(&key, conversation_id, kind, item_id, epoch, &sealed).is_err(),
                "a payload opened with the wrong AAD must fail"
            );
        }
        assert!(
            open_planner_payload(&[8u8; 32], &conversation, "poll", &item, 3, &sealed).is_err()
        );
        let mut tampered = URL_SAFE_NO_PAD.decode(&sealed).unwrap();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open_planner_payload(
            &key,
            &conversation,
            "poll",
            &item,
            3,
            &URL_SAFE_NO_PAD.encode(tampered)
        )
        .is_err());
        assert!(seal_planner_payload(&key, &conversation, "poll", &item, 3, "").is_err());
        assert!(seal_planner_payload(
            &key,
            &conversation,
            "poll",
            &item,
            3,
            &"x".repeat(PLANNER_PLAINTEXT_LIMIT + 1)
        )
        .is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn planner_keys_follow_the_mls_epoch_and_exclude_a_removed_member() {
        let root = std::env::temp_dir().join(format!("ghostchat-planner-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
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
        assert_eq!(
            planner_key(&alice_db, &alice_id, &conversation_id, None)
                .await
                .unwrap(),
            None,
            "no group, no planner key"
        );
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
        let first = planner_key(&alice_db, &alice_id, &conversation_id, None)
            .await
            .unwrap()
            .expect("Alice derives a planner key");
        for (db, id) in [(&bob_db, &bob_id), (&mallory_db, &mallory_id)] {
            assert_eq!(
                planner_key(db, id, &conversation_id, None).await.unwrap(),
                Some(first.clone()),
                "every member at the same epoch derives the same key"
            );
        }

        // An item sealed by Bob opens for Alice and Mallory, bound to its AAD.
        let poll_id = Uuid::new_v4().to_string();
        let sealed = planner_seal(
            &bob_db,
            &bob_id,
            &conversation_id,
            "poll",
            &poll_id,
            "{\"question\":\"Dinner?\"}",
        )
        .await
        .unwrap();
        assert_eq!(sealed.epoch, first.epoch);
        assert_eq!(sealed.key_id, first.key_id);
        assert!(!sealed.ciphertext.contains("Dinner"));
        for (db, id) in [(&alice_db, &alice_id), (&mallory_db, &mallory_id)] {
            assert_eq!(
                planner_open(
                    db,
                    id,
                    &conversation_id,
                    "poll",
                    &poll_id,
                    sealed.epoch,
                    &sealed.key_id,
                    &sealed.ciphertext,
                )
                .await
                .unwrap()
                .as_deref(),
                Some("{\"question\":\"Dinner?\"}")
            );
        }
        assert!(planner_open(
            &alice_db,
            &alice_id,
            &conversation_id,
            "event",
            &poll_id,
            sealed.epoch,
            &sealed.key_id,
            &sealed.ciphertext,
        )
        .await
        .is_err());
        // A response is sealed for its own account only.
        assert!(planner_seal(
            &bob_db,
            &bob_id,
            &conversation_id,
            "poll_vote",
            &format!("{poll_id}/{alice_id}"),
            "{\"option\":0}",
        )
        .await
        .is_err());

        // Alice removes Mallory: the epoch moves and the key changes.
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
        let _ = process_crypto_event(
            &mallory_db,
            &mallory_id,
            &conversation_id,
            &alice_id,
            None,
            Some(2),
            true,
            &remaining,
            &removal.commit_payload,
        )
        .await;
        let second = planner_key(&alice_db, &alice_id, &conversation_id, None)
            .await
            .unwrap()
            .unwrap();
        assert!(second.epoch > first.epoch);
        assert_ne!(second.key_id, first.key_id, "a commit replaces the key");
        assert_eq!(
            planner_key(&bob_db, &bob_id, &conversation_id, None)
                .await
                .unwrap(),
            Some(second.clone())
        );
        let mallory_now = planner_key(&mallory_db, &mallory_id, &conversation_id, None)
            .await
            .unwrap();
        assert!(
            mallory_now.as_ref() != Some(&second),
            "the removed member cannot derive the new key ({mallory_now:?})"
        );

        let event_id = Uuid::new_v4().to_string();
        let newer = planner_seal(
            &alice_db,
            &alice_id,
            &conversation_id,
            "event",
            &event_id,
            "{\"title\":\"After\"}",
        )
        .await
        .unwrap();
        assert_eq!(newer.key_id, second.key_id);
        assert_eq!(
            planner_open(
                &mallory_db,
                &mallory_id,
                &conversation_id,
                "event",
                &event_id,
                newer.epoch,
                &newer.key_id,
                &newer.ciphertext,
            )
            .await
            .unwrap(),
            None,
            "the removed member's device cannot open a new item"
        );
        assert_eq!(
            planner_open(
                &bob_db,
                &bob_id,
                &conversation_id,
                "event",
                &event_id,
                newer.epoch,
                &newer.key_id,
                &newer.ciphertext,
            )
            .await
            .unwrap()
            .as_deref(),
            Some("{\"title\":\"After\"}")
        );
        // Items of the earlier epoch stay readable for members from the
        // local key cache, which OpenMLS itself does not keep.
        assert_eq!(
            planner_open(
                &alice_db,
                &alice_id,
                &conversation_id,
                "poll",
                &poll_id,
                sealed.epoch,
                &sealed.key_id,
                &sealed.ciphertext,
            )
            .await
            .unwrap()
            .as_deref(),
            Some("{\"question\":\"Dinner?\"}")
        );
        assert_eq!(
            planner_key(&bob_db, &bob_id, &conversation_id, Some(first.epoch))
                .await
                .unwrap(),
            Some(first.clone())
        );
        let raw_cache = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT encrypted_key FROM ghostchat_planner_keys LIMIT 1",
        )
        .fetch_one(&bob_db)
        .await
        .unwrap();
        assert!(
            raw_cache.starts_with(b"GC\x01"),
            "cached keys are encrypted at rest"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_epoch_passed_without_planner_use_is_still_cached() {
        // Bob never runs a planner command before Alice's commit moves the
        // epoch; the key of the epoch he left is cached by the commit itself.
        let root = std::env::temp_dir().join(format!("ghostchat-planner-cache-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let _directory = TestDirectory(root.clone());
        let alice_db = open_database(&root.join("alice.sqlite")).await;
        let bob_db = open_database(&root.join("bob.sqlite")).await;
        let carol_db = open_database(&root.join("carol.sqlite")).await;
        let alice_id = Uuid::new_v4().to_string();
        let bob_id = Uuid::new_v4().to_string();
        let carol_id = Uuid::new_v4().to_string();
        let conversation_id = Uuid::new_v4().to_string();
        prepare_device(&alice_db, &alice_id).await.unwrap();
        let bob_device = prepare_device(&bob_db, &bob_id).await.unwrap();
        let carol_device = prepare_device(&carol_db, &carol_id).await.unwrap();
        let mut roster = vec![alice_id.clone(), bob_id.clone(), carol_id.clone()];
        roster.sort();
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
        // Alice seals in the first epoch, without Bob calling the planner.
        let poll_id = Uuid::new_v4().to_string();
        let sealed = planner_seal(
            &alice_db,
            &alice_id,
            &conversation_id,
            "poll",
            &poll_id,
            "{\"question\":\"Old?\"}",
        )
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
            planner_open(
                &bob_db,
                &bob_id,
                &conversation_id,
                "poll",
                &poll_id,
                sealed.epoch,
                &sealed.key_id,
                &sealed.ciphertext,
            )
            .await
            .unwrap()
            .as_deref(),
            Some("{\"question\":\"Old?\"}"),
            "the epoch Bob left is in his key cache"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_key_cache_is_bounded_per_group() {
        let root = std::env::temp_dir().join(format!("ghostchat-planner-bound-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let _directory = TestDirectory(root.clone());
        let db = open_database(&root.join("profile.sqlite")).await;
        let user_id = Uuid::new_v4().to_string();
        prepare_device(&db, &user_id).await.unwrap();
        prepare_crypto_storage(&db).await.unwrap();
        let conversation_id = Uuid::new_v4().to_string();
        let mut connection = db.acquire().await.unwrap();
        for epoch in 0..(PLANNER_KEY_CACHE_LIMIT as u64 + 10) {
            let key = Zeroizing::new([epoch as u8; 32]);
            let entry = PlannerEpochKey {
                epoch,
                key_id: planner_key_id(&conversation_id, &key),
                key,
            };
            store_planner_keys(
                &mut connection,
                &user_id,
                &conversation_id,
                std::slice::from_ref(&entry),
            )
            .await
            .unwrap();
        }
        let (count, oldest) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT count(*),min(epoch) FROM ghostchat_planner_keys WHERE conversation_id=?",
        )
        .bind(&conversation_id)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(count, PLANNER_KEY_CACHE_LIMIT);
        assert_eq!(oldest, 10, "the oldest keys are dropped first");
    }
}
