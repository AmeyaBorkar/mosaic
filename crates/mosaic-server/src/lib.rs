//! # mosaic-server
//!
//! Mosaic's authoritative HTTP service. It is the "truth" side of the platform: it
//! **certifies** untrusted Facets (the same admission the browser mirrors) and **renders**
//! them natively through the proven sandbox, so a shared or exported artifact is defined by
//! the server, not by whatever the client happened to run.
//!
//! The crate is split so the router is testable without a socket: [`app`] builds the
//! [`axum::Router`] from an [`AppState`], and the binary ([`main`](../main.rs)) binds it.
//! Integration tests drive it in-process via `tower::ServiceExt::oneshot`.
//!
//! CPU-bound work (compiling and running Facet wasm) runs on `tokio::task::spawn_blocking`,
//! never on the async executor; the [`Sandbox`](mosaic_runtime::Sandbox) is shared behind an
//! `Arc` and hands every execution its own fresh, zero-capability store.

#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use mosaic_runtime::Sandbox;

mod certify;
pub mod error;
mod health;
mod render;

/// Maximum request body. An 8 MiB Facet base64-encodes to ~10.9 MiB, and raw-RGBA render
/// inputs can be larger; 32 MiB is generous while still bounding per-request memory. Axum's
/// default is 2 MiB — far too small for a wasm module — so this is raised deliberately.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Shared, cheaply-cloneable application state. Holds the long-lived authoritative sandbox;
/// later phases add the registry store.
#[derive(Clone)]
pub struct AppState {
    /// The authoritative Facet host, shared across requests.
    pub sandbox: Arc<Sandbox>,
}

/// Build the application router from `state`. Separated from serving so tests can drive it
/// with `oneshot` and no network.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/v1/certify", post(certify::certify_handler))
        .route("/v1/render", post(render::render_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}
