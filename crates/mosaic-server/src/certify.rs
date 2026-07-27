//! `POST /v1/certify` — the authoritative gate as an endpoint. Decodes a base64 Facet,
//! runs [`mosaic_certify::certify`] on a blocking worker (it compiles and executes wasm),
//! and returns the certificate (200) or the rejection (422). This is the same admission a
//! publish runs; exposing it lets an author check a Facet before submitting.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mosaic_certify::{Certificate, CertifyOutcome, certify};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;

/// Request body for `POST /v1/certify`.
#[derive(Deserialize)]
pub struct CertifyRequest {
    /// The Facet module, base64 (standard alphabet).
    pub wasm: String,
}

/// Success body: the Facet's certificate.
#[derive(Serialize)]
struct CertifyResponse {
    certificate: Certificate,
}

pub async fn certify_handler(
    State(state): State<AppState>,
    Json(req): Json<CertifyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let bytes = decode_wasm(&req.wasm)?;
    // Compiling and executing wasm is CPU-bound and blocking; keep it off the async
    // executor. The sandbox is shared (Arc) and each execution gets a fresh store.
    let sandbox = state.sandbox.clone();
    let outcome = tokio::task::spawn_blocking(move || certify(&sandbox, &bytes))
        .await
        .map_err(|e| ApiError::internal(format!("certify worker failed: {e}")))?;

    match outcome {
        CertifyOutcome::Certified(certificate) => {
            Ok((StatusCode::OK, Json(CertifyResponse { certificate })))
        }
        CertifyOutcome::Rejected(rejection) => Err(ApiError::Rejected(rejection)),
    }
}

/// Decode the base64 Facet bytes, mapping a decode failure to a 400.
pub(crate) fn decode_wasm(b64: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("`wasm` is not valid base64: {e}")))
}
