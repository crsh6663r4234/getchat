//! Opt-in isolated SQLCipher vault. This module never imports a legacy database.
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    ConnectOptions, SqlitePool,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    vault_id: uuid::Uuid,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
    wrap_salt: String,
    nonce: String,
    wrapped_key: String,
}

fn decode<const N: usize>(value: &str) -> Result<[u8; N], String> {
    STANDARD
        .decode(value)
        .map_err(|_| "Invalid vault metadata".to_string())?
        .try_into()
        .map_err(|_| "Invalid vault metadata length".to_string())
}

fn canonical_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Vault path must be absolute".into());
    }
    let parent = path
        .parent()
        .ok_or("Missing vault parent")?
        .canonicalize()
        .map_err(|_| "Vault parent must already exist")?;
    let name = path.file_name().ok_or("Missing vault filename")?;
    let result = parent.join(name);
    if result.to_str().is_none() {
        return Err("Vault path must be UTF-8".into());
    }
    Ok(result)
}

fn aad(path: &Path, m: &Manifest) -> Vec<u8> {
    let path = path.to_str().expect("validated UTF-8 path").as_bytes();
    let mut result = b"GhostChat SQLCipher vault wrapping v1\0".to_vec();
    result.extend_from_slice(&m.version.to_be_bytes());
    result.extend_from_slice(m.vault_id.as_bytes());
    result.extend_from_slice(&(path.len() as u64).to_be_bytes());
    result.extend_from_slice(path);
    result
}

fn wrapping_key(
    m: &Manifest,
    passphrase: &[u8],
    os_secret: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, String> {
    if m.version != 1 || m.memory_kib != 65536 || m.iterations != 3 || m.parallelism != 4 {
        return Err("Unsupported vault metadata or KDF parameters".into());
    }
    let salt = decode::<16>(&m.salt)?;
    let wrap_salt = decode::<16>(&m.wrap_salt)?;
    let mut derived = Zeroizing::new([0u8; 32]);
    let params = Params::new(65536, 3, 4, Some(32)).map_err(|_| "Invalid vault KDF")?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase, &salt, &mut *derived)
        .map_err(|_| "Vault KDF failed")?;
    let mut input = Zeroizing::new([0u8; 64]);
    input[..32].copy_from_slice(&*derived);
    input[32..].copy_from_slice(os_secret);
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&wrap_salt), &*input)
        .expand(b"GhostChat SQLCipher vault KEK v1", &mut *key)
        .map_err(|_| "Vault key derivation failed")?;
    Ok(key)
}

fn regular_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() => Ok(true),
        Ok(_) => Err("Vault files must be regular files, without symbolic links".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("Cannot inspect vault files".into()),
    }
}

/// Opens an authenticated, path-bound isolated vault. Requires the sqlcipher-vault feature.
pub async fn open_or_create(
    path: &Path,
    passphrase: &[u8],
    os_secret: &[u8; 32],
) -> Result<SqlitePool, String> {
    if passphrase.is_empty() || passphrase.len() > 1024 {
        return Err("Vault passphrase must contain 1 to 1024 bytes".into());
    }
    let path = canonical_path(path)?;
    let mut metadata_name = path.as_os_str().to_os_string();
    metadata_name.push(".vault.json");
    let metadata = PathBuf::from(metadata_name);
    let exists = regular_exists(&path)?;
    if exists != regular_exists(&metadata)? {
        return Err("Incomplete vault: database and manifest must both exist; automatic recovery is forbidden".into());
    }
    if !exists {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            if fs::symlink_metadata(PathBuf::from(sidecar)).is_ok() {
                return Err("Orphan vault sidecar exists".into());
            }
        }
    }
    let mut dek = Zeroizing::new([0u8; 32]);
    let m: Manifest;
    if exists {
        if fs::metadata(&metadata)
            .map_err(|_| "Cannot inspect vault manifest")?
            .len()
            > 16384
        {
            return Err("Vault manifest exceeds size limit".into());
        }
        m = serde_json::from_slice(&fs::read(&metadata).map_err(|_| "Cannot read vault manifest")?)
            .map_err(|_| "Invalid vault manifest")?;
        let key = wrapping_key(&m, passphrase, os_secret)?;
        let nonce = decode::<12>(&m.nonce)?;
        let encrypted = decode::<48>(&m.wrapped_key)?;
        let plain = Zeroizing::new(
            Aes256Gcm::new_from_slice(&*key)
                .map_err(|_| "Invalid wrapping key")?
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &encrypted,
                        aad: &aad(&path, &m),
                    },
                )
                .map_err(|_| "Vault authentication failed")?,
        );
        if plain.len() != 32 {
            return Err("Invalid vault key length".into());
        }
        dek.copy_from_slice(&plain);
    } else {
        let mut salt = [0u8; 16];
        let mut wrap_salt = [0u8; 16];
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut salt);
        OsRng.fill_bytes(&mut wrap_salt);
        OsRng.fill_bytes(&mut nonce);
        OsRng.fill_bytes(&mut *dek);
        let mut fresh = Manifest {
            version: 1,
            vault_id: uuid::Uuid::new_v4(),
            memory_kib: 65536,
            iterations: 3,
            parallelism: 4,
            salt: STANDARD.encode(salt),
            wrap_salt: STANDARD.encode(wrap_salt),
            nonce: STANDARD.encode(nonce),
            wrapped_key: String::new(),
        };
        let key = wrapping_key(&fresh, passphrase, os_secret)?;
        fresh.wrapped_key = STANDARD.encode(
            Aes256Gcm::new_from_slice(&*key)
                .map_err(|_| "Invalid wrapping key")?
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &*dek,
                        aad: &aad(&path, &fresh),
                    },
                )
                .map_err(|_| "Vault wrapping failed")?,
        );
        // create_new is the exclusive first-use reservation. Interrupted creation stays blocked.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&metadata)
            .map_err(|_| "Vault manifest already exists or cannot be created")?;
        file.write_all(&serde_json::to_vec(&fresh).map_err(|_| "Cannot serialize vault manifest")?)
            .map_err(|_| "Cannot write vault manifest")?;
        file.sync_all()
            .map_err(|_| "Cannot persist vault manifest")?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|f| f.sync_all())
            .map_err(|_| "Cannot reserve fresh vault database")?;
        m = fresh;
    }
    let mut hex = Zeroizing::new(String::with_capacity(68));
    hex.push_str("\"x'");
    use std::fmt::Write as _;
    for b in dek.iter() {
        write!(&mut *hex, "{b:02x}").map_err(|_| "Cannot encode vault key")?;
    }
    hex.push_str("'\"");
    // SQLx explicitly orders key first, before its journal/schema pragmas. Logging is disabled.
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(false)
        .pragma("key", hex.to_string())
        .pragma("cipher_compatibility", "4")
        .pragma("cipher_plaintext_header_size", "0")
        .pragma("cipher_use_hmac", "ON")
        .pragma("temp_store", "MEMORY")
        .pragma("cipher_memory_security", "ON")
        .busy_timeout(Duration::from_secs(5))
        .disable_statement_logging();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| "Cannot unlock encrypted vault database")?;
    let validated = async {
        let version: Option<String> = sqlx::query_scalar("PRAGMA cipher_version").fetch_optional(&pool).await.map_err(|_| "Cannot verify SQLCipher")?;
        if !version.is_some_and(|v| v.starts_with("4.")) { return Err("SQLCipher 4 is required; ordinary SQLite is forbidden".to_string()); }
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sqlite_master").fetch_one(&pool).await.map_err(|_| "Vault database authentication failed")?;
        if !exists {
            sqlx::query("CREATE TABLE ghostchat_vault_identity (id INTEGER PRIMARY KEY CHECK(id=1), vault_id TEXT NOT NULL)").execute(&pool).await.map_err(|_| "Cannot initialize vault identity")?;
            sqlx::query("INSERT INTO ghostchat_vault_identity VALUES (1, ?)").bind(m.vault_id.to_string()).execute(&pool).await.map_err(|_| "Cannot persist vault identity")?;
        }
        let stored: String = sqlx::query_scalar("SELECT vault_id FROM ghostchat_vault_identity WHERE id=1").fetch_one(&pool).await.map_err(|_| "Vault identity is missing")?;
        if stored != m.vault_id.to_string() { return Err("Vault identity mismatch".into()); }
        Ok::<_, String>(())
    }.await;
    if let Err(e) = validated {
        pool.close().await;
        return Err(e);
    }
    Ok(pool)
}

#[cfg(all(test, feature = "sqlcipher-vault"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn encrypted_roundtrip_and_fail_closed_credentials() {
        let directory =
            std::env::temp_dir().join(format!("ghostchat-vault-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("vault.sqlite");
        let secret = [71u8; 32];
        let pool = open_or_create(&path, b"test passphrase", &secret)
            .await
            .unwrap();
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE private_data (body TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let marker = "GHOSTCHAT_PLAINTEXT_MARKER_DO_NOT_STORE_671389";
        sqlx::query("INSERT INTO private_data VALUES (?)")
            .bind(marker)
            .execute(&pool)
            .await
            .unwrap();
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|x| x == "json") {
                continue;
            }
            let bytes = fs::read(entry.path()).unwrap();
            assert!(!bytes.windows(marker.len()).any(|w| w == marker.as_bytes()));
            assert!(!bytes.starts_with(b"SQLite format 3\0"));
        }
        pool.close().await;
        assert!(open_or_create(&path, b"wrong passphrase", &secret)
            .await
            .is_err());
        assert!(open_or_create(&path, b"test passphrase", &[72u8; 32])
            .await
            .is_err());
        let pool = open_or_create(&path, b"test passphrase", &secret)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT body FROM private_data")
                .fetch_one(&pool)
                .await
                .unwrap(),
            marker
        );
        pool.close().await;
        // Corrupt authenticated encrypted page; wrapped DEK still authenticates but DB must not.
        let mut bytes = fs::read(&path).unwrap();
        bytes[100] ^= 0x40;
        fs::write(&path, bytes).unwrap();
        assert!(open_or_create(&path, b"test passphrase", &secret)
            .await
            .is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn no_plaintext_migration_or_orphan_reinitialization() {
        let directory =
            std::env::temp_dir().join(format!("ghostchat-vault-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("legacy.sqlite");
        fs::write(&path, b"SQLite format 3\0legacy").unwrap();
        assert!(open_or_create(&path, b"passphrase", &[1; 32])
            .await
            .is_err());
        let orphan = directory.join("orphan.sqlite");
        fs::write(directory.join("orphan.sqlite.vault.json"), b"{}").unwrap();
        assert!(open_or_create(&orphan, b"passphrase", &[1; 32])
            .await
            .is_err());
        assert!(!orphan.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
