// SPDX-License-Identifier: AGPL-3.0-or-later

//! Health check HTTP server for `trakkt-connect`.
//!
//! Exposes a single `GET /healthz` endpoint that reports WebSocket connectivity.
//! Returns 200 when the agent is connected, 503 otherwise. Intended for use by
//! container orchestrators (Kubernetes liveness/readiness probes, Docker
//! HEALTHCHECK, etc.).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};

#[derive(Clone)]
struct HealthState {
    ws_connected: Arc<AtomicBool>,
}

/// Start the health check HTTP server.
///
/// Binds to `0.0.0.0:{port}` and serves until the process exits.
pub async fn run_health_server(port: u16, ws_connected: Arc<AtomicBool>) {
    let state = HealthState { ws_connected };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(port, "Health check server starting");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(port, error = %e, "Failed to bind health check server");
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "Health check server error");
    }
}

async fn healthz(State(state): State<HealthState>) -> impl IntoResponse {
    let ws = state.ws_connected.load(Ordering::Relaxed);

    let status = if ws {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(serde_json::json!({
            "status": if ws { "healthy" } else { "unhealthy" },
            "ws_connected": ws,
        })),
    )
}
