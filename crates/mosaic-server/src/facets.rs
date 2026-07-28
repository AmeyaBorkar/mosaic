//! The registry endpoints: publish, list, fetch metadata, fetch bytes.
//!
//! - `POST /v1/facets` **[author]** — certify the submission (the same gate the browser
//!   mirrors) and, on pass, store it `Certified` awaiting moderation. A submission is a wasm
//!   module (`wasm`) or a DSL program (`program` + `engine`).
//! - `GET /v1/facets` — public listing of `Published` Facets, newest first.
//! - `GET /v1/facets/{id}` — a Facet's metadata + certificate.
//! - `GET /v1/facets/{id}/wasm` · `GET /v1/facets/{id}/program` — the stored bytes, one
//!   endpoint per kind (a wasm module, or DSL bytecode).
//!
//! Visibility: a `Published` Facet is public; a not-yet-published one is visible only to its
//! author or a moderator, and to everyone else it is a 404 (not a 403) — the registry does
//! not reveal that an unpublished Facet exists.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use mosaic_certify::{
    CertifyOutcome, ProgramCertifyOutcome, Rejection, RejectionCode, certify, certify_program,
};
use mosaic_registry::{
    ArtifactKind, FacetArtifact, FacetRecord, FacetState, ListFilter, NewFacet, StoreError,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{AuthedPrincipal, OptionalPrincipal, Principal, Role};
use crate::certify::decode_wasm;
use crate::error::ApiError;
use crate::render::engine_stride;
use crate::{AppState, MAX_NAME_LEN};

/// Request body for `POST /v1/facets`. A submission is either a self-contained wasm module
/// (`wasm`) or a DSL bytecode program (`program`, with the `engine` it targets) — exactly one.
#[derive(Deserialize)]
pub struct PublishRequest {
    /// Display name.
    name: String,
    /// A self-contained wasm Facet module, base64. Mutually exclusive with `program`.
    #[serde(default)]
    wasm: Option<String>,
    /// A DSL bytecode program, base64. Mutually exclusive with `wasm`; requires `engine`.
    #[serde(default)]
    program: Option<String>,
    /// The feature engine a `program` targets (`ascii`, `ascii-structural`, `spectral`).
    #[serde(default)]
    engine: Option<String>,
}

/// Success body for a publish: the stored record.
#[derive(Serialize)]
struct FacetEnvelope {
    facet: FacetRecord,
}

/// `POST /v1/facets` — publish a Facet (author only). Certifies, then stores `Certified`.
/// Accepts a wasm module (`wasm`) or a DSL program (`program` + `engine`) — the same gate,
/// two artifact kinds.
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

    let (artifact, bytes) = match (req.wasm, req.program) {
        (Some(_), Some(_)) => {
            return Err(ApiError::bad_request(
                "provide exactly one of `wasm` or `program`, not both",
            ));
        }
        (None, None) => {
            return Err(ApiError::bad_request(
                "a Facet submission needs `wasm` (a module) or `program` (DSL bytecode)",
            ));
        }
        (Some(wasm_b64), None) => certify_wasm_submission(&state, &wasm_b64).await?,
        (None, Some(program_b64)) => {
            let engine = req
                .engine
                .ok_or_else(|| ApiError::bad_request("a `program` submission needs an `engine`"))?;
            certify_program_submission(&state, &engine, &program_b64).await?
        }
    };

    let new = NewFacet {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        author: principal.id,
        state: FacetState::Certified,
        created_at: now_unix(),
        artifact,
        bytes,
    };

    let registry = state.registry.clone();
    let record = tokio::task::spawn_blocking(move || registry.insert(new))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(store_error)?;

    Ok((StatusCode::CREATED, Json(FacetEnvelope { facet: record })))
}

/// Certify a wasm-module submission, returning its artifact metadata and the module bytes.
async fn certify_wasm_submission(
    state: &AppState,
    wasm_b64: &str,
) -> Result<(FacetArtifact, Vec<u8>), ApiError> {
    let bytes = decode_wasm(wasm_b64)?;
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
    let artifact = FacetArtifact::Wasm {
        abi_kind: certificate.abi_kind,
        wasm_sha256: certificate.wasm_sha256.clone(),
        certificate,
    };
    Ok((artifact, bytes))
}

/// Certify a DSL-program submission for `engine`, returning its artifact metadata and the
/// bytecode. Rejects an unknown engine, and a program whose declared stride does not match
/// the engine it is published for.
async fn certify_program_submission(
    state: &AppState,
    engine: &str,
    program_b64: &str,
) -> Result<(FacetArtifact, Vec<u8>), ApiError> {
    let engine = engine.to_string();
    let want_stride = engine_stride(&engine).ok_or_else(|| {
        ApiError::Rejected(Rejection {
            code: RejectionCode::UnknownEngine,
            message: format!("unknown engine {engine:?}"),
        })
    })?;
    let program = decode_program(program_b64)?;

    // Certify on a blocking worker (runs the program through the sandboxed interpreter).
    let sandbox = state.sandbox.clone();
    let interp = state.interp.clone();
    let (outcome, program) = tokio::task::spawn_blocking(move || {
        let outcome = certify_program(&sandbox, &interp, &program);
        (outcome, program)
    })
    .await
    .map_err(|e| ApiError::internal(format!("certify worker failed: {e}")))?;

    let certificate = match outcome {
        ProgramCertifyOutcome::Certified(cert) => cert,
        ProgramCertifyOutcome::Rejected(rejection) => return Err(ApiError::Rejected(rejection)),
    };
    if certificate.stride != want_stride {
        return Err(ApiError::Rejected(Rejection {
            code: RejectionCode::ProgramStrideMismatch,
            message: format!(
                "program declares stride {} but the {engine:?} engine has stride {want_stride}",
                certificate.stride
            ),
        }));
    }
    let artifact = FacetArtifact::Program {
        engine,
        stride: certificate.stride,
        program_sha256: certificate.program_sha256.clone(),
        certificate,
    };
    Ok((artifact, program))
}

/// Query for `GET /v1/facets`.
#[derive(Deserialize)]
pub struct ListQuery {
    /// Restrict to a moderation state. Omitted (or `published`) is the public view; any
    /// other state is the moderator queue and requires the moderator role.
    state: Option<String>,
}

/// `GET /v1/facets` — list Facets, newest first. Public callers see `Published`; a moderator
/// may pass `?state=certified` (or `rejected`) to review the queue.
pub async fn list(
    State(state): State<AppState>,
    OptionalPrincipal(principal): OptionalPrincipal,
    Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let want = match query.state.as_deref() {
        None | Some("published") => FacetState::Published,
        Some(other) => {
            let parsed = FacetState::parse(other)
                .ok_or_else(|| ApiError::bad_request(format!("unknown state {other:?}")))?;
            // Anything but the public Published view requires a moderator.
            let is_moderator = principal
                .as_ref()
                .is_some_and(|p| p.has_role(Role::Moderator));
            if !is_moderator {
                return Err(ApiError::forbidden(
                    "listing non-published Facets requires the moderator role",
                ));
            }
            parsed
        }
    };

    let registry = state.registry.clone();
    let facets =
        tokio::task::spawn_blocking(move || registry.list(&ListFilter { state: Some(want) }))
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

/// `GET /v1/facets/{id}/wasm` — a **wasm** Facet's module bytes, subject to visibility. A
/// program Facet has no module here; that is a 404 (fetch its bytecode from `/program`).
pub async fn get_wasm(
    State(state): State<AppState>,
    OptionalPrincipal(principal): OptionalPrincipal,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    serve_bytes(
        &state,
        principal,
        id,
        ArtifactKind::Wasm,
        "application/wasm",
    )
    .await
}

/// `GET /v1/facets/{id}/program` — a **program** Facet's DSL bytecode, subject to visibility.
/// A wasm Facet has no bytecode here; that is a 404 (fetch its module from `/wasm`).
pub async fn get_program(
    State(state): State<AppState>,
    OptionalPrincipal(principal): OptionalPrincipal,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    serve_bytes(
        &state,
        principal,
        id,
        ArtifactKind::Program,
        "application/octet-stream",
    )
    .await
}

/// Serve a Facet's stored bytes if it is of `want` kind and visible to the caller. A visible
/// Facet of the other kind is a 404 (it has no bytes at this endpoint), keeping the two byte
/// endpoints cleanly kind-specific.
async fn serve_bytes(
    state: &AppState,
    principal: Option<Principal>,
    id: String,
    want: ArtifactKind,
    content_type: &'static str,
) -> Result<Response, ApiError> {
    let record = load(state, id.clone()).await?;
    ensure_visible(&record, principal.as_ref())?;
    if record.artifact.kind() != want {
        return Err(ApiError::not_found("no such facet"));
    }

    let registry = state.registry.clone();
    let bytes = tokio::task::spawn_blocking(move || registry.get_bytes(&id))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(store_error)?
        .ok_or_else(|| ApiError::not_found("no such facet"))?;

    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

/// Request body for `POST /v1/facets/{id}/moderate`.
#[derive(Deserialize)]
pub struct ModerateRequest {
    decision: ModerateDecision,
}

/// A moderator's decision.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModerateDecision {
    Publish,
    Reject,
}

/// `POST /v1/facets/{id}/moderate` — approve or reject a certified Facet (moderator only).
/// The only valid transition is `Certified -> Published | Rejected`; moderating a Facet in
/// any other state is a 409, and an unknown id is a 404.
pub async fn moderate(
    State(state): State<AppState>,
    AuthedPrincipal(principal): AuthedPrincipal,
    Path(id): Path<String>,
    Json(req): Json<ModerateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    principal.require(Role::Moderator)?;

    let mut record = load(&state, id.clone()).await?;
    if record.state != FacetState::Certified {
        return Err(ApiError::conflict(format!(
            "Facet is '{}'; only a certified Facet awaiting moderation can be moderated",
            record.state.as_str()
        )));
    }
    let new_state = match req.decision {
        ModerateDecision::Publish => FacetState::Published,
        ModerateDecision::Reject => FacetState::Rejected,
    };

    let registry = state.registry.clone();
    let id_for_set = id.clone();
    let found = tokio::task::spawn_blocking(move || registry.set_state(&id_for_set, new_state))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(store_error)?;
    if !found {
        // Raced with a delete between load and set — treat as gone.
        return Err(ApiError::not_found("no such facet"));
    }
    record.state = new_state;
    Ok(Json(FacetEnvelope { facet: record }))
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

/// Decode the base64 DSL program bytes, mapping a decode failure to a 400.
fn decode_program(b64: &str) -> Result<Vec<u8>, ApiError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("`program` is not valid base64: {e}")))
}

/// A registry backend failure is a 500 — it is the server's fault, not the caller's.
fn store_error(e: StoreError) -> ApiError {
    ApiError::internal(format!("registry backend error: {e}"))
}
