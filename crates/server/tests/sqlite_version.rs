//! RUSTSEC-2026-0459 / CVE-2026-119375: SQLite before 3.54.0 can return
//! stale page-cache memory when it opens a database together with a crafted
//! `-wal` file. `sqlx` 0.8 only accepts `libsqlite3-sys` 0.30, which bundles
//! SQLite 3.46.0, so the workspace patches that crate with the SQLite 3.54.1
//! amalgamation (vendor/libsqlite3-sys/GHOSTCHAT_PATCH.md). This test proves
//! the SQLite that sqlx actually links is the fixed one.

/// First SQLite release with the fix, as `SQLITE_VERSION_NUMBER`.
const FIXED_SQLITE_VERSION_NUMBER: i64 = 3_054_000;

fn version_number(version: &str) -> i64 {
    let mut parts = version.split('.').map(|p| p.parse::<i64>().unwrap());
    let major = parts.next().unwrap();
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    major * 1_000_000 + minor * 1_000 + patch
}

#[tokio::test]
async fn linked_sqlite_has_wal_stale_memory_fix() {
    let dir =
        std::env::temp_dir().join(format!("ghostchat-sqlite-version-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("server.sqlite");
    let pool = ghostchat_server::open_database(path.to_str().unwrap())
        .await
        .unwrap();

    let version: String = sqlx::query_scalar("select sqlite_version()")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        version_number(&version) >= FIXED_SQLITE_VERSION_NUMBER,
        "linked SQLite {version} is older than 3.54.0 (RUSTSEC-2026-0459)"
    );
}
