// SPDX-License-Identifier: AGPL-3.0-or-later

//! Leptos frontend serving.
//!
//! Serves the Trunk-built SPA (index.html + WASM bundle + CSS) and
//! falls back to index.html for client-side routing.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;
use tower_http::set_status::SetStatus;

/// Path to Trunk dist directory (relative to where the binary runs).
/// Set via TRUNK_DIST_DIR env var, or defaults to crates/trakkt-ui/dist.
fn dist_dir() -> String {
    std::env::var("TRUNK_DIST_DIR")
        .unwrap_or_else(|_| "crates/trakkt-ui/dist".to_string())
}

/// Build a ServeDir service for static assets from trunk dist.
///
/// The returned service is wrapped with `SetResponseHeader` to add
/// `Cache-Control: public, max-age=31536000, immutable` to all responses
/// that don't already have a `Cache-Control` header. Trunk adds content-hash
/// suffixes to WASM/CSS/JS filenames, so immutable caching is safe.
///
/// The `not_found_service` fallback (which calls [`serve()`]) already sets
/// `Cache-Control: no-cache`, so `if_not_present` preserves that header for
/// SPA route fallbacks while adding immutable caching for static assets.
pub fn static_files_service<F>(not_found: F) -> SetResponseHeader<ServeDir<SetStatus<F>>, HeaderValue> {
    let svc = ServeDir::new(dist_dir()).not_found_service(not_found);
    SetResponseHeader::if_not_present(
        svc,
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    )
}

/// Serve the SPA index.html from trunk dist (fallback for all routes).
///
/// Sets `Cache-Control: no-cache` so browsers revalidate on every visit,
/// ensuring they always load the latest WASM bundle after deployments.
pub async fn serve() -> Response {
    let index_path = format!("{}/index.html", dist_dir());
    match tokio::fs::read_to_string(&index_path).await {
        Ok(html) => {
            let mut response = Html(html).into_response();
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            );
            response
        }
        Err(_) => (StatusCode::NOT_FOUND, "Frontend not built. Run: cd crates/trakkt-ui && trunk build").into_response(),
    }
}

/// Serve the Leptos shell HTML (for explicit /login, /signup routes).
pub async fn serve_leptos_shell() -> Response {
    serve().await
}
