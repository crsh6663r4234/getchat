//! Self-update of the standalone Windows server exe (docs/UPDATES.md).
//!
//! Only the server binary opts in (`main.rs`, Windows, not disabled with
//! `GHOSTCHAT_AUTO_UPDATE=false`); the desktop's built-in loopback server
//! calls `serve` and never starts it. The network, the files next to the exe
//! and the process swap sit behind small traits so the decision logic is
//! tested with fakes.

use std::{
    collections::HashSet,
    ffi::OsString,
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};

/// The updater public key of `crates/desktop/tauri.conf.json`
/// (`plugins.updater.pubkey`): base64 of a minisign public key file.
pub const UPDATE_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEYxMjY3RDk0QkQyMjNFMDAKUldRQVBpSzlsSDBtOGI5V3VCOFNTd1J6c1NuSU9PRUJVaFM4bVFXWC9IK29GQktXQmFUR05RQTEK";
/// The GitHub repository whose releases carry the server exe.
pub const RELEASE_REPOSITORY: &str = "crsh6663r4234/getchat";
/// The release asset name of the standalone Windows server.
pub const SERVER_ASSET: &str = "ghostchat-server-windows-x64.exe";
/// How often a running server looks for a newer release.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether the standalone server updates itself: only on Windows, and not
/// when `GHOSTCHAT_AUTO_UPDATE` is `false`, `0`, `no` or `off`.
pub fn enabled(setting: Option<&str>, windows: bool) -> bool {
    let disabled = setting.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        )
    });
    windows && !disabled
}

fn parse_version(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let mut parsed = [0; 3];
    for slot in &mut parsed {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(parsed)
}

/// Whether `candidate` is a strictly higher `major.minor.patch` than
/// `current`. Anything that does not parse is never newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Verifies a Tauri updater `.sig` file (base64 of a minisign signature)
/// over `data` with `public_key` (base64 of a minisign public key file).
pub fn verify_signature(
    data: &[u8],
    signature_file: &[u8],
    public_key: &str,
) -> Result<(), String> {
    fn base64_text(encoded: &[u8], what: &str) -> Result<String, String> {
        let encoded = std::str::from_utf8(encoded)
            .map_err(|_| format!("the {what} is not text"))?
            .trim();
        let raw = STANDARD
            .decode(encoded)
            .map_err(|_| format!("the {what} is not base64"))?;
        String::from_utf8(raw).map_err(|_| format!("the {what} is not a minisign file"))
    }
    let key =
        minisign_verify::PublicKey::decode(&base64_text(public_key.as_bytes(), "public key")?)
            .map_err(|e| format!("the public key is invalid: {e}"))?;
    let signature = minisign_verify::Signature::decode(&base64_text(signature_file, "signature")?)
        .map_err(|e| format!("the signature is invalid: {e}"))?;
    // `true` also accepts legacy (non-prehashed) signatures, exactly as the
    // desktop's Tauri updater verifies the MSI with the same key.
    key.verify(data, &signature, true)
        .map_err(|e| format!("the signature does not match: {e}"))
}

/// Where the latest release is read from and its assets downloaded.
pub trait Releases: Send + Sync {
    fn latest_version(&self) -> impl Future<Output = Result<String, String>> + Send;
    fn download(
        &self,
        version: &str,
        asset: &str,
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
}

/// The files next to the running exe.
pub trait Files: Send + Sync {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String>;
    fn read(&self, path: &Path) -> Result<Vec<u8>, String>;
    /// Replaces `to` if it exists (Windows allows renaming a running exe).
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String>;
    /// Succeeds when `path` does not exist.
    fn remove(&self, path: &Path) -> Result<(), String>;
}

/// Stopping this server, starting the new exe and judging it.
pub trait Swap: Send + Sync {
    /// Returns once no call is relayed through this process (its rooms
    /// would drop with the old process), or after [`CALL_DEFER_LIMIT`].
    fn wait_for_no_calls(&self) -> impl Future<Output = ()> + Send;
    fn backup_database(&self) -> impl Future<Output = Result<(), String>> + Send;
    /// Stops accepting connections and releases the listening port.
    fn stop_serving(&self) -> impl Future<Output = ()> + Send;
    /// Listens again after a failed swap.
    fn resume_serving(&self) -> impl Future<Output = ()> + Send;
    /// Starts `exe` with this process's arguments, environment and directory.
    fn start(&self, exe: &Path) -> Result<(), String>;
    /// Whether the started process answers `/health` with `version` in time.
    fn healthy(&self, version: &str) -> impl Future<Output = bool> + Send;
    /// Stops the started process after a failed health check.
    fn stop_started(&self) -> impl Future<Output = ()> + Send;
    /// The highest migration recorded in the database.
    fn schema_version(&self) -> impl Future<Output = Result<i64, String>> + Send;
}

/// The files of one swap, all next to the running exe.
#[derive(Debug, Clone)]
pub struct SwapPaths {
    pub exe: PathBuf,
    pub old: PathBuf,
    pub new: PathBuf,
    pub new_signature: PathBuf,
}

impl SwapPaths {
    pub fn for_exe(exe: &Path) -> Self {
        let sibling = |suffix: &str| {
            let mut name = OsString::from(exe.as_os_str());
            name.push(suffix);
            PathBuf::from(name)
        };
        Self {
            exe: exe.to_path_buf(),
            old: sibling(".old"),
            new: sibling(".new"),
            new_signature: sibling(".new.sig"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The latest release is not newer (or already failed in this process).
    UpToDate(String),
    /// Nothing was changed: the release could not be read or verified, or
    /// the database could not be backed up.
    Rejected(String),
    /// The swap failed and the old exe is back in place and serving.
    RolledBack { version: String, reason: String },
    /// The new exe failed after it migrated the database, which the old exe
    /// cannot open again. The new exe stays in place (the old one is kept as
    /// `.old`), this server does not listen again, and the process exits so
    /// that the new exe can be started again (by a service manager or by
    /// hand) or the old one restored together with the database backup.
    KeptNew { version: String, reason: String },
    /// The new exe runs and answered its health check; this process exits.
    Installed(String),
}

/// One update attempt. `skip` holds versions that already rolled back in
/// this process, so a broken release is not retried every day.
pub async fn try_update<R: Releases, F: Files, S: Swap>(
    current: &str,
    public_key: &str,
    paths: &SwapPaths,
    skip: &HashSet<String>,
    releases: &R,
    files: &F,
    swap: &S,
) -> Outcome {
    let latest = match releases.latest_version().await {
        Ok(version) => version,
        Err(reason) => {
            tracing::warn!(%reason, "self-update: could not read the latest release");
            return Outcome::Rejected(format!("could not read the latest release: {reason}"));
        }
    };
    if !is_newer(&latest, current) {
        tracing::info!(%latest, %current, "self-update: the server is up to date");
        return Outcome::UpToDate(latest);
    }
    if skip.contains(&latest) {
        tracing::info!(%latest, "self-update: skipping a release that already failed here");
        return Outcome::UpToDate(latest);
    }
    tracing::info!(%latest, %current, "self-update: a newer server release is available");
    let discard_download = || {
        let _ = files.remove(&paths.new);
        let _ = files.remove(&paths.new_signature);
    };
    if let Err(reason) = stage(&latest, public_key, paths, releases, files).await {
        discard_download();
        tracing::warn!(%latest, %reason, "self-update: the release was rejected");
        return Outcome::Rejected(reason);
    }
    tracing::info!(%latest, "self-update: the downloaded exe has a valid signature");
    swap.wait_for_no_calls().await;
    if let Err(reason) = swap.backup_database().await {
        discard_download();
        tracing::warn!(%latest, %reason, "self-update: no database backup, not updating");
        return Outcome::Rejected(format!("could not back up the database: {reason}"));
    }
    tracing::info!("self-update: stopping this server to swap the exe");
    swap.stop_serving().await;
    // Read with nothing serving, so only the new exe can change it.
    let schema_before = match swap.schema_version().await {
        Ok(version) => version,
        Err(reason) => {
            discard_download();
            swap.resume_serving().await;
            tracing::error!(%latest, %reason, "self-update: could not read the database schema; not updating");
            return Outcome::RolledBack {
                version: latest,
                reason: format!("could not read the database schema: {reason}"),
            };
        }
    };
    let rolled_back = |reason: String| {
        tracing::error!(%latest, %reason, "self-update: rolled back to the running version");
        Outcome::RolledBack {
            version: latest.clone(),
            reason,
        }
    };
    if let Err(reason) = files
        .remove(&paths.old)
        .and_then(|()| files.rename(&paths.exe, &paths.old))
    {
        discard_download();
        swap.resume_serving().await;
        return rolled_back(reason);
    }
    if let Err(reason) = files.rename(&paths.new, &paths.exe) {
        log_restore(&files.rename(&paths.old, &paths.exe));
        discard_download();
        swap.resume_serving().await;
        return rolled_back(reason);
    }
    let _ = files.remove(&paths.new_signature);
    tracing::info!(exe = %paths.exe.display(), "self-update: starting the new server");
    let reason = match swap.start(&paths.exe) {
        Ok(()) if swap.healthy(&latest).await => {
            tracing::info!(%latest, "self-update: the new server is healthy; this one exits");
            return Outcome::Installed(latest);
        }
        Ok(()) => {
            swap.stop_started().await;
            "the new server did not pass its health check in time".to_owned()
        }
        Err(reason) => format!("could not start the new server: {reason}"),
    };
    let schema_after = swap.schema_version().await;
    if schema_after
        .as_ref()
        .map_or(true, |after| *after != schema_before)
    {
        tracing::error!(
            %latest,
            %reason,
            schema_before,
            schema_after = ?schema_after,
            exe = %paths.exe.display(),
            old = %paths.old.display(),
            "self-update: the new server failed after it migrated the database, which the old \
             server cannot open; keeping the new exe. Start it again, or restore the old exe \
             (the .old file) together with the database backup <database>.pre-update.bak"
        );
        return Outcome::KeptNew {
            version: latest,
            reason,
        };
    }
    log_restore(&files.rename(&paths.old, &paths.exe));
    swap.resume_serving().await;
    rolled_back(reason)
}

fn log_restore(restored: &Result<(), String>) {
    match restored {
        Ok(()) => tracing::info!("self-update: the old exe is back in place"),
        Err(reason) => tracing::error!(
            %reason,
            "self-update: could not put the old exe back; it is still running as the .old file"
        ),
    }
}

/// Downloads the exe and its signature next to the running exe and
/// verifies the bytes on disk, which are the bytes that will run.
async fn stage<R: Releases, F: Files>(
    version: &str,
    public_key: &str,
    paths: &SwapPaths,
    releases: &R,
    files: &F,
) -> Result<(), String> {
    let exe = releases.download(version, SERVER_ASSET).await?;
    let signature = releases
        .download(version, &format!("{SERVER_ASSET}.sig"))
        .await?;
    files.write(&paths.new, &exe)?;
    files.write(&paths.new_signature, &signature)?;
    verify_signature(
        &files.read(&paths.new)?,
        &files.read(&paths.new_signature)?,
        public_key,
    )
}

/// The real files next to the exe.
pub struct DiskFiles;

impl Files for DiskFiles {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        std::fs::write(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
    }
    fn read(&self, path: &Path) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|e| {
            format!(
                "could not rename {} to {}: {e}",
                from.display(),
                to.display()
            )
        })
    }
    fn remove(&self, path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove {}: {error}", path.display())),
        }
    }
}

/// Removes what an earlier update left next to `exe`: an unused download
/// and the previous `.old` exe. On Windows the `.old` exe stays locked while
/// the previous process waits for this one's health check, so its removal
/// is retried in the background for a while.
pub fn remove_leftovers(exe: &Path) {
    let paths = SwapPaths::for_exe(exe);
    let _ = DiskFiles.remove(&paths.new);
    let _ = DiskFiles.remove(&paths.new_signature);
    if !paths.old.exists() {
        return;
    }
    tokio::spawn(async move {
        for _ in 0..120 {
            match std::fs::remove_file(&paths.old) {
                Ok(()) => {
                    tracing::info!(old = %paths.old.display(), "self-update: removed the previous server exe");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
            }
        }
        tracing::warn!(old = %paths.old.display(), "self-update: could not remove the previous server exe");
    });
}

/// The largest exe or signature the updater downloads.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// The GitHub release feed: `latest.json` of the latest release (written by
/// the release workflow) names the version; assets are read from its tag.
pub struct GithubReleases {
    client: reqwest::Client,
}

impl GithubReleases {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("ghostchat-server/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(15 * 60))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client })
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_DOWNLOAD_BYTES)
        {
            return Err(format!("{url}: the file is too large"));
        }
        Ok(response)
    }
}

impl Releases for GithubReleases {
    async fn latest_version(&self) -> Result<String, String> {
        let url =
            format!("https://github.com/{RELEASE_REPOSITORY}/releases/latest/download/latest.json");
        let feed: serde_json::Value = self
            .get(&url)
            .await?
            .json()
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        feed["version"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{url}: no version"))
    }

    async fn download(&self, version: &str, asset: &str) -> Result<Vec<u8>, String> {
        if parse_version(version).is_none() {
            return Err(format!("invalid release version {version:?}"));
        }
        let url =
            format!("https://github.com/{RELEASE_REPOSITORY}/releases/download/v{version}/{asset}");
        let bytes = self
            .get(&url)
            .await?
            .bytes()
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
            return Err(format!("{url}: the file is too large"));
        }
        tracing::info!(%url, bytes = bytes.len(), "self-update: downloaded");
        Ok(bytes.to_vec())
    }
}

/// Lets the updater stop and restart the standalone server's listener.
#[derive(Clone)]
pub struct ServeControl {
    wanted: std::sync::Arc<tokio::sync::watch::Sender<bool>>,
    listening: tokio::sync::watch::Receiver<bool>,
}

/// The serving loop's side of a [`ServeControl`].
pub struct ServeSignals {
    /// Whether the server should be listening.
    pub wanted: tokio::sync::watch::Receiver<bool>,
    /// Set by the serving loop once it listens / has released the port.
    pub listening: tokio::sync::watch::Sender<bool>,
}

impl ServeControl {
    pub fn new() -> (Self, ServeSignals) {
        let (wanted_tx, wanted_rx) = tokio::sync::watch::channel(true);
        let (listening_tx, listening_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                wanted: std::sync::Arc::new(wanted_tx),
                listening: listening_rx,
            },
            ServeSignals {
                wanted: wanted_rx,
                listening: listening_tx,
            },
        )
    }

    /// Waits until the server listens.
    pub async fn listening(&self) {
        let _ = self.listening.clone().wait_for(|on| *on).await;
    }

    pub(crate) async fn stop(&self) {
        self.wanted.send_replace(false);
        let _ = self.listening.clone().wait_for(|on| !*on).await;
    }

    pub(crate) async fn resume(&self) {
        self.wanted.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(60), self.listening()).await;
    }
}

/// The longest a swap waits for relayed calls to end.
pub const CALL_DEFER_LIMIT: Duration = Duration::from_secs(30 * 60);
/// How often a waiting swap looks again.
const CALL_DEFER_POLL: Duration = Duration::from_secs(15);

/// Waits while `active` says a call room is open, looking every `poll`, for
/// at most `limit`. Returns whether the calls ended (false: gave up).
pub async fn wait_for_no_calls(active: impl Fn() -> bool, limit: Duration, poll: Duration) -> bool {
    if !active() {
        return true;
    }
    tracing::info!(
        limit_minutes = limit.as_secs() / 60,
        "self-update: a call is relayed through this server; waiting for it to end"
    );
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("self-update: calls are still active; updating anyway");
            return false;
        }
        tokio::time::sleep(poll.min(deadline - tokio::time::Instant::now())).await;
        if !active() {
            tracing::info!("self-update: no call is active any more; updating");
            return true;
        }
    }
}

/// Whether a call room of this process's media relay is open.
pub type CallsActive = std::sync::Arc<dyn Fn() -> bool + Send + Sync>;

/// How long a started server has to answer `/health` with its version
/// (it may run database migrations first).
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

/// The real swap: database backup, the serving loop, and a child process.
pub struct ProcessSwap {
    db: sqlx::SqlitePool,
    database_path: String,
    health_url: String,
    control: ServeControl,
    calls_active: CallsActive,
    client: reqwest::Client,
    child: std::sync::Mutex<Option<std::process::Child>>,
}

impl ProcessSwap {
    pub fn new(
        db: sqlx::SqlitePool,
        database_path: String,
        address: std::net::SocketAddr,
        control: ServeControl,
        calls_active: CallsActive,
    ) -> Self {
        let host = match address.ip() {
            std::net::IpAddr::V4(ip) if ip.is_unspecified() => {
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            }
            std::net::IpAddr::V6(ip) if ip.is_unspecified() => {
                std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
            }
            ip => ip,
        };
        Self {
            db,
            database_path,
            health_url: format!(
                "http://{}/health",
                std::net::SocketAddr::new(host, address.port())
            ),
            control,
            calls_active,
            client: reqwest::Client::new(),
            child: std::sync::Mutex::new(None),
        }
    }

    fn started_has_exited(&self) -> bool {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        match child.as_mut().map(|child| child.try_wait()) {
            Some(Ok(Some(status))) => {
                tracing::warn!(%status, "self-update: the new server exited");
                true
            }
            _ => false,
        }
    }

    async fn reports(&self, version: &str) -> bool {
        let Ok(response) = self
            .client
            .get(&self.health_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        else {
            return false;
        };
        let Ok(health) = response.json::<serde_json::Value>().await else {
            return false;
        };
        health["status"] == "ok" && health["version"] == version
    }
}

impl Swap for ProcessSwap {
    async fn wait_for_no_calls(&self) {
        let active = self.calls_active.clone();
        wait_for_no_calls(move || active(), CALL_DEFER_LIMIT, CALL_DEFER_POLL).await;
    }

    async fn backup_database(&self) -> Result<(), String> {
        crate::backup_before_self_update(&self.db, &self.database_path)
            .await
            .map(|_| ())
    }

    async fn stop_serving(&self) {
        self.control.stop().await;
    }

    async fn resume_serving(&self) {
        self.control.resume().await;
    }

    fn start(&self, exe: &Path) -> Result<(), String> {
        // Same arguments; environment and working directory are inherited.
        let child = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .spawn()
            .map_err(|e| e.to_string())?;
        tracing::info!(pid = child.id(), "self-update: started the new server");
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        Ok(())
    }

    async fn healthy(&self, version: &str) -> bool {
        let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if self.started_has_exited() {
                return false;
            }
            if self.reports(version).await {
                return true;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        false
    }

    async fn schema_version(&self) -> Result<i64, String> {
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&self.db)
            .await
            .map_err(|e| e.to_string())
    }

    async fn stop_started(&self) {
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = child.kill();
                child.wait()
            })
            .await;
            tracing::info!("self-update: stopped the new server");
        }
    }
}

/// Starts the self-updater of the standalone server: removes leftovers of
/// an earlier update, then checks once the server listens and every
/// [`CHECK_INTERVAL`]. A successful update exits this process.
pub fn start(
    db: sqlx::SqlitePool,
    database_path: String,
    address: std::net::SocketAddr,
    control: ServeControl,
    calls_active: CallsActive,
) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::warn!(%error, "self-update: the server exe path is unknown; not updating");
            return;
        }
    };
    remove_leftovers(&exe);
    let releases = match GithubReleases::new() {
        Ok(releases) => releases,
        Err(error) => {
            tracing::warn!(%error, "self-update: no HTTP client; not updating");
            return;
        }
    };
    tracing::info!(
        exe = %exe.display(),
        "self-update: on (set GHOSTCHAT_AUTO_UPDATE=false to turn it off)"
    );
    let swap = ProcessSwap::new(db, database_path, address, control.clone(), calls_active);
    tokio::spawn(async move {
        control.listening().await;
        let paths = SwapPaths::for_exe(&exe);
        let mut failed = HashSet::new();
        loop {
            let outcome = try_update(
                crate::SERVER_VERSION,
                UPDATE_PUBLIC_KEY,
                &paths,
                &failed,
                &releases,
                &DiskFiles,
                &swap,
            )
            .await;
            match outcome {
                Outcome::Installed(version) => {
                    tracing::info!(%version, "self-update: handing over to the new server");
                    std::process::exit(0);
                }
                Outcome::RolledBack { version, .. } => {
                    failed.insert(version);
                }
                Outcome::KeptNew { version, .. } => {
                    // Exits non-zero, so a service manager restarts the exe,
                    // which is now the new version.
                    tracing::error!(%version, "self-update: exiting; the new server exe is in place");
                    std::process::exit(1);
                }
                Outcome::UpToDate(_) | Outcome::Rejected(_) => {}
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::{collections::HashMap, sync::Mutex};

    /// A fixed test build; its BLAKE2b-512 digest is precomputed because
    /// the Tauri signer makes prehashed ("ED") minisign signatures.
    const PAYLOAD: &[u8] = b"ghostchat-server 9.9.9 test build";
    const PAYLOAD_BLAKE2B: &str = "0905628d2db4a448052daff956dee038b189928e003a422d970a8bf509b4150a4aba0c1faced1df811179311929e5ab9565813625a7ef08786117b5199502642";
    const KEY_ID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn test_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    /// base64 of a minisign public key file, as in tauri.conf.json.
    fn public_key_of(key: &SigningKey) -> String {
        let mut raw = b"Ed".to_vec();
        raw.extend_from_slice(&KEY_ID);
        raw.extend_from_slice(key.verifying_key().as_bytes());
        STANDARD.encode(format!(
            "untrusted comment: minisign public key: test\n{}\n",
            STANDARD.encode(raw)
        ))
    }

    /// A Tauri `.sig` file over PAYLOAD: base64 of a prehashed minisign
    /// signature file.
    fn signature_of_payload(key: &SigningKey) -> Vec<u8> {
        let signature = key.sign(&hex(PAYLOAD_BLAKE2B)).to_bytes();
        let trusted = "timestamp:1793700000\tfile:ghostchat-server-windows-x64.exe";
        let mut global_input = signature.to_vec();
        global_input.extend_from_slice(trusted.as_bytes());
        let global = key.sign(&global_input).to_bytes();
        let mut line = b"ED".to_vec();
        line.extend_from_slice(&KEY_ID);
        line.extend_from_slice(&signature);
        STANDARD
            .encode(format!(
                "untrusted comment: signature from tauri secret key\n{}\ntrusted comment: {trusted}\n{}\n",
                STANDARD.encode(line),
                STANDARD.encode(global)
            ))
            .into_bytes()
    }

    struct FakeReleases {
        latest: String,
        assets: HashMap<String, Vec<u8>>,
        downloads: Mutex<Vec<String>>,
    }

    impl FakeReleases {
        fn new(latest: &str, exe: &[u8], signature: Vec<u8>) -> Self {
            Self {
                latest: latest.into(),
                assets: HashMap::from([
                    (SERVER_ASSET.to_owned(), exe.to_vec()),
                    (format!("{SERVER_ASSET}.sig"), signature),
                ]),
                downloads: Mutex::new(Vec::new()),
            }
        }
    }

    impl Releases for FakeReleases {
        async fn latest_version(&self) -> Result<String, String> {
            Ok(self.latest.clone())
        }
        async fn download(&self, version: &str, asset: &str) -> Result<Vec<u8>, String> {
            self.downloads
                .lock()
                .unwrap()
                .push(format!("{version}/{asset}"));
            self.assets.get(asset).cloned().ok_or_else(|| "404".into())
        }
    }

    #[derive(Default)]
    struct FakeSwap {
        unhealthy: bool,
        backup_fails: bool,
        /// The started exe applies a migration before it fails its check.
        migrates: bool,
        started: Mutex<bool>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeSwap {
        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Swap for FakeSwap {
        async fn wait_for_no_calls(&self) {
            self.record("wait_for_no_calls".into());
        }
        async fn backup_database(&self) -> Result<(), String> {
            self.record("backup".into());
            if self.backup_fails {
                Err("disk full".into())
            } else {
                Ok(())
            }
        }
        async fn stop_serving(&self) {
            self.record("stop_serving".into());
        }
        async fn resume_serving(&self) {
            self.record("resume_serving".into());
        }
        fn start(&self, exe: &Path) -> Result<(), String> {
            let running = std::fs::read(exe).unwrap();
            self.record(format!("start:{}", String::from_utf8_lossy(&running)));
            *self.started.lock().unwrap() = true;
            Ok(())
        }
        async fn schema_version(&self) -> Result<i64, String> {
            Ok(if self.migrates && *self.started.lock().unwrap() {
                33
            } else {
                32
            })
        }
        async fn healthy(&self, version: &str) -> bool {
            self.record(format!("healthy:{version}"));
            !self.unhealthy
        }
        async fn stop_started(&self) {
            self.record("stop_started".into());
        }
    }

    const OLD_BUILD: &[u8] = b"ghostchat-server 4.0.51 running build";

    /// A temp dir holding a running exe; returns its swap paths.
    fn running_exe() -> SwapPaths {
        let dir = std::env::temp_dir().join(format!("ghostchat-update-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(SERVER_ASSET);
        std::fs::write(&exe, OLD_BUILD).unwrap();
        SwapPaths::for_exe(&exe)
    }

    fn files_in(paths: &SwapPaths) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(paths.exe.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    async fn attempt(releases: &FakeReleases, swap: &FakeSwap, paths: &SwapPaths) -> Outcome {
        try_update(
            "4.0.51",
            &public_key_of(&test_key(9)),
            paths,
            &HashSet::new(),
            releases,
            &DiskFiles,
            swap,
        )
        .await
    }

    #[test]
    fn embedded_public_key_is_the_desktop_updater_key() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../desktop/tauri.conf.json")).unwrap();
        assert_eq!(
            config["plugins"]["updater"]["pubkey"].as_str(),
            Some(UPDATE_PUBLIC_KEY)
        );
    }

    #[test]
    fn signature_check_accepts_only_the_signed_bytes_and_key() {
        let key = test_key(9);
        let signature = signature_of_payload(&key);
        assert_eq!(
            verify_signature(PAYLOAD, &signature, &public_key_of(&key)),
            Ok(())
        );
        assert!(verify_signature(b"tampered build", &signature, &public_key_of(&key)).is_err());
        assert!(verify_signature(PAYLOAD, &signature, &public_key_of(&test_key(3))).is_err());
        assert!(verify_signature(PAYLOAD, b"not a signature", &public_key_of(&key)).is_err());
        assert!(verify_signature(PAYLOAD, &signature, UPDATE_PUBLIC_KEY).is_err());
    }

    #[test]
    fn only_strictly_higher_versions_are_newer() {
        assert!(is_newer("4.0.52", "4.0.51"));
        assert!(is_newer("v4.1.0", "4.0.99"));
        assert!(is_newer("4.0.10", "4.0.9"));
        assert!(is_newer("5.0.0", "4.9.9"));
        assert!(!is_newer("4.0.51", "4.0.51"));
        assert!(!is_newer("4.0.50", "4.0.51"));
        assert!(!is_newer("3.9.99", "4.0.0"));
        assert!(!is_newer("4.0.52-beta", "4.0.51"));
        assert!(!is_newer("4.0", "4.0.51"));
        assert!(!is_newer("", "4.0.51"));
    }

    #[test]
    fn self_update_runs_only_on_windows_unless_disabled() {
        assert!(enabled(None, true));
        assert!(enabled(Some("true"), true));
        assert!(enabled(Some(""), true));
        for off in ["false", "FALSE", " false ", "0", "no", "off"] {
            assert!(!enabled(Some(off), true), "{off:?} did not disable it");
        }
        assert!(!enabled(None, false));
        assert!(!enabled(Some("true"), false));
    }

    #[tokio::test]
    async fn bad_signature_is_rejected_and_nothing_changes() {
        let paths = running_exe();
        let releases = FakeReleases::new(
            "9.9.9",
            b"ghostchat-server 9.9.9 tampered build",
            signature_of_payload(&test_key(9)),
        );
        let swap = FakeSwap::default();
        let outcome = attempt(&releases, &swap, &paths).await;
        assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
        assert_eq!(std::fs::read(&paths.exe).unwrap(), OLD_BUILD);
        assert_eq!(files_in(&paths), vec![SERVER_ASSET.to_owned()]);
        assert!(swap.calls().is_empty(), "{:?}", swap.calls());
    }

    #[tokio::test]
    async fn older_or_equal_release_is_ignored_without_downloading() {
        for latest in ["4.0.51", "4.0.50", "garbage"] {
            let paths = running_exe();
            let releases = FakeReleases::new(latest, PAYLOAD, signature_of_payload(&test_key(9)));
            let swap = FakeSwap::default();
            let outcome = attempt(&releases, &swap, &paths).await;
            assert_eq!(outcome, Outcome::UpToDate(latest.into()));
            assert!(releases.downloads.lock().unwrap().is_empty());
            assert!(swap.calls().is_empty());
            assert_eq!(std::fs::read(&paths.exe).unwrap(), OLD_BUILD);
        }
    }

    #[tokio::test]
    async fn newer_verified_release_is_swapped_in_after_a_backup() {
        let paths = running_exe();
        let releases = FakeReleases::new("9.9.9", PAYLOAD, signature_of_payload(&test_key(9)));
        let swap = FakeSwap::default();
        let outcome = attempt(&releases, &swap, &paths).await;
        assert_eq!(outcome, Outcome::Installed("9.9.9".into()));
        assert_eq!(
            swap.calls(),
            vec![
                "wait_for_no_calls".to_owned(),
                "backup".into(),
                "stop_serving".into(),
                format!("start:{}", String::from_utf8_lossy(PAYLOAD)),
                "healthy:9.9.9".into(),
            ]
        );
        assert_eq!(std::fs::read(&paths.exe).unwrap(), PAYLOAD);
        assert_eq!(std::fs::read(&paths.old).unwrap(), OLD_BUILD);
        assert_eq!(
            files_in(&paths),
            vec![SERVER_ASSET.to_owned(), format!("{SERVER_ASSET}.old")]
        );
    }

    #[tokio::test]
    async fn failed_health_check_restores_the_old_exe_and_keeps_serving() {
        let paths = running_exe();
        let releases = FakeReleases::new("9.9.9", PAYLOAD, signature_of_payload(&test_key(9)));
        let swap = FakeSwap {
            unhealthy: true,
            ..FakeSwap::default()
        };
        let outcome = attempt(&releases, &swap, &paths).await;
        assert!(
            matches!(&outcome, Outcome::RolledBack { version, .. } if version == "9.9.9"),
            "{outcome:?}"
        );
        assert_eq!(
            swap.calls(),
            vec![
                "wait_for_no_calls".to_owned(),
                "backup".into(),
                "stop_serving".into(),
                format!("start:{}", String::from_utf8_lossy(PAYLOAD)),
                "healthy:9.9.9".into(),
                "stop_started".into(),
                "resume_serving".into(),
            ]
        );
        assert_eq!(std::fs::read(&paths.exe).unwrap(), OLD_BUILD);
        assert_eq!(files_in(&paths), vec![SERVER_ASSET.to_owned()]);
    }

    #[tokio::test]
    async fn the_swap_waits_for_relayed_calls_to_end_but_not_forever() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let polls = AtomicUsize::new(0);
        // Calls end after the third look: the wait returns then.
        let ended = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_no_calls(
                || polls.fetch_add(1, Ordering::SeqCst) < 3,
                Duration::from_secs(60),
                Duration::from_millis(5),
            ),
        )
        .await;
        assert_eq!(
            ended,
            Ok(true),
            "the swap goes ahead once no call is active"
        );
        assert_eq!(polls.load(Ordering::SeqCst), 4);
        // A call that never ends delays the swap only up to the limit.
        let started = std::time::Instant::now();
        let gave_up = wait_for_no_calls(
            || true,
            Duration::from_millis(100),
            Duration::from_millis(5),
        )
        .await;
        assert!(!gave_up);
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// The old exe refuses to start on a database a newer one migrated, so
    /// it must not be put back once the new exe applied a migration.
    #[tokio::test]
    async fn a_new_exe_that_migrated_the_database_is_kept_instead_of_rolled_back() {
        let paths = running_exe();
        let releases = FakeReleases::new("9.9.9", PAYLOAD, signature_of_payload(&test_key(9)));
        let swap = FakeSwap {
            unhealthy: true,
            migrates: true,
            ..FakeSwap::default()
        };
        let outcome = attempt(&releases, &swap, &paths).await;
        assert!(
            matches!(&outcome, Outcome::KeptNew { version, .. } if version == "9.9.9"),
            "{outcome:?}"
        );
        assert_eq!(
            swap.calls(),
            vec![
                "wait_for_no_calls".to_owned(),
                "backup".into(),
                "stop_serving".into(),
                format!("start:{}", String::from_utf8_lossy(PAYLOAD)),
                "healthy:9.9.9".into(),
                "stop_started".into(),
            ],
            "the old server must not serve the migrated database"
        );
        assert_eq!(std::fs::read(&paths.exe).unwrap(), PAYLOAD);
        assert_eq!(std::fs::read(&paths.old).unwrap(), OLD_BUILD);
    }

    #[tokio::test]
    async fn failed_backup_stops_the_update_before_the_swap() {
        let paths = running_exe();
        let releases = FakeReleases::new("9.9.9", PAYLOAD, signature_of_payload(&test_key(9)));
        let swap = FakeSwap {
            backup_fails: true,
            ..FakeSwap::default()
        };
        let outcome = attempt(&releases, &swap, &paths).await;
        assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
        assert_eq!(
            swap.calls(),
            vec!["wait_for_no_calls".to_owned(), "backup".into()],
            "the swap waits for calls to end before the backup"
        );
        assert_eq!(std::fs::read(&paths.exe).unwrap(), OLD_BUILD);
        assert_eq!(files_in(&paths), vec![SERVER_ASSET.to_owned()]);
    }

    #[tokio::test]
    async fn a_version_that_rolled_back_is_not_retried_in_this_process() {
        let paths = running_exe();
        let releases = FakeReleases::new("9.9.9", PAYLOAD, signature_of_payload(&test_key(9)));
        let swap = FakeSwap::default();
        let outcome = try_update(
            "4.0.51",
            &public_key_of(&test_key(9)),
            &paths,
            &HashSet::from(["9.9.9".to_owned()]),
            &releases,
            &DiskFiles,
            &swap,
        )
        .await;
        assert_eq!(outcome, Outcome::UpToDate("9.9.9".into()));
        assert!(releases.downloads.lock().unwrap().is_empty());
        assert!(swap.calls().is_empty());
    }
}
