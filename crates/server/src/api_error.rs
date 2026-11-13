//! The JSON error type every handler returns.

use super::*;

#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: &'static str,
    /// A stable identifier for refusals a client must tell apart from other
    /// errors with the same status (sent as `code` next to `error`).
    pub(crate) code: Option<&'static str>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: &'static str) -> Self {
        Self {
            status,
            message,
            code: None,
        }
    }
    pub(crate) fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }
    pub(crate) fn bad(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    pub(crate) fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Invalid or expired session.")
    }
    pub(crate) fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "You do not have access to this resource.",
        )
    }
    pub(crate) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "The requested item was not found.")
    }
    pub(crate) fn conflict(message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }
    pub(crate) fn too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Files must be 5 GB or smaller.",
        )
    }
    pub(crate) fn crypto_payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Cryptographic payloads must be 64 KB or smaller.",
        )
    }
    pub(crate) fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The server could not complete this request.",
        )
    }
    pub(crate) fn unavailable(message: &'static str) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({"error": self.message});
        if let Some(code) = self.code {
            body["code"] = code.into();
        }
        (self.status, Json(body)).into_response()
    }
}
