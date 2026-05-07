// SPDX-License-Identifier: AGPL-3.0-or-later

//! Leptos frontend serving.
//!
//! Serves the Trunk-built SPA (index.html + WASM bundle + CSS) and
//! falls back to index.html for client-side routing.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use tower_http::services::ServeDir;

/// Path to Trunk dist directory (relative to where the binary runs).
/// Set via TRUNK_DIST_DIR env var, or defaults to crates/trakkt-ui/dist.
fn dist_dir() -> String {
    std::env::var("TRUNK_DIST_DIR")
        .unwrap_or_else(|_| "crates/trakkt-ui/dist".to_string())
}

/// Build a ServeDir service for static assets from trunk dist.
pub fn static_files_service() -> ServeDir {
    ServeDir::new(dist_dir())
}

/// Serve the SPA index.html from trunk dist (fallback for all routes).
pub async fn serve() -> Response {
    let index_path = format!("{}/index.html", dist_dir());
    match tokio::fs::read_to_string(&index_path).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Frontend not built. Run: cd crates/trakkt-ui && trunk build").into_response(),
    }
}

/// Serve the Leptos shell HTML (for explicit /login, /signup routes).
pub async fn serve_leptos_shell() -> Response {
    serve().await
}
