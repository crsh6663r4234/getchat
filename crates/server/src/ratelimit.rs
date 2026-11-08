//! Distributed admission primitives. Errors are failures, never permission to
//! bypass a limit. Redis durability/eviction policy remains an operator duty.
use std::{fmt, sync::Arc, time::Duration};

use redis::{
    aio::MultiplexedConnection, cluster::ClusterClient, cluster_async::ClusterConnection,
    FromRedisValue, Script, ScriptInvocation,
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};

const MAX_EVENTS: u32 = 10_000;
const MAX_SUBJECTS: u32 = 4096;
const MAX_WINDOW_MS: u64 = 86_400_000;

// Both keys share one scope hash tag. No dynamic keys are constructed in Lua.
// A scope registry caps active subjects; each event log is capped by the limit.
// TIME prevents clients selecting an arbitrary clock. Clamp backwards time to
// the last admitted event for this subject; this does not solve Redis rollback.
const WINDOW_SCRIPT: &str = r#"
local window = tonumber(ARGV[1])
local limit = tonumber(ARGV[2])
local cap = tonumber(ARGV[3])
local nowparts = redis.call('TIME')
local now = tonumber(nowparts[1]) * 1000 + math.floor(tonumber(nowparts[2]) / 1000)
local newest = redis.call('ZRANGE', KEYS[1], -1, -1, 'WITHSCORES')
if #newest > 0 then now = math.max(now, tonumber(newest[2])) end
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now-window)
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', now)
local count = redis.call('ZCARD', KEYS[1])
if count >= limit then
  local oldest = redis.call('ZRANGE', KEYS[1], 0, 0, 'WITHSCORES')
  return {0, 0, math.max(1, tonumber(oldest[2])+window-now)}
end
if not redis.call('ZSCORE', KEYS[2], ARGV[4]) and redis.call('ZCARD', KEYS[2]) >= cap then
  return {2, 0, window}
end
redis.call('ZADD', KEYS[1], now, ARGV[5])
redis.call('PEXPIRE', KEYS[1], window)
redis.call('ZADD', KEYS[2], now+window, ARGV[4])
redis.call('PEXPIRE', KEYS[2], window*2)
return {1, limit-count-1, 0}
"#;

// Exact replay log with one hash-tagged key: at most MAX_EVENTS live request
// IDs per trusted session. No eviction fallback silently accepts replays.
const REPLAY_SCRIPT: &str = r#"
local ttl = tonumber(ARGV[1])
local cap = tonumber(ARGV[2])
local t = redis.call('TIME')
local now = tonumber(t[1])*1000 + math.floor(tonumber(t[2])/1000)
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
if redis.call('ZSCORE', KEYS[1], ARGV[3]) then return 0 end
if redis.call('ZCARD', KEYS[1]) >= cap then return 2 end
redis.call('ZADD', KEYS[1], now+ttl, ARGV[3])
local oldttl = redis.call('PTTL', KEYS[1])
redis.call('PEXPIRE', KEYS[1], math.max(oldttl, ttl*2))
return 1
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    Configuration,
    InvalidPolicy,
    Unavailable,
    TimedOut,
    Busy,
    Capacity,
    Replay,
}
impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configuration => "Redis admission configuration is invalid",
            Self::InvalidPolicy => "Admission policy or identity is invalid",
            Self::Unavailable => "Redis admission service is unavailable",
            Self::TimedOut => "Redis admission service timed out",
            Self::Busy => "Admission verification capacity is exhausted",
            Self::Capacity => "Admission state capacity is exhausted",
            Self::Replay => "Request proof was already used",
        })
    }
}
impl std::error::Error for LimitError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub remaining: u32,
    pub retry_after: Duration,
}

#[derive(Clone)]
pub struct RateLimiter {
    client: RedisClient,
    namespace: Arc<str>,
    timeout: Duration,
    connection: Arc<Mutex<Option<RedisConnection>>>,
    permits: Arc<Semaphore>,
}

#[derive(Clone)]
enum RedisClient {
    Single(redis::Client),
    Cluster(ClusterClient),
}
#[derive(Clone)]
enum RedisConnection {
    Single(MultiplexedConnection),
    Cluster(ClusterConnection),
}
impl RedisConnection {
    async fn invoke<T: FromRedisValue>(
        &mut self,
        invocation: &ScriptInvocation<'_>,
    ) -> Result<T, LimitError> {
        match self {
            Self::Single(connection) => invocation.invoke_async(connection).await,
            Self::Cluster(connection) => invocation.invoke_async(connection).await,
        }
        .map_err(|_| LimitError::Unavailable)
    }
}

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn bounded_ms(value: Duration, maximum: u64) -> Result<u64, LimitError> {
    let ms = u64::try_from(value.as_millis()).map_err(|_| LimitError::InvalidPolicy)?;
    if ms == 0 || ms > maximum {
        Err(LimitError::InvalidPolicy)
    } else {
        Ok(ms)
    }
}

impl RateLimiter {
    /// None means explicitly unconfigured, NOT secure admission. The caller
    /// must select fail-closed policy or label a separate development mode.
    pub fn from_env() -> Result<Option<Self>, LimitError> {
        let cluster = match std::env::var("GHOSTCHAT_REDIS_CLUSTER_URLS") {
            Ok(value) if !value.trim().is_empty() => Some(value),
            Ok(_) | Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err(LimitError::Configuration),
        };
        let url = match std::env::var("GHOSTCHAT_REDIS_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) | Err(std::env::VarError::NotPresent) if cluster.is_some() => String::new(),
            Ok(_) | Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(_) => return Err(LimitError::Configuration),
        };
        let namespace =
            std::env::var("GHOSTCHAT_REDIS_NAMESPACE").unwrap_or_else(|_| "ghostchat-v1".into());
        let timeout = std::env::var("GHOSTCHAT_REDIS_TIMEOUT_MS")
            .unwrap_or_else(|_| "1500".into())
            .parse::<u64>()
            .map_err(|_| LimitError::Configuration)?;
        if let Some(nodes) = cluster {
            if !url.is_empty() {
                return Err(LimitError::Configuration);
            }
            let urls: Vec<_> = nodes.split(',').map(str::trim).collect();
            Self::new_cluster(&urls, &namespace, Duration::from_millis(timeout)).map(Some)
        } else {
            Self::new(&url, &namespace, Duration::from_millis(timeout)).map(Some)
        }
    }

    pub fn new(url: &str, namespace: &str, timeout: Duration) -> Result<Self, LimitError> {
        if !label(namespace) || !(50..=10_000).contains(&timeout.as_millis()) || url.len() > 2048 {
            return Err(LimitError::Configuration);
        }
        // TLS support is selected by Cargo features; unsupported redis schemes
        // fail configuration/connection rather than falling back to plaintext.
        let client = redis::Client::open(url).map_err(|_| LimitError::Configuration)?;
        Ok(Self {
            client: RedisClient::Single(client),
            namespace: namespace.into(),
            timeout,
            connection: Arc::new(Mutex::new(None)),
            permits: Arc::new(Semaphore::new(32)),
        })
    }

    pub fn new_cluster(
        urls: &[&str],
        namespace: &str,
        timeout: Duration,
    ) -> Result<Self, LimitError> {
        if !label(namespace)
            || !(50..=10_000).contains(&timeout.as_millis())
            || urls.is_empty()
            || urls.len() > 16
            || urls.iter().any(|url| url.is_empty() || url.len() > 2048)
        {
            return Err(LimitError::Configuration);
        }
        let client = ClusterClient::new(urls.to_vec()).map_err(|_| LimitError::Configuration)?;
        Ok(Self {
            client: RedisClient::Cluster(client),
            namespace: namespace.into(),
            timeout,
            connection: Arc::new(Mutex::new(None)),
            permits: Arc::new(Semaphore::new(32)),
        })
    }

    async fn connection(&self) -> Result<RedisConnection, LimitError> {
        let mut connection = self.connection.lock().await;
        if let Some(value) = &*connection {
            return Ok(value.clone());
        }
        let value = match &self.client {
            RedisClient::Single(client) => RedisConnection::Single(
                client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(|_| LimitError::Unavailable)?,
            ),
            RedisClient::Cluster(client) => RedisConnection::Cluster(
                client
                    .get_async_connection()
                    .await
                    .map_err(|_| LimitError::Unavailable)?,
            ),
        };
        *connection = Some(value.clone());
        Ok(value)
    }

    fn keys(&self, scope: &str, subject: &str) -> Result<(String, String, String), LimitError> {
        if !label(scope) || subject.is_empty() || subject.len() > 512 {
            return Err(LimitError::InvalidPolicy);
        }
        let tag = digest(&format!("{}:{scope}", self.namespace));
        let subject = digest(subject);
        Ok((
            format!("{}:{{{tag}}}:events:{subject}", self.namespace),
            format!("{}:{{{tag}}}:subjects", self.namespace),
            subject,
        ))
    }

    /// Exact log over (now-window, now], counting accepted requests. Policy
    /// scope names and per-scope limit/window must be deployment constants.
    pub async fn check(
        &self,
        scope: &str,
        subject: &str,
        limit: u32,
        window: Duration,
    ) -> Result<Decision, LimitError> {
        if limit == 0 || limit > MAX_EVENTS {
            return Err(LimitError::InvalidPolicy);
        }
        let window = bounded_ms(window, MAX_WINDOW_MS)?;
        let (key, registry, subject) = self.keys(scope, subject)?;
        let _permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| LimitError::Busy)?;
        let result = tokio::time::timeout(self.timeout, async {
            let mut connection = self.connection().await?;
            let script = Script::new(WINDOW_SCRIPT);
            let mut invocation = script.prepare_invoke();
            invocation
                .key(key)
                .key(registry)
                .arg(window)
                .arg(limit)
                .arg(MAX_SUBJECTS)
                .arg(subject)
                .arg(uuid::Uuid::new_v4().to_string());
            let (accepted, remaining, retry): (i64, i64, i64) =
                connection.invoke(&invocation).await?;
            match accepted {
                0 | 1 if remaining >= 0 && retry >= 0 => Ok(Decision {
                    allowed: accepted == 1,
                    remaining: remaining as u32,
                    retry_after: Duration::from_millis(retry as u64),
                }),
                2 => Err(LimitError::Capacity),
                _ => Err(LimitError::Unavailable),
            }
        })
        .await
        .map_err(|_| LimitError::TimedOut)?;
        if matches!(result, Err(LimitError::Unavailable)) {
            self.connection.lock().await.take();
        }
        result
    }

    /// Call ONLY after verifying a signature against a trusted device/session
    /// binding and applying a per-session admission limit. No enrollment here.
    pub async fn claim_replay(
        &self,
        trusted_session: &str,
        request_id: &str,
        ttl: Duration,
    ) -> Result<(), LimitError> {
        if trusted_session.is_empty() || trusted_session.len() > 1024 {
            return Err(LimitError::InvalidPolicy);
        }
        let request_id = uuid::Uuid::parse_str(request_id)
            .map_err(|_| LimitError::InvalidPolicy)?
            .to_string();
        let ttl = bounded_ms(ttl, 300_000)?;
        let key = format!("{}:replay:{{{}}}", self.namespace, digest(trusted_session));
        let _permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| LimitError::Busy)?;
        let result = tokio::time::timeout(self.timeout, async {
            let mut connection = self.connection().await?;
            let script = Script::new(REPLAY_SCRIPT);
            let mut invocation = script.prepare_invoke();
            invocation
                .key(key)
                .arg(ttl)
                .arg(MAX_EVENTS)
                .arg(&request_id);
            let value: i64 = connection.invoke(&invocation).await?;
            match value {
                1 => Ok(()),
                0 => Err(LimitError::Replay),
                2 => Err(LimitError::Capacity),
                _ => Err(LimitError::Unavailable),
            }
        })
        .await
        .map_err(|_| LimitError::TimedOut)?;
        if matches!(result, Err(LimitError::Unavailable)) {
            self.connection.lock().await.take();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_keys_share_a_cluster_slot_without_exposing_subject() {
        let limiter =
            RateLimiter::new("redis://127.0.0.1/", "test", Duration::from_secs(1)).unwrap();
        let (a, b, subject) = limiter.keys("send", "private-user-id").unwrap();
        assert_eq!(
            a.split('{').nth(1).unwrap().split('}').next(),
            b.split('{').nth(1).unwrap().split('}').next()
        );
        assert!(!a.contains("private-user-id"));
        assert_eq!(subject.len(), 64);
        assert!(limiter.keys("attacker:{slot}", "x").is_err());
        assert!(limiter.keys("send", &"x".repeat(513)).is_err());
        assert!(bounded_ms(Duration::ZERO, MAX_WINDOW_MS).is_err());
    }
    #[tokio::test]
    async fn unavailable_redis_never_grants_permission() {
        // Reserve then release a local port; no Redis service is started.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let limiter = RateLimiter::new(
            &format!("redis://127.0.0.1:{port}/"),
            "test",
            Duration::from_millis(100),
        )
        .unwrap();
        assert!(limiter
            .check("send", "user", 2, Duration::from_secs(1))
            .await
            .is_err());
    }
    /// Real Redis required. Uses a unique namespace and short-lived keys only.
    #[tokio::test]
    #[ignore = "requires GHOSTCHAT_TEST_REDIS_URL pointing to an isolated Redis"]
    async fn redis_exact_window_concurrent_workers_and_replay() {
        let url = std::env::var("GHOSTCHAT_TEST_REDIS_URL").expect("isolated test Redis URL");
        let ns = format!("test-{}", uuid::Uuid::new_v4().simple());
        let a = RateLimiter::new(&url, &ns, Duration::from_secs(3)).unwrap();
        let b = RateLimiter::new(&url, &ns, Duration::from_secs(3)).unwrap();
        let mut tasks = Vec::new();
        for index in 0..20 {
            let l = if index % 2 == 0 { a.clone() } else { b.clone() };
            tasks.push(tokio::spawn(async move {
                l.check("send", "one-user", 5, Duration::from_secs(10))
                    .await
                    .unwrap()
                    .allowed
            }));
        }
        let mut allowed = 0;
        for task in tasks {
            if task.await.unwrap() {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 5);
        assert!(
            a.check("expiry", "one-user", 1, Duration::from_millis(100))
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !b.check("expiry", "one-user", 1, Duration::from_millis(100))
                .await
                .unwrap()
                .allowed
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            b.check("expiry", "one-user", 1, Duration::from_millis(100))
                .await
                .unwrap()
                .allowed
        );
        let request = uuid::Uuid::new_v4().to_string();
        a.claim_replay("trusted-session", &request, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            b.claim_replay("trusted-session", &request, Duration::from_secs(1))
                .await,
            Err(LimitError::Replay)
        );
    }
}
