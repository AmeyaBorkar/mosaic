//! In-process HTTP tests: drive the router with `oneshot`, no sockets. Covers liveness and
//! the certify endpoint's success (200 + certificate) and refusal (422 + error envelope)
//! branches, plus a malformed-input 400.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mosaic_registry::InMemoryStore;
use mosaic_runtime::Sandbox;
use mosaic_server::{AppState, AuthConfig, TokenEntry, app, compile_interp};
use serde_json::Value;
use tower::ServiceExt;

const FACET_RAMP: &[u8] = include_bytes!("../../tessera-ascii/tests/facet_ramp.wasm");

/// A test app with three principals: `author-token` (alice, author), `mod-token`
/// (max, author + moderator), and `reader-token` (bob, no roles).
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
        TokenEntry {
            token: "reader-token".to_string(),
            id: "bob".to_string(),
            roles: vec![],
        },
    ])
    .unwrap();
    let sandbox = Sandbox::new().expect("sandbox");
    let interp = Arc::new(compile_interp(&sandbox).expect("compile interp"));
    app(AppState {
        sandbox: Arc::new(sandbox),
        interp,
        auth: Arc::new(auth),
        registry: Arc::new(InMemoryStore::new()),
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

fn post_json_auth(uri: &str, body: Value, token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn publish_ramp(app: &axum::Router, token: &str, name: &str) -> Response {
    let body = serde_json::json!({ "name": name, "wasm": STANDARD.encode(FACET_RAMP) });
    app.clone()
        .oneshot(post_json_auth("/v1/facets", body, token))
        .await
        .unwrap()
}

#[tokio::test]
async fn publish_requires_the_author_role() {
    let app = test_app();
    // no token -> 401
    let body = serde_json::json!({ "name": "X", "wasm": STANDARD.encode(FACET_RAMP) });
    let resp = app
        .clone()
        .oneshot(post_json("/v1/facets", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // authenticated but no author role -> 403
    let resp = publish_ramp(&app, "reader-token", "X").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(resp).await["error"]["code"], "forbidden");
}

#[tokio::test]
async fn publish_certifies_and_stores_certified() {
    let app = test_app();
    let resp = publish_ramp(&app, "author-token", "Ramp Deluxe").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    assert_eq!(body["facet"]["name"], "Ramp Deluxe");
    assert_eq!(body["facet"]["author"], "alice");
    assert_eq!(body["facet"]["state"], "certified");
    assert_eq!(body["facet"]["artifact"]["kind"], "wasm");
    assert_eq!(body["facet"]["artifact"]["abiKind"], "gather");
    assert!(!body["facet"]["id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn publish_rejects_a_non_conformant_facet() {
    let app = test_app();
    let body = serde_json::json!({ "name": "Bad", "wasm": STANDARD.encode(b"not a wasm module") });
    let resp = app
        .clone()
        .oneshot(post_json_auth("/v1/facets", body, "author-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(resp).await["error"]["code"], "malformed");
}

#[tokio::test]
async fn certified_facet_is_visible_to_author_not_public() {
    let app = test_app();
    let created = json_body(publish_ramp(&app, "author-token", "Secret").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();

    // The author sees their own not-yet-published facet.
    let resp = app
        .clone()
        .oneshot(get_with_token(&format!("/v1/facets/{id}"), "author-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["facet"]["name"], "Secret");

    // Anonymous -> 404 (its existence is not revealed).
    let anon = Request::builder()
        .uri(format!("/v1/facets/{id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(anon).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // The public listing excludes it (Certified, not Published).
    let list = Request::builder()
        .uri("/v1/facets")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(list).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["facets"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn get_wasm_returns_the_module_bytes() {
    let app = test_app();
    let created = json_body(publish_ramp(&app, "author-token", "Bytes").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(get_with_token(
            &format!("/v1/facets/{id}/wasm"),
            "author-token",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/wasm"
    );
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.as_ref(), FACET_RAMP);
}

#[tokio::test]
async fn get_unknown_facet_is_404() {
    let app = test_app();
    let resp = app
        .clone()
        .oneshot(get_with_token("/v1/facets/does-not-exist", "author-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn moderate(app: &axum::Router, id: &str, decision: &str, token: &str) -> Response {
    let body = serde_json::json!({ "decision": decision });
    app.clone()
        .oneshot(post_json_auth(
            &format!("/v1/facets/{id}/moderate"),
            body,
            token,
        ))
        .await
        .unwrap()
}

#[tokio::test]
async fn moderator_publishes_a_certified_facet() {
    let app = test_app();
    let created = json_body(publish_ramp(&app, "author-token", "Approve Me").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();

    let resp = moderate(&app, &id, "publish", "mod-token").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["facet"]["state"], "published");

    // Now the public listing includes it.
    let list = Request::builder()
        .uri("/v1/facets")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(list).await.unwrap();
    let facets = json_body(resp).await;
    assert_eq!(facets["facets"].as_array().unwrap().len(), 1);
    assert_eq!(facets["facets"][0]["id"], id);

    // And an anonymous fetch by id now succeeds.
    let anon = Request::builder()
        .uri(format!("/v1/facets/{id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(anon).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn moderation_requires_the_moderator_role() {
    let app = test_app();
    let created = json_body(publish_ramp(&app, "author-token", "X").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();
    let resp = moderate(&app, &id, "publish", "author-token").await; // author is not a moderator
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn moderating_a_non_certified_facet_is_a_conflict() {
    let app = test_app();
    let created = json_body(publish_ramp(&app, "author-token", "X").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        moderate(&app, &id, "publish", "mod-token").await.status(),
        StatusCode::OK
    );
    // Already published: moderating again is a 409.
    let resp = moderate(&app, &id, "reject", "mod-token").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(resp).await["error"]["code"], "conflict");
}

#[tokio::test]
async fn moderating_an_unknown_facet_is_404() {
    let app = test_app();
    let resp = moderate(&app, "does-not-exist", "publish", "mod-token").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn moderator_queue_is_gated_and_reject_works() {
    let app = test_app();
    json_body(publish_ramp(&app, "author-token", "Queued").await).await;

    // A moderator sees the certified queue.
    let resp = app
        .clone()
        .oneshot(get_with_token("/v1/facets?state=certified", "mod-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["facets"].as_array().unwrap().len(), 1);

    // A non-moderator cannot use the queue.
    let resp = app
        .clone()
        .oneshot(get_with_token("/v1/facets?state=certified", "author-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Reject transitions to rejected.
    let created = json_body(publish_ramp(&app, "author-token", "Rejectee").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();
    let resp = moderate(&app, &id, "reject", "mod-token").await;
    assert_eq!(json_body(resp).await["facet"]["state"], "rejected");
}

#[tokio::test]
async fn render_halfblock_returns_colored_cells() {
    let (w, h) = (4u32, 4u32);
    let rgba = vec![128u8; (w * h * 4) as usize];
    let body = serde_json::json!({
        "engine": "halfblock",
        "input": { "rgba": STANDARD.encode(&rgba), "width": w, "height": h },
        "params": { "cols": 4, "cellAspect": 1.0 }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = json_body(resp).await;
    assert_eq!(out["glyph"], 0x2580); // ▀
    let n = (out["cols"].as_u64().unwrap() * out["rows"].as_u64().unwrap()) as usize;
    assert_eq!(out["fg"].as_array().unwrap().len(), n);
    assert_eq!(out["bg"].as_array().unwrap().len(), n);
    assert!(out["text"].is_null()); // no glyph text for the half-block engine
}

#[tokio::test]
async fn render_braille_returns_dot_text() {
    let (w, h) = (8u32, 8u32);
    // Solid white: every sub-cell is lit, so each cell is the full braille glyph ⣿ (U+28FF).
    let rgba = vec![255u8; (w * h * 4) as usize];
    let body = serde_json::json!({
        "engine": "braille",
        "input": { "rgba": STANDARD.encode(&rgba), "width": w, "height": h },
        "params": { "cols": 4, "cellAspect": 1.0 }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = json_body(resp).await;
    let text = out["text"].as_str().unwrap();
    assert!(text.chars().filter(|c| *c != '\n').all(|c| c == '\u{28FF}'));
    assert!(out["glyph"].is_null()); // braille is plain text, not a half-block glyph
    assert!(out["fg"].is_null());
    assert_eq!(text.lines().count() as u64, out["rows"].as_u64().unwrap());
}

#[tokio::test]
async fn render_ascii_with_color_adds_per_cell_colors() {
    let (w, h) = (8u32, 8u32);
    let rgba = vec![200u8; (w * h * 4) as usize];
    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "inline": STANDARD.encode(FACET_RAMP) },
        "input": { "rgba": STANDARD.encode(&rgba), "width": w, "height": h },
        "params": { "cols": 8, "cellAspect": 1.0, "color": true }
    });
    let resp = test_app()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = json_body(resp).await;
    assert!(out["text"].is_string());
    let n = (out["cols"].as_u64().unwrap() * out["rows"].as_u64().unwrap()) as usize;
    assert_eq!(out["colors"].as_array().unwrap().len(), n);
}

// --- DSL program Facets: publish, moderate, render by id, fetch bytecode. ---

const ASCII_SCHEMA: mosaic_dsl::Schema = mosaic_dsl::Schema {
    stride: 8,
    features: &[
        ("luma", 0),
        ("grad_mag", 1),
        ("grad_dir", 2),
        ("u", 3),
        ("v", 4),
        ("r", 5),
        ("g", 6),
        ("b", 7),
    ],
    params: &[("threshold", 0.6)],
};

/// A real authored DSL program: a density ramp over the ASCII (stride-8) vocabulary.
fn ascii_program() -> Vec<u8> {
    mosaic_dsl::compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap()
}

async fn publish_program(app: &axum::Router, token: &str, name: &str, engine: &str) -> Response {
    let body = serde_json::json!({
        "name": name,
        "engine": engine,
        "program": STANDARD.encode(ascii_program()),
    });
    app.clone()
        .oneshot(post_json_auth("/v1/facets", body, token))
        .await
        .unwrap()
}

#[tokio::test]
async fn publish_program_certifies_and_stores() {
    let app = test_app();
    let resp = publish_program(&app, "author-token", "DSL Ramp", "ascii").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    assert_eq!(body["facet"]["state"], "certified");
    assert_eq!(body["facet"]["artifact"]["kind"], "program");
    assert_eq!(body["facet"]["artifact"]["engine"], "ascii");
    assert_eq!(body["facet"]["artifact"]["stride"], 8);
    assert_eq!(body["facet"]["artifact"]["certificate"]["stride"], 8);
    assert!(
        body["facet"]["artifact"]["programSha256"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
}

#[tokio::test]
async fn publish_program_rejects_unknown_engine() {
    let app = test_app();
    let resp = publish_program(&app, "author-token", "Mystery", "bogus-engine").await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(resp).await["error"]["code"], "unknown_engine");
}

#[tokio::test]
async fn publish_program_rejects_stride_mismatch() {
    // A stride-8 (ascii) program published for the stride-1 spectral engine is refused.
    let app = test_app();
    let resp = publish_program(&app, "author-token", "Wrong Engine", "spectral").await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json_body(resp).await["error"]["code"],
        "program_stride_mismatch"
    );
}

#[tokio::test]
async fn publish_rejects_both_wasm_and_program() {
    let app = test_app();
    let body = serde_json::json!({
        "name": "Both",
        "wasm": STANDARD.encode(FACET_RAMP),
        "program": STANDARD.encode(ascii_program()),
        "engine": "ascii",
    });
    let resp = app
        .clone()
        .oneshot(post_json_auth("/v1/facets", body, "author-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(resp).await["error"]["code"], "bad_request");
}

#[tokio::test]
async fn render_by_id_runs_a_published_program() {
    let app = test_app();
    // Publish a DSL program, then a moderator approves it so it is renderable by id.
    let created = json_body(publish_program(&app, "author-token", "DSL Ramp", "ascii").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        moderate(&app, &id, "publish", "mod-token").await.status(),
        StatusCode::OK
    );

    let (w, h) = (8u32, 8u32);
    let rgba = vec![200u8; (w * h * 4) as usize];
    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "id": id },
        "input": { "rgba": STANDARD.encode(&rgba), "width": w, "height": h },
        "params": { "cols": 8, "cellAspect": 1.0 }
    });
    let resp = app
        .clone()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = json_body(resp).await;
    assert_eq!(out["cols"], 8);
    assert!(!out["text"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn render_by_id_refuses_unpublished_program() {
    let app = test_app();
    // Certified but not yet published: not renderable by id (a 404, not revealing it exists).
    let created = json_body(publish_program(&app, "author-token", "Draft", "ascii").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();

    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "id": id },
        "input": { "rgba": STANDARD.encode(vec![200u8; 8 * 8 * 4]), "width": 8, "height": 8 },
        "params": { "cols": 8 }
    });
    let resp = app
        .clone()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_program_returns_bytecode_and_wasm_endpoint_404s() {
    let app = test_app();
    let created = json_body(publish_program(&app, "author-token", "Bytes", "ascii").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();

    // The bytecode is served from /program (author sees their own certified facet).
    let resp = app
        .clone()
        .oneshot(get_with_token(
            &format!("/v1/facets/{id}/program"),
            "author-token",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/octet-stream"
    );
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.as_ref(), ascii_program().as_slice());

    // A program has no wasm module: /wasm is a 404 for it.
    let resp = app
        .clone()
        .oneshot(get_with_token(
            &format!("/v1/facets/{id}/wasm"),
            "author-token",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn render_by_id_runs_a_published_wasm_facet() {
    let app = test_app();
    let created = json_body(publish_ramp(&app, "author-token", "Ramp").await).await;
    let id = created["facet"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        moderate(&app, &id, "publish", "mod-token").await.status(),
        StatusCode::OK
    );

    let body = serde_json::json!({
        "engine": "ascii",
        "facet": { "id": id },
        "input": { "rgba": STANDARD.encode(vec![200u8; 8 * 8 * 4]), "width": 8, "height": 8 },
        "params": { "cols": 8, "cellAspect": 1.0 }
    });
    let resp = app
        .clone()
        .oneshot(post_json("/v1/render", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!json_body(resp).await["text"].as_str().unwrap().is_empty());
}
