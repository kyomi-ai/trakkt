// SPDX-License-Identifier: AGPL-3.0-or-later

//! Route-level tests for how the SPA fallback answers a request no file
//! matches.
//!
//! The behaviour under test is a property of the *wiring*, not of any handler:
//! it is decided by how the static file service is attached to its fallback in
//! `build_router` (`apps/server/src/lib.rs`). Calling the handlers directly
//! would skip the exact layer that produced the bug, so every test here drives
//! the real `build_router` output with `tower::ServiceExt::oneshot`.
//!
//! What went wrong: the fallback was attached with `ServeDir::not_found_service`,
//! which wraps it in `tower_http::set_status::SetStatus`. That middleware's
//! response future runs `*response.status_mut() = self.status` unconditionally
//! (`tower-http-0.6.10/src/set_status.rs:131-136`), so the SPA shell — served
//! with `200` and rendering perfectly — reached the client stamped `404`. Every
//! consumer that reads the status rather than the body (crawlers, uptime
//! monitors, CDN cacheability rules, the browser console) saw each deep link
//! fail.
//!
//! `ServeDir::fallback` leaves the inner status alone. That alone is not the
//! whole fix: `ServeDir` calls its fallback for *any* miss and says nothing
//! about which kind it was, so with `fallback` a request for a genuinely
//! missing `/nope.js` would start answering `200` with a page of HTML. The
//! fallback therefore has to make the distinction itself, and the tests below
//! pin both halves of it.

// `pub` rather than plain `mod`: the fixture module is a shared toolkit and
// this binary uses only `test_state` from it. Under a private `mod` the token
// helpers it also exports are unreachable from this crate root, so `dead_code`
// fires on them and `-D warnings` turns that into a clippy failure. Making the
// module public states the fact — reachability is not this binary's to decide —
// instead of silencing the lint with an attribute, which the lint suppression
// policy forbids.
pub mod common;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use trakkt_server::state::AppState;

/// Marker content for the stand-in `index.html`.
///
/// Distinctive enough that "the response body is the app shell" and "the
/// response body is something else" can never be confused for one another.
const INDEX_HTML: &str = "<!doctype html><title>trakkt shell</title><div id=\"app\"></div>";

/// A throwaway directory standing in for the Trunk `dist` output.
///
/// `cargo test` never builds the frontend, so no real dist directory exists;
/// each test creates its own and populates it with exactly the files that test
/// needs. Owning a directory per test is also what keeps them independent —
/// `an_absent_index_html_reports_the_frontend_is_not_built` needs a dist
/// directory *without* `index.html` while its neighbours need one with it, and
/// the tests run concurrently.
struct TempDistDir(PathBuf);

impl TempDistDir {
    /// Create an empty dist directory. `label` only makes a leaked directory
    /// traceable to the test that made it; the uuid is what makes it unique.
    fn empty(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "trakkt-spa-routing-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("creating the temporary dist directory");
        Self(path)
    }

    /// Create a dist directory holding [`INDEX_HTML`] as `index.html`, which is
    /// the state a built frontend is in.
    fn with_index(label: &str) -> Self {
        let dir = Self::empty(label);
        std::fs::write(dir.0.join("index.html"), INDEX_HTML)
            .expect("writing index.html into the temporary dist directory");
        dir
    }

    fn path(&self) -> &str {
        self.0
            .to_str()
            .expect("the temporary dist directory path is valid UTF-8")
    }
}

impl Drop for TempDistDir {
    fn drop(&mut self) {
        // Reported rather than propagated: this runs during unwinding when a
        // test fails, and panicking there aborts the process and destroys the
        // assertion failure that matters. `eprintln!` rather than
        // `tracing::warn!` because an integration test binary installs no
        // subscriber, so a tracing event here would go nowhere.
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "could not remove temporary dist directory {}: {err}",
                self.0.display()
            );
        }
    }
}

/// The real router, with the dist directory pointed at `dist_dir`.
///
/// `build_router` reads the path from `config.dist_dir`, so overriding it on
/// the config is enough to redirect both the static file service and the SPA
/// fallback — and it does so without mutating process environment, which
/// concurrently running tests would race on.
async fn app(dist_dir: &str) -> Router {
    let mut state: AppState = common::test_state().await;
    let mut config = (*state.config).clone();
    config.dist_dir = dist_dir.to_string();
    state.config = Arc::new(config);
    trakkt_server::build_router(state)
}

/// `GET path` against the real router, returning status, body and the
/// `Cache-Control` header.
async fn get(router: Router, path: &str) -> (StatusCode, String, Option<String>) {
    let response = router
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("building the GET request"),
        )
        .await
        .expect("the router answers the request");

    let status = response.status();
    let cache_control = response
        .headers()
        .get(axum::http::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading the response body");

    (
        status,
        String::from_utf8(bytes.to_vec()).expect("the response body is UTF-8"),
        cache_control,
    )
}

/// A client-routed deep link is answered with the app shell **and** a status
/// that says so.
///
/// This is the reproduction: before the fix the body below was already correct
/// and the status was `404`, which is precisely why the defect survived — the
/// page rendered, so nothing that looked at the page noticed.
#[tokio::test]
async fn a_client_route_is_served_the_app_shell_with_200() {
    let dist = TempDistDir::with_index("client-route");
    let (status, body, cache_control) = get(app(dist.path()).await, "/settings/workspace").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "a client route must report the status of the document it actually \
         serves; got {status} with body {body:?}"
    );
    assert_eq!(body, INDEX_HTML, "a client route must be served the app shell");
    assert_eq!(
        cache_control.as_deref(),
        Some("no-cache"),
        "the shell must stay revalidated so a deploy is picked up"
    );
}

/// The explicitly registered shell routes are unaffected.
///
/// `/`, `/login` and `/signup` are matched by `Router::route` before the static
/// file fallback is consulted, so they never travelled through `SetStatus` and
/// were never part of the bug. They read the dist directory through a different
/// path than the fallback does, and this pins that the two agree.
#[tokio::test]
async fn an_explicitly_registered_shell_route_is_served_the_app_shell_with_200() {
    let dist = TempDistDir::with_index("login-route");
    let (status, body, _) = get(app(dist.path()).await, "/login").await;

    assert_eq!(status, StatusCode::OK, "/login is a registered shell route");
    assert_eq!(body, INDEX_HTML, "/login must be served the app shell");
}

/// A path naming a file that is not in the dist directory is still a `404`, and
/// is not answered with the app shell.
///
/// The status half is a regression guard, not a reproduction: it held before
/// the fix too, because `SetStatus` forced *every* fallback response to `404`.
/// It is here because the obvious one-line fix — swapping `not_found_service`
/// for `fallback` and stopping — breaks it, turning a missing script into `200`
/// plus a page of HTML that the browser then parses as JavaScript.
///
/// The body and `Cache-Control` assertions do fail before the fix: the old code
/// answered a missing asset with the shell, and a `404` that carried no
/// `Cache-Control` of its own would be stamped `immutable` for a year by the
/// static file service's header layer, outliving the deploy that adds the file.
#[tokio::test]
async fn a_missing_static_asset_is_still_a_404() {
    for path in ["/nope.js", "/assets/missing.css"] {
        let dist = TempDistDir::with_index("missing-asset");
        let (status, body, cache_control) = get(app(dist.path()).await, path).await;

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{path} names a file that is not in the dist directory"
        );
        assert_ne!(
            body, INDEX_HTML,
            "{path} must not be answered with the app shell — the browser would \
             parse the HTML as the asset it asked for"
        );
        assert_eq!(
            cache_control.as_deref(),
            Some("no-cache"),
            "a 404 for {path} must not be cached immutably; the deploy that adds \
             the file would not dislodge it"
        );
    }
}

/// With no `index.html` on disk there is no shell to serve, and the response
/// says so.
///
/// A regression guard, not a reproduction — this branch answered `404` before
/// the fix and must keep doing so. It is the one case where a client route
/// legitimately is not `200`, and the risk in making client routes succeed is
/// that this branch quietly starts succeeding too.
#[tokio::test]
async fn an_absent_index_html_reports_the_frontend_is_not_built() {
    let dist = TempDistDir::empty("no-index");
    let (status, body, _) = get(app(dist.path()).await, "/settings/workspace").await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "there is no shell to serve, so the client route cannot be a 200"
    );
    assert!(
        body.contains("Frontend not built"),
        "the body must say the frontend was never built rather than be empty; got {body:?}"
    );
}
