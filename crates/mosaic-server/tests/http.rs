//! In-process HTTP tests: drive the router with `oneshot`, no sockets. Covers liveness and
//! the certify endpoint's success (200 + certificate) and refusal (422 + error envelope)
//! branches, plus a malformed-input 400.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mosaic_runtime::Sandbox;
use mosaic_server::{AppState, app};
use serde_json::Value;
use tower::ServiceExt;

const FACET_RAMP: &[u8] = include_bytes!("../../tessera-ascii/tests/facet_ramp.wasm");

fn test_app() -> axum::Router {
    app(AppState {
        sandbox: Arc::new(Sandbox::new().expect("sandbox")),
    })
}

async fn json_body(resp: Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn healthz_reports_ok() {
    let resp = test_app()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "ok");
}

#[tokio::test]
async fn certify_returns_certificate_for_a_valid_facet() {
    let req = post_json(
        "/v1/certify",
        serde_json::json!({ "wasm": STANDARD.encode(FACET_RAMP) }),
    );
    let resp = test_app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["certificate"]["abiKind"], "gather");
    assert_eq!(body["certificate"]["certifyVersion"], 1);
    assert!(body["certificate"]["wasmSha256"].as_str().unwrap().len() == 64);
}

#[tokio::test]
async fn certify_rejects_non_wasm_with_422() {
    // Well-formed base64, but not a wasm module: the gate rejects it as malformed.
    let req = post_json(
        "/v1/certify",
        serde_json::json!({ "wasm": STANDARD.encode(b"not a wasm module") }),
    );
    let resp = test_app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert_eq!(body["error"]["code"], "malformed");
}

#[tokio::test]
async fn certify_rejects_bad_base64_with_400() {
    let req = post_json(
        "/v1/certify",
        serde_json::json!({ "wasm": "!!! not base64 !!!" }),
    );
    let resp = test_app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(resp).await["error"]["code"], "bad_request");
}
