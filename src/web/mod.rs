pub mod api;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tokio::net::TcpListener;
use tracing::info;

use crate::daemon::SharedDaemonState;

/// Builds the complete local Companion Web App router.
pub fn build_router(state: Arc<SharedDaemonState>) -> Router {
    Router::new()
        .route("/", get(api::serve_index))
        .route("/api/status", get(api::get_status))
        .route("/api/config", get(api::get_config).put(api::put_config))
        .route("/api/rumble-test", post(api::post_rumble_test))
        .route("/api/daemon/pause", post(api::post_pause))
        .route("/api/daemon/resume", post(api::post_resume))
        .route("/api/probe", get(api::get_probe))
        .route("/api/capture", post(api::post_capture))
        .route("/ws/live", get(api::ws_live))
        .with_state(state)
}

/// Starts the localhost-only Axum server until `shutdown` resolves.
///
/// Safety invariant: refuses to bind any non-loopback address.
pub async fn serve_web_ui<F>(
    bind_addr: SocketAddr,
    state: Arc<SharedDaemonState>,
    shutdown: F,
) -> Result<(), String>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if !bind_addr.ip().is_loopback() {
        return Err(format!(
            "refusing to bind Companion Web UI to non-loopback address {bind_addr}"
        ));
    }

    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|e| format!("failed to bind Companion Web UI at {bind_addr}: {e}"))?;
    info!(address = %bind_addr, "embedded Companion Web UI listening at http://{bind_addr}");

    let router = build_router(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| format!("Companion Web UI server error: {e}"))
}
