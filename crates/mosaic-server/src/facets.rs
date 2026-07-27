//! The registry endpoints: publish, list, fetch metadata, fetch bytes.
//!
//! - `POST /v1/facets` **[author]** — certify the submitted module (the same gate the
//!   browser mirrors) and, on pass, store it `Certified` awaiting moderation.
//! - `GET /v1/facets` — public listing of `Published` Facets, newest first.
//! - `GET /v1/facets/{id}` — a Facet's metadata + certificate.
//! - `GET /v1/facets/{id}/wasm` — the module bytes.
//!
//! Visibility: a `Published` Facet is public; a not-yet-published one is visible only to its
//! author or a moderator, and to everyone else it is a 404 (not a 403) — the registry does
//! not reveal that an unpublished Facet exists.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use mosaic_certify::{CertifyOutcome, certify};
use mosaic_registry::{FacetRecord, FacetState, ListFilter, NewFacet, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{AuthedPrincipal, OptionalPrincipal, Principal, Role};
use crate::certify::decode_wasm;
use crate::error::ApiError;
use crate::{AppState, MAX_NAME_LEN};

/// Request body for `POST /v1/facets`.
#[derive(Deserialize)]
pub struct PublishRequest {
    /// Display name.
    name: String,
    /// The Facet module, base64.
    wasm: String,
}

/// Success body for a publish: the stored record.
#[derive(Serialize)]
struct FacetEnvelope {
    facet: FacetRecord,
}

/// `POST /v1/facets` — publish a Facet (author only). Certifies, then stores `Certified`.
pub async fn publish(
    State(state): State<AppState>,
    AuthedPrincipal(principal): AuthedPrincipal,
    Json(req): Json<PublishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    principal.require(Role::Author)?;

    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::bad_request(format!(
            "name must be at most {MAX_NAME_LEN} characters"
        )));
    }
    let bytes = decode_wasm(&req.wasm)?;

    // Certify on a blocking worker (compiles + runs wasm), handing the bytes back so they can
    // be stored without decoding twice.
    let sandbox = state.sandbox.clone();
    let (outcome, bytes) = tokio::task::spawn_blocking(move || {
        let outcome = certify(&sandbox, &bytes);
        (outcome, bytes)
    })
    .await
    .map_err(|e| ApiError::internal(format!("certify worker failed: {e}")))?;

    let certificate = match outcome {
        CertifyOutcome::Certified(cert) => cert,
        CertifyOutcome::Rejected(rejection) => return Err(ApiError::Rejected(rejection)),
    };

    let abi_kind = certificate.abi_kind;
    let wasm_sha256 = certificate.wasm_sha256.clone();
    let new = NewFacet {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        author: principal.id,
        abi_kind,
        wasm_sha256,
        state: FacetState::Certified,
        created_at: now_unix(),
        certificate,
        wasm: bytes,
    };

    let registry = state.registry.clone();
    let record = tokio::task::spawn_blocking(move || registry.insert(new))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(store_error)?;

    Ok((StatusCode::CREATED, Json(FacetEnvelope { facet: record })))
}

/// `GET /v1/facets` — public listing of published Facets, newest first.
pub async fn list(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let registry = state.registry.clone();
    let facets = tokio::task::spawn_blocking(move || {
        registry.list(&ListFilter {
            state: Some(FacetState::Published),
        })
    })
    .await
    .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
    .map_err(store_error)?;
    Ok(Json(json!({ "facets": facets })))
}

/// `GET /v1/facets/{id}` — a Facet's metadata + certificate, subject to visibility.
pub async fn get_facet(
    State(state): State<AppState>,
    OptionalPrincipal(principal): OptionalPrincipal,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let record = load(&state, id).await?;
    ensure_visible(&record, principal.as_ref())?;
    Ok(Json(FacetEnvelope { facet: record }))
}

/// `GET /v1/facets/{id}/wasm` — the module bytes, subject to visibility.
pub async fn get_wasm(
    State(state): State<AppState>,
    OptionalPrincipal(principal): OptionalPrincipal,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let record = load(&state, id.clone()).await?;
    ensure_visible(&record, principal.as_ref())?;

    let registry = state.registry.clone();
    let bytes = tokio::task::spawn_blocking(move || registry.get_wasm(&id))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(store_error)?
        .ok_or_else(|| ApiError::not_found("no such facet"))?;

    Ok(([(header::CONTENT_TYPE, "application/wasm")], bytes).into_response())
}

/// Load a record by id (on a blocking worker), 404 if absent.
async fn load(state: &AppState, id: String) -> Result<FacetRecord, ApiError> {
    let registry = state.registry.clone();
    tokio::task::spawn_blocking(move || registry.get(&id))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(store_error)?
        .ok_or_else(|| ApiError::not_found("no such facet"))
}

/// Enforce visibility: a `Published` Facet is public; otherwise only its author or a
/// moderator may see it, and to anyone else it is a 404 (never revealing its existence).
fn ensure_visible(record: &FacetRecord, principal: Option<&Principal>) -> Result<(), ApiError> {
    if record.state == FacetState::Published {
        return Ok(());
    }
    let allowed = principal.is_some_and(|p| p.has_role(Role::Moderator) || p.id == record.author);
    if allowed {
        Ok(())
    } else {
        Err(ApiError::not_found("no such facet"))
    }
}

/// Current unix time in seconds (monotonic enough for a created-at stamp; 0 before the epoch).
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A registry backend failure is a 500 — it is the server's fault, not the caller's.
fn store_error(e: StoreError) -> ApiError {
    ApiError::internal(format!("registry backend error: {e}"))
}
