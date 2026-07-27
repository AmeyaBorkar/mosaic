//! The `mosaic-server` binary: build the authoritative sandbox, bind the router, and serve
//! with graceful shutdown. Everything else lives in the library so it stays testable.

use std::sync::Arc;

use mosaic_runtime::Sandbox;
use mosaic_server::{AppState, app};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let sandbox = Arc::new(Sandbox::new()?);
    let router = app(AppState { sandbox });

    // Bind address is configurable; default to loopback so nothing is exposed by accident.
    let addr = std::env::var("MOSAIC_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("mosaic-server listening on http://{addr}");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolve when the process is asked to stop, so in-flight requests can drain.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("mosaic-server shutting down");
}
