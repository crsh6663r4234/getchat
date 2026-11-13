//! Request admission: the distributed (Redis) limiter middleware and the in-process rate limiter.

use super::*;

pub(crate) async fn distributed_admission(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    match &state.distributed_limiter {
        Err(_) => {
            return ApiError::unavailable("Distributed admission is misconfigured.").into_response()
        }
        Ok(None) if state.require_redis => {
            return ApiError::unavailable("Distributed admission is required but unavailable.")
                .into_response()
        }
        Ok(None) => {} // Compatibility mode, explicitly not distributed security.
        Ok(Some(limiter)) => {
            let subject = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .filter(|v| v.len() <= 256)
                .map(session_hash)
                .unwrap_or_else(|| "anonymous".into());
            for (scope, identity, count, window) in [
                ("http-global", "global", 200, Duration::from_secs(1)),
                (
                    "http-session",
                    subject.as_str(),
                    600,
                    Duration::from_secs(60),
                ),
            ] {
                match limiter.check(scope, identity, count, window).await {
                    Ok(decision) if decision.allowed => {}
                    Ok(_) => {
                        return ApiError::new(
                            StatusCode::TOO_MANY_REQUESTS,
                            "Please slow down and try again.",
                        )
                        .into_response()
                    }
                    Err(_) => {
                        return ApiError::unavailable(
                            "Distributed admission could not be verified.",
                        )
                        .into_response()
                    }
                }
            }
        }
    }
    next.run(request).await
}

pub(crate) fn allow_rate(state: &AppState, key: &str, count: usize, period: Duration) -> bool {
    let now = Instant::now();
    let mut limiter = match state.limiter.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    };
    let entries = limiter.entry(key.to_owned()).or_default();
    while entries
        .front()
        .is_some_and(|instant| now.duration_since(*instant) > period)
    {
        entries.pop_front();
    }
    if entries.len() >= count {
        return false;
    }
    entries.push_back(now);
    true
}
