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
use mosaic_server::{AppState, AuthConfig, TokenEntry, app};
use serde_json::Value;
use tower::ServiceExt;

const FACET_RAMP: &[u8] = include_bytes!("../../tessera-ascii/tests/facet_ramp.wasm");

/// A test app with two principals: `author-token` (alice, author) and `mod-token`
/// (max, author + moderator).
fn test_app() -> axum::Router {
    let auth = AuthConfig::from_entries(vec![
        TokenEntry {
            token: "author-token".to_string(),
            id: "alice".to_string(),
            roles: vec!["author".to_string()],
        },
        TokenEntry {
            token: "mod-token".to_string(),
            id: "max".to_string(),
            roles: vec!["author".to_string(), "moderator".to_string()],
        },
    ])
    .unwrap();
    app(AppState {
        sandbox: Arc::new(Sandbox::new().expect("sandbox")),
        auth: Arc::new(auth),
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

#[tokio::test]
async fn render_ascii_produces_a_text_grid() {
    let (w, h) = (8u32, 8u32);
    let rgba = vec![200u8; (w * h * 4) as usize]; // a solid bright image
    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "inline": STANDARD.encode(FACET_RAMP) },
        "input": { "rgba": STANDARD.encode(&rgba), "width": w, "height": h },
        "params": { "cols": 8, "cellAspect": 1.0 }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = json_body(resp).await;
    assert_eq!(out["cols"], 8);
    let rows = out["rows"].as_u64().unwrap() as usize;
    let text = out["text"].as_str().unwrap();
    assert!(!text.is_empty());
    // compose joins `rows` rows of `cols` chars with '\n' between them.
    assert_eq!(text.lines().count(), rows);
}

#[tokio::test]
async fn render_spectral_produces_a_text_grid() {
    // A 1024-sample ramp as little-endian f32 bytes.
    let mut pcm = Vec::with_capacity(1024 * 4);
    for i in 0..1024u32 {
        pcm.extend_from_slice(&(i as f32 / 1024.0).to_le_bytes());
    }
    let body = serde_json::json!({
        "engine": "spectral",
        "facet": { "inline": STANDARD.encode(FACET_RAMP) },
        "input": { "pcm": STANDARD.encode(&pcm), "sampleRate": 8000 },
        "params": { "bands": 16, "win": 256, "hop": 128, "fmin": 50.0, "fmax": 4000.0 }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = json_body(resp).await;
    assert_eq!(out["rows"], 16); // one row per band
    assert!(out["text"].as_str().unwrap().len() >= 16);
}

#[tokio::test]
async fn render_rejects_a_non_conformant_facet_with_422() {
    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "inline": STANDARD.encode(b"not a wasm module") },
        "input": { "rgba": STANDARD.encode(vec![0u8; 64]), "width": 4, "height": 4 },
        "params": { "cols": 4 }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(resp).await["error"]["code"], "malformed");
}

#[tokio::test]
async fn render_rejects_a_mismatched_image_size_with_400() {
    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "inline": STANDARD.encode(FACET_RAMP) },
        "input": { "rgba": STANDARD.encode(vec![0u8; 10]), "width": 4, "height": 4 }, // needs 64 bytes
        "params": { "cols": 4 }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(resp).await["error"]["code"], "bad_request");
}

fn get_with_token(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn whoami_requires_a_token() {
    let resp = test_app()
        .oneshot(
            Request::builder()
                .uri("/v1/whoami")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(resp).await["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn whoami_returns_identity_for_a_valid_token() {
    let resp = test_app()
        .oneshot(get_with_token("/v1/whoami", "author-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["id"], "alice");
    assert_eq!(body["roles"], serde_json::json!(["author"]));
}

#[tokio::test]
async fn whoami_rejects_an_unknown_token() {
    let resp = test_app()
        .oneshot(get_with_token("/v1/whoami", "not-a-real-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
