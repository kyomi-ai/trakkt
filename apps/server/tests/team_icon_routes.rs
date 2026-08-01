// SPDX-License-Identifier: AGPL-3.0-or-later

//! Route-level tests for `GET /api/v1/teams/{team_id}/icon`.
//!
//! The handler's protection is two separate things — the `AuthUser` extractor
//! and the workspace comparison inside the handler — and only a test that goes
//! through the router exercises the first of them. These drive the real
//! `Router` with `tower::ServiceExt::oneshot`, so the extractor, the JWT
//! validation and the error-to-status mapping all run as they do in production.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use trakkt_core::test_helpers::{seed_team, seed_user, seed_workspace};
use trakkt_server::routes;
use trakkt_server::state::AppState;

const WS_A: &str = "ws_alpha";
const WS_B: &str = "ws_beta";
const USER_A: &str = "usr_alpha";
const USER_B: &str = "usr_beta";
const TEAM_A: &str = "team_alpha";
const TEAM_B: &str = "team_beta";

/// PNG magic bytes plus a marker, so a leak is recognisable in a failure
/// message and greppable in a response body.
const ICON_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nalpha-team-icon-bytes";
const ICON_MIME: &str = "image/png";

/// The team-icon routes, mounted at the same prefix `build_router` uses
/// (`apps/server/src/lib.rs:81`), so the paths under test are the real ones.
fn app(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1/teams", routes::team_icon::routes())
        .with_state(state)
}

/// Two workspaces with one team and one member each, and a custom icon stored
/// on team A.
///
/// `USER_B` is the cross-tenant caller: a fully legitimate, authenticated user
/// who simply belongs to the other workspace. `TEAM_A` is a team they can name
/// but must not be able to read.
async fn two_workspaces() -> AppState {
    let state = common::test_state().await;
    let db = &state.db;

    seed_user(db, USER_A, "alpha@example.test")
        .await
        .expect("seed user A");
    seed_user(db, USER_B, "beta@example.test")
        .await
        .expect("seed user B");

    // Also enrols the owner as a workspace member, which `AuthUser` requires
    // before it will populate the workspace context.
    seed_workspace(db, WS_A, USER_A)
        .await
        .expect("seed workspace A");
    seed_workspace(db, WS_B, USER_B)
        .await
        .expect("seed workspace B");

    seed_team(db, TEAM_A, WS_A, "ALP").await.expect("seed team A");
    seed_team(db, TEAM_B, WS_B, "BET").await.expect("seed team B");

    // Store the icon directly rather than through `upload_icon`: these tests are
    // about who may read the bytes, and routing the fixture through the upload
    // handler would make a read test depend on the write handler's own auth.
    trakkt_core::db_execute!(
        db,
        "UPDATE teams SET icon_type = 'custom', icon_data = $1, icon_mime = $2 \
         WHERE team_id = $3",
        ICON_BYTES,
        ICON_MIME,
        TEAM_A
    )
    .expect("seed team A icon bytes");

    state
}

fn icon_path(team_id: &str) -> String {
    format!("/api/v1/teams/{team_id}/icon")
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body")
        .to_vec()
}

/// True if `haystack` contains the icon's marker bytes anywhere.
fn contains_icon_bytes(haystack: &[u8]) -> bool {
    haystack
        .windows(ICON_BYTES.len())
        .any(|window| window == ICON_BYTES)
}

#[tokio::test]
async fn get_icon_rejects_an_unauthenticated_request() {
    let state = two_workspaces().await;

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri(icon_path(TEAM_A))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a request with no token must not reach the handler"
    );

    let body = body_bytes(response).await;
    assert!(
        !contains_icon_bytes(&body),
        "the rejection must not carry the icon bytes"
    );
}

#[tokio::test]
async fn get_icon_returns_the_icon_to_a_member_of_its_workspace() {
    let state = two_workspaces().await;
    let token = common::access_token(&state, USER_A, WS_A);

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri(icon_path(TEAM_A))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "adding auth must not break the legitimate read"
    );
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some(ICON_MIME)
    );
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("private, max-age=3600"),
        "a user-scoped response must never be marked publicly cacheable"
    );

    let body = body_bytes(response).await;
    assert_eq!(body, ICON_BYTES, "the stored bytes come back verbatim");
}

#[tokio::test]
async fn get_icon_accepts_the_access_token_cookie() {
    let state = two_workspaces().await;
    let token = common::access_token(&state, USER_A, WS_A);
    let cookie = format!("{}={token}", common::access_token_cookie_name());

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri(icon_path(TEAM_A))
                .header("cookie", cookie)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router responds");

    // This is the case that makes requiring auth safe at all: the UI renders the
    // icon as a same-origin `<img src>`
    // (`crates/trakkt-ui/src/components/team_icon.rs:163`), which cannot set an
    // Authorization header — the browser sends only the cookie.
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the cookie fallback is what keeps the <img> tag working; without it \
         requiring auth would break every custom team icon in the UI"
    );

    let body = body_bytes(response).await;
    assert_eq!(body, ICON_BYTES);
}

#[tokio::test]
async fn get_icon_refuses_a_member_of_another_workspace() {
    let state = two_workspaces().await;

    // A real, valid session — just for the wrong tenant. This is precisely the
    // caller an authentication check alone would still serve.
    let token = common::access_token(&state, USER_B, WS_B);

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri(icon_path(TEAM_A))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "workspace B must not be able to read workspace A's team icon"
    );

    let body = body_bytes(response).await;
    assert!(
        !contains_icon_bytes(&body),
        "the refusal must not leak the icon bytes it just refused"
    );
}

#[tokio::test]
async fn get_icon_is_not_found_for_a_team_without_a_custom_icon() {
    let state = two_workspaces().await;
    // Team B is seeded with no icon, and user B legitimately belongs to it, so
    // this isolates the `Ok(None)` arm from the two rejection paths above.
    let token = common::access_token(&state, USER_B, WS_B);

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri(icon_path(TEAM_B))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router responds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
