//! Opening the SQLite database, migrations and the pre-upgrade backups.

use super::*;

pub async fn open_database(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    let migrator = sqlx::migrate!("./migrations");
    backup_before_upgrade(&pool, path, &migrator).await?;
    migrator
        .run(&pool)
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    Ok(pool)
}

/// `open_database` stopped because the pre-upgrade backup could not be
/// written; nothing was migrated. Carried in `sqlx::Error::Configuration`.
#[derive(Debug)]
pub struct PreUpgradeBackupError(pub(crate) String);

impl std::fmt::Display for PreUpgradeBackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PreUpgradeBackupError {}

/// Whether `open_database` failed because its pre-upgrade backup could not
/// be written (and so nothing was migrated).
pub fn is_pre_upgrade_backup_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Configuration(inner) if inner.is::<PreUpgradeBackupError>())
}

/// Migrations from this version on cannot be run by an older server binary
/// and have no reverse migration, so a backup is the only way back.
pub(crate) const FIRST_ONE_WAY_MIGRATION: i64 = 25;

/// Writes `<database>.pre-<first pending version>.bak` with `VACUUM INTO`
/// before an upgrade that applies a one-way migration to a database that
/// already holds data. Skips in-memory and brand-new databases and never
/// overwrites an existing backup. A failure stops startup before migrating.
pub(crate) async fn backup_before_upgrade(
    pool: &SqlitePool,
    path: &str,
    migrator: &sqlx::migrate::Migrator,
) -> Result<(), sqlx::Error> {
    if path.is_empty() || path == ":memory:" || path.contains("mode=memory") {
        return Ok(());
    }
    let has_history = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await?
        > 0;
    if !has_history {
        return Ok(());
    }
    let applied: std::collections::HashSet<i64> =
        sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    if applied.is_empty() {
        return Ok(());
    }
    let pending: Vec<i64> = migrator
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .filter(|version| !applied.contains(version))
        .collect();
    if !pending
        .iter()
        .any(|version| *version >= FIRST_ONE_WAY_MIGRATION)
    {
        return Ok(());
    }
    let first = pending.iter().copied().min().unwrap_or_default();
    let backup = PathBuf::from(format!("{path}.pre-{first:04}.bak"));
    if backup.exists() {
        tracing::info!(backup = %backup.display(), "pre-upgrade backup already exists");
        return Ok(());
    }
    let failed = |reason: String| {
        sqlx::Error::Configuration(Box::new(PreUpgradeBackupError(format!(
            "could not write the pre-upgrade backup {}: {reason}; the database was not migrated",
            backup.display()
        ))))
    };
    // VACUUM INTO refuses an existing target, so a stale partial file from an
    // interrupted attempt is removed first; the rename makes the final name
    // appear only once the backup is complete.
    let partial = PathBuf::from(format!("{}.tmp", backup.display()));
    if partial.exists() {
        std::fs::remove_file(&partial).map_err(|e| failed(e.to_string()))?;
    }
    sqlx::query("VACUUM INTO ?")
        .bind(partial.to_string_lossy().into_owned())
        .execute(pool)
        .await
        .map_err(|e| failed(e.to_string()))?;
    std::fs::rename(&partial, &backup).map_err(|e| failed(e.to_string()))?;
    tracing::info!(backup = %backup.display(), "wrote the pre-upgrade database backup");
    rotate_upgrade_backups(&backup);
    Ok(())
}

/// Writes `<database>.pre-update.bak` with `VACUUM INTO` before the
/// standalone server's self-updater replaces the exe; the next one replaces
/// it. Skips in-memory databases (`Ok(None)`).
pub(crate) async fn backup_before_self_update(
    pool: &SqlitePool,
    path: &str,
) -> Result<Option<PathBuf>, String> {
    if path.is_empty() || path == ":memory:" || path.contains("mode=memory") {
        return Ok(None);
    }
    let backup = PathBuf::from(format!("{path}.pre-update.bak"));
    let partial = PathBuf::from(format!("{}.tmp", backup.display()));
    if partial.exists() {
        std::fs::remove_file(&partial).map_err(|e| e.to_string())?;
    }
    sqlx::query("VACUUM INTO ?")
        .bind(partial.to_string_lossy().into_owned())
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &backup).map_err(|e| e.to_string())?;
    tracing::info!(backup = %backup.display(), "wrote the database backup before the self-update");
    Ok(Some(backup))
}

/// How many `pre-<version>.bak` backups of one database are kept.
pub(crate) const KEPT_UPGRADE_BACKUPS: usize = 3;

/// After `written` was completed, deletes this database's older
/// `<database>.pre-<version>.bak` files so that `written` and the newest
/// others (by version) make KEPT_UPGRADE_BACKUPS. `written` is never deleted;
/// other files are left alone. A failed deletion is logged, not fatal.
pub(crate) fn rotate_upgrade_backups(written: &std::path::Path) {
    let (Some(directory), Some(written_name)) = (
        written.parent(),
        written.file_name().and_then(|name| name.to_str()),
    ) else {
        return;
    };
    let Some(prefix) = written_name
        .rsplit_once(".pre-")
        .map(|(database, _)| format!("{database}.pre-"))
    else {
        return;
    };
    let directory = if directory.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        directory
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut others: Vec<(u64, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix(&prefix)?.strip_suffix(".bak")?;
            if name == written_name
                || version.len() < 4
                || !version.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            Some((version.parse().ok()?, entry.path()))
        })
        .collect();
    others.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in others.into_iter().skip(KEPT_UPGRADE_BACKUPS - 1) {
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(backup = %path.display(), "removed an older pre-upgrade backup")
            }
            Err(error) => {
                tracing::warn!(backup = %path.display(), %error, "could not remove an older pre-upgrade backup")
            }
        }
    }
}
