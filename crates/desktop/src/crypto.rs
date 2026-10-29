use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
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
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ghostchat_crypto_text_cache (\
            client_message_id TEXT PRIMARY KEY NOT NULL,\
            conversation_id TEXT NOT NULL,\
            encrypted_content BLOB NOT NULL,\
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))\
        )",
    )
    .execute(db)
    .await
    .map_err(|_| "GhostChat could not prepare its encrypted message cache.".to_string())?;
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

    let mut connection = db
        .acquire()
        .await
        .map_err(|_| "GhostChat could not open its local encryption database.".to_string())?;
    SqliteStorageProvider::<EncryptedJsonCodec>::new(&mut *connection)
        .run_migrations()
        .map_err(|_| "GhostChat could not initialize its encrypted MLS storage.".to_string())
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

fn group_roster_matches(
    group: &MlsGroup,
    expected_member_ids: &std::collections::HashSet<String>,
    own_identity: &str,
) -> bool {
    let Some(identities) = group_account_identities(group) else {
        return false;
    };
    identities.len() == expected_member_ids.len()
        && identities
            .keys()
            .all(|user_id| expected_member_ids.contains(user_id))
        && identities
            .values()
            .any(|(credential, _)| credential == own_identity.as_bytes())
}

fn existing_group_identities_unchanged(
    group: &MlsGroup,
    existing: &std::collections::HashMap<String, (Vec<u8>, Vec<u8>)>,
) -> bool {
    let Some(updated) = group_account_identities(group) else {
        return false;
    };
    existing
        .iter()
        .all(|(user_id, identity)| updated.get(user_id) == Some(identity))
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
            | "openmls::ciphersuite::hash_ref::HashReference"
            | "openmls::group::GroupId"
            | "openmls::group::GroupEpoch"
    )
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

pub async fn prepare_group_membership_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    peers: &[GroupPeerKeyPackage],
) -> Result<GroupMembershipUpdate, String> {
    prepare_group_membership_change(
        db,
        user_id,
        conversation_id,
        expected_member_ids,
        "add",
        &[],
        peers,
    )
    .await
}

pub async fn prepare_group_removal_update(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    removed_member_ids: &[String],
) -> Result<GroupMembershipUpdate, String> {
    prepare_group_membership_change(
        db,
        user_id,
        conversation_id,
        expected_member_ids,
        "remove",
        removed_member_ids,
        &[],
    )
    .await
}

async fn prepare_group_membership_change(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    expected_member_ids: &[String],
    action: &str,
    selected_member_ids: &[String],
    peers: &[GroupPeerKeyPackage],
) -> Result<GroupMembershipUpdate, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    if action == "add" && (peers.is_empty() || peers.len() > 15 || !selected_member_ids.is_empty())
    {
        return Err("Choose 1 to 15 friends to add to the encrypted group.".into());
    }
    if action == "remove" && (selected_member_ids.len() != 1 || !peers.is_empty()) {
        return Err("Choose exactly one encrypted group member to remove.".into());
    }
    if !matches!(action, "add" | "remove") {
        return Err("Unsupported encrypted membership operation.".into());
    }

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
        return Ok(pending);
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
        if members.len() != expected_member_ids.len()
            || actual_member_ids.len() != expected_member_ids.len()
            || actual_member_ids != current_set.into_iter().cloned().collect()
            || !members
                .iter()
                .any(|member| member.credential.serialized_content() == own_identity.as_bytes())
        {
            return Err("The local MLS group does not match the current server roster.".into());
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
    Ok(update)
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
) -> Result<GroupMembershipUpdate, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
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

    let existing = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT encrypted_content FROM ghostchat_crypto_text_cache WHERE client_message_id=?",
    )
    .bind(&client_message_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| "GhostChat could not load the encrypted message cache.".to_string())?;
    if let Some(existing) = existing {
        let existing: String = EncryptedJsonCodec::from_slice(&existing)
            .map_err(|_| "The local encrypted message cache could not be decrypted.".to_string())?;
        if existing != content {
            return Err("This message ID is already associated with different content.".into());
        }
    } else {
        sqlx::query("INSERT INTO ghostchat_crypto_text_cache(client_message_id,conversation_id,encrypted_content) VALUES(?,?,?)")
            .bind(&client_message_id)
            .bind(&conversation_id)
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
    transaction
        .commit()
        .await
        .map_err(|_| "GhostChat could not save the local encryption state.".to_string())?;
    Ok(EncryptedApplicationMessage { payload })
}

pub async fn get_cached_sent_text(
    db: &SqlitePool,
    client_message_id: &str,
) -> Result<Option<String>, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let client_message_id = normalized_uuid(client_message_id, "message")?;
    prepare_crypto_storage(db).await?;
    let encrypted_content = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT encrypted_content FROM ghostchat_crypto_text_cache WHERE client_message_id=?",
    )
    .bind(&client_message_id)
    .fetch_optional(db)
    .await
    .map_err(|_| "GhostChat could not load the encrypted message cache.".to_string())?;
    encrypted_content
        .map(|bytes| {
            EncryptedJsonCodec::from_slice(&bytes).map_err(|_| {
                "The local encrypted message cache could not be decrypted.".to_string()
            })
        })
        .transpose()
}

pub async fn process_crypto_event(
    db: &SqlitePool,
    user_id: &str,
    conversation_id: &str,
    sender_id: &str,
    client_message_id: Option<&str>,
    event_sequence: i64,
    conversation_is_group: bool,
    expected_member_ids: &[String],
    payload: &str,
) -> Result<CryptoEventResult, String> {
    let _operation = CRYPTO_OPERATION_LOCK.lock().await;
    let user_id = normalized_uuid(user_id, "account")?;
    let conversation_id = normalized_uuid(conversation_id, "conversation")?;
    let sender_id = normalized_uuid(sender_id, "sender account")?;
    if event_sequence <= 0 {
        return Err("The encrypted event sequence is invalid.".into());
    }
    let expected_member_ids = expected_member_ids
        .iter()
        .map(|member_id| normalized_uuid(member_id, "conversation member"))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    if expected_member_ids.len() < 2 || expected_member_ids.len() > 16 {
        return Err("The expected MLS conversation roster is invalid.".into());
    }
    let client_message_id = client_message_id
        .map(|message_id| normalized_uuid(message_id, "message"))
        .transpose()?;
    prepare_crypto_storage(db).await?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|_| "The encrypted event has invalid encoding.".to_string())?;
    if bytes.is_empty() || bytes.len() > 3 * 1024 * 1024 {
        return Err("The encrypted event has an invalid size.".into());
    }
    let message = MlsMessageIn::tls_deserialize_exact(bytes.as_slice())
        .map_err(|_| "The encrypted event could not be decoded as MLS data.".to_string())?;
    let is_welcome = matches!(message.clone().extract(), MlsMessageBodyIn::Welcome(_));
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
    if last_sequence.is_some_and(|last| event_sequence <= last) {
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
            _ => {
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
                let protocol_message = message.try_into_protocol_message().map_err(|_| {
                    "This MLS event is not a processable group message.".to_string()
                })?;
                if protocol_message.group_id().as_slice() != group_id.as_slice() {
                    return Err("The MLS event belongs to a different conversation.".into());
                }
                let processed =
                    group
                        .process_message(&provider, protocol_message)
                        .map_err(|_| {
                            "GhostChat could not authenticate or decrypt this MLS event."
                                .to_string()
                        })?;
                if account_id_from_credential(processed.credential().serialized_content())
                    != Some(sender_id.clone())
                {
                    return Err("The MLS sender identity does not match the relay account.".into());
                }
                match processed.into_content() {
                    ProcessedMessageContent::ApplicationMessage(application) => {
                        if !group_roster_matches(&group, &expected_member_ids, &own_identity)
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
                        if !group_roster_matches(&group, &expected_member_ids, &own_identity)
                            || !existing_group_identities_unchanged(
                                &group,
                                &existing_group_identities,
                            )
                        {
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
                        if !group_roster_matches(&group, &expected_member_ids, &own_identity) {
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
    if let (Some(client_message_id), Some(content)) =
        (client_message_id.as_deref(), cache_content.as_deref())
    {
        let encrypted_content = EncryptedJsonCodec::to_vec(content)
            .map_err(|_| "GhostChat could not encrypt the local message cache.".to_string())?;
        sqlx::query("INSERT OR IGNORE INTO ghostchat_crypto_text_cache(client_message_id,conversation_id,encrypted_content) VALUES(?,?,?)")
            .bind(client_message_id)
            .bind(&conversation_id)
            .bind(encrypted_content)
            .execute(&mut *transaction)
            .await
            .map_err(|_| "GhostChat could not save the encrypted local message cache.".to_string())?;
    }
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
        if is_mls_storage_lookup_key(std::any::type_name::<T>()) {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&**key)
                .map_err(|_| CryptoCodecError("Local storage key is invalid".into()))?;
            mac.update(b"GhostChat MLS storage lookup key v1\0");
            mac.update(&plaintext);
            plaintext.zeroize();
            let digest = mac.finalize().into_bytes();
            let mut encoded = Vec::with_capacity(4 + digest.len());
            encoded.extend_from_slice(b"GCK\x01");
            encoded.extend_from_slice(&digest);
            return Ok(encoded);
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
            1,
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
        assert_eq!(
            get_cached_sent_text(&alice_db, &first_id)
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
            2,
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
            2,
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
            3,
            false,
            &expected_members,
            &second.payload,
        )
        .await
        .expect("Bob should decrypt the next message after restart and replay");
        assert_eq!(second_received.kind, "application");
        assert_eq!(second_received.content.as_deref(), Some(second_text));
    }
}
