//! The single API error envelope: every failure the service returns is
//! `{ "error": { "code": <stable slug>, "message": <human text> } }`, so a client can
//! branch on `code` and show `message`. A conformance [`Rejection`] surfaces its own
//! stable code unchanged (e.g. `import`, `memory_cap_exceeded`), so "why was my Facet
//! refused" is machine-readable end to end.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use mosaic_certify::Rejection;
use serde_json::json;

/// A request-handling failure, rendered as the shared error envelope.
pub enum ApiError {
    /// Malformed request (bad base64, missing field, unusable value) — 400.
    BadRequest(String),
    /// The Facet was refused by the conformance gate — 422, carrying the rejection's
    /// stable code and message.
    Rejected(Rejection),
    /// A certified/admitted Facet trapped or errored while rendering (e.g. it exhausted its
    /// fuel or accessed out of bounds on this input) — 422. Distinct from `Rejected`, which
    /// is a static refusal; this is a runtime failure on a specific input.
    RenderFailed(String),
    /// The requested resource does not exist — 404.
    NotFound(String),
    /// An internal failure (e.g. a worker task panicked) — 500. The message is generic;
    /// details are logged, not leaked.
    Internal(String),
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError::BadRequest(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError::NotFound(message.into())
    }

    pub fn render_failed(message: impl Into<String>) -> Self {
        ApiError::RenderFailed(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        ApiError::Internal(message.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request".to_string(), m),
            ApiError::Rejected(r) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                r.code.as_str().to_string(),
                r.message,
            ),
            ApiError::RenderFailed(m) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "render_failed".to_string(),
                m,
            ),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found".to_string(), m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, "internal".to_string(), m),
        };
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}
