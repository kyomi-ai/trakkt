// SPDX-License-Identifier: AGPL-3.0-or-later

//! Leptos frontend serving.
//!
//! Serves the Trunk-built SPA (index.html + WASM bundle + CSS) and
//! falls back to index.html for client-side routing.

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;

use crate::state::AppState;

/// Build a `ServeDir` service for static assets from the Trunk dist directory.
///
/// `not_found` is attached with [`ServeDir::fallback`], **not**
/// `not_found_service`. The latter wraps the fallback in
/// `tower_http::set_status::SetStatus`, whose response future does
/// `*response.status_mut() = self.status` unconditionally — it rewrites the
/// fallback's status to 404 while passing the body through untouched. With the
/// SPA shell as the fallback that turned every client-routed deep link into a
/// 404 carrying a page that renders fine, so anything branching on status
/// (crawlers, uptime monitors, CDN cacheability) treated every route as a
/// failure. `fallback` leaves the inner status alone, which is why the fallback
/// itself must decide between 200 and 404 — see [`serve_spa_fallback`].
///
/// The result is wrapped with `SetResponseHeader` to add
/// `Cache-Control: public, max-age=31536000, immutable` to responses that don't
/// already carry a `Cache-Control` header. Trunk adds content-hash suffixes to
/// WASM/CSS/JS filenames, so immutable caching is safe for the files it serves.
/// Every response [`serve_spa_fallback`] produces sets its own `Cache-Control`,
/// so `if_not_present` never stamps a year-long lifetime on a shell or a 404.
pub fn static_files_service<F>(
    dist_dir: &str,
    not_found: F,
) -> SetResponseHeader<ServeDir<F>, HeaderValue> {
    let svc = ServeDir::new(dist_dir).fallback(not_found);
    SetResponseHeader::if_not_present(
        svc,
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    )
}

/// Whether `path` names a file rather than a client route.
///
/// `ServeDir` calls its fallback for *any* path it has no file for
/// (`serve_dir/future.rs`, the `OpenFileOutput::FileNotFound` arm), and passes
/// no signal about which kind of miss it was. A missing `/nope.js` and an
/// unbuilt-on-the-server route like `/settings/workspace` arrive identically,
/// so the fallback has to tell them apart itself or one of the two gets the
/// wrong answer.
///
/// The discriminator is a dot in the final path segment. Every client route in
/// `crates/trakkt-ui/src/app.rs` is dot-free, including the parameterised ones:
/// the parameters are UUIDs (`:view_id`, `:id`, `:transfer_id`), team keys
/// (`:key`) and issue identifiers (`:identifier`, e.g. `TRA-123`). A new route
/// whose path or parameter values can contain a dot would be served a 404 here,
/// so keep them dot-free.
///
/// Header-based alternatives were rejected: `Accept: text/html` is sent by
/// browsers navigating and by crawlers, but not by `curl` or by most uptime
/// monitors, which send `*/*` — and those are two of the three consumers this
/// distinction exists to serve.
fn is_static_asset_path(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|last_segment| last_segment.contains('.'))
}

/// Handle a request `ServeDir` found no file for.
///
/// A client route gets the SPA shell with `200`, so the document status matches
/// the document that is actually served. A path naming a file that is not in
/// `dist_dir` gets a `404`, because it genuinely is not there and answering
/// with the app shell would turn a broken asset reference into a page of HTML
/// parsed as JavaScript.
pub async fn serve_spa_fallback(dist_dir: &str, path: &str) -> Response {
    if is_static_asset_path(path) {
        return asset_not_found();
    }
    serve(dist_dir).await
}

/// `404` for a static file that is not in the dist directory.
///
/// `no-cache` is deliberate: the immutable lifetime `static_files_service` adds
/// to header-less responses would otherwise pin "this file does not exist" in
/// browser and CDN caches for a year, outliving the deploy that adds the file.
fn asset_not_found() -> Response {
    let mut response = (StatusCode::NOT_FOUND, "Not found").into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    response
}

/// Serve the SPA index.html from `dist_dir`.
///
/// Sets `Cache-Control: no-cache` so browsers revalidate on every visit,
/// ensuring they always load the latest WASM bundle after deployments.
///
/// When `index.html` is absent the frontend was never built, which is an
/// operator error rather than a client one — but there is no page to serve, so
/// the honest status is still `404`, with a body that says what to do about it.
pub async fn serve(dist_dir: &str) -> Response {
    let index_path = format!("{dist_dir}/index.html");
    let mut response = match tokio::fs::read_to_string(&index_path).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "Frontend not built. Run: cd crates/trakkt-ui && trunk build",
        )
            .into_response(),
    };
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    response
}

/// Serve the Leptos shell HTML (for the explicit `/`, `/login`, `/signup`
/// routes, which axum matches before the static-file fallback is reached).
pub async fn serve_leptos_shell(State(state): State<AppState>) -> Response {
    serve(&state.config.dist_dir).await
}
