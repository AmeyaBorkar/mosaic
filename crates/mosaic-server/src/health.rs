//! `GET /healthz` — a stateless liveness probe for load balancers and orchestrators.

use axum::Json;
use serde_json::{Value, json};

pub async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
