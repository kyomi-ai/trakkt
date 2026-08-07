// SPDX-License-Identifier: AGPL-3.0-or-later

//! Route-level tests for `DELETE /mcp` — MCP session termination.
//!
//! The handler's protection is two separate things — the authentication gate
//! and the workspace comparison that follows it — and both live inside the
//! handler rather than in an extractor, so only a request driven through the
//! real `Router` exercises them as production does. These use
//! `tower::ServiceExt::oneshot` against the same `Router` `build_router` mounts
//! at `/mcp` (`apps/server/src/lib.rs:83`).
//!
//! What is at stake is availability, not disclosure: `remove_session` evicts a
//! session entry and nothing else. The session id has to be known to be used,
//! and it is minted by `MCPSessionManager::create_session` and returned only to
//! the client whose `initialize` authenticated — so there is no enumeration
//! path from outside. But session ids travel in a plain request header and are
//! not treated as secrets anywhere else, and terminating an authenticated
//! user's MCP session is not something an unauthenticated request may do.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use trakkt_core::config::TrakktMode;
use trakkt_core::test_helpers::{seed_user, seed_workspace};
use trakkt_server::routes;
use trakkt_server::state::AppState;

const WS_A: &str = "ws_alpha";
const WS_B: &str = "ws_beta";
const USER_A: &str = "usr_alpha";
const USER_B: &str = "usr_beta";

/// The header `handle_delete` reads the session id from, spelled exactly as the
/// MCP 2025-03-26 spec does.
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// The MCP routes, mounted at the same prefix `build_router` uses
/// (`apps/server/src/lib.rs:83`), so the path under test is the real one.
fn app(state: AppState) -> Router {
    Router::new()
        .nest("/mcp", routes::mcp::routes())
        .with_state(state)
}

/// Two workspaces with one member each.
///
/// `USER_B` is the cross-tenant caller: a fully legitimate, authenticated user
/// who simply belongs to the other workspace. `resolve_auth` takes the
/// workspace from the JWT claims and only re-reads the user row
/// (`apps/server/src/routes/auth_shared.rs:72-99`), so the seeded users are
/// what make the tokens resolve at all.
async fn two_workspaces() -> AppState {
    let state = common::test_state().await;
    let db = &state.db;

    seed_user(db, USER_A, "alpha@example.test")
        .await
        .expect("seed user A");
    seed_user(db, USER_B, "beta@example.test")
        .await
        .expect("seed user B");

    seed_workspace(db, WS_A, USER_A)
        .await
        .expect("seed workspace A");
    seed_workspace(db, WS_B, USER_B)
        .await
        .expect("seed workspace B");

    state
}

/// The same state with the deployment mode flipped to personal.
///
/// Personal mode is a single-user desktop deployment with no login at all, so
/// all three `/mcp` handlers bypass authentication in it. `common::test_state`
/// deliberately asserts the opposite mode — every auth assertion built on it
/// would be vacuous otherwise — so a personal-mode test has to say so
/// explicitly, here, rather than by weakening the shared fixture.
fn into_personal_mode(mut state: AppState) -> AppState {
    let mut config = (*state.config).clone();
    config.mode = TrakktMode::Personal;
    assert!(
        config.is_personal(),
        "the personal-mode test needs a config `is_personal()` actually reports \
         as personal — the handlers branch on that method, not on the field"
    );
    state.config = Arc::new(config);
    state
}

/// A `DELETE /mcp` request carrying `session_id`, and optionally a token.
fn delete_request(session_id: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("DELETE")
        .uri("/mcp")
        .header(MCP_SESSION_ID_HEADER, session_id);

    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

    builder.body(Body::empty()).expect("build request")
}

#[tokio::test]
async fn delete_rejects_an_unauthenticated_request() {
    let state = two_workspaces().await;
    let session_id = state.mcp_sessions.create_session(WS_A).await;

    let response = app(state.clone())
        .oneshot(delete_request(&session_id, None))
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a DELETE with no token must not reach the session store — its two \
         siblings on this route both answer 401 here"
    );
    assert_eq!(
        state.mcp_sessions.validate_session(&session_id).await,
        Some(WS_A.to_string()),
        "the refused request must leave the session usable; a 401 that still \
         terminated the session would be the same denial of service with a \
         different status code"
    );
}

#[tokio::test]
async fn delete_terminates_the_callers_own_session() {
    let state = two_workspaces().await;
    let session_id = state.mcp_sessions.create_session(WS_A).await;
    let token = common::access_token(&state, USER_A, WS_A);

    let response = app(state.clone())
        .oneshot(delete_request(&session_id, Some(&token)))
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "adding the gate must not break the one caller it is meant to serve"
    );
    assert_eq!(
        state.mcp_sessions.validate_session(&session_id).await,
        None,
        "a client terminating the session its own `initialize` was handed must \
         still have it removed"
    );
}

#[tokio::test]
async fn delete_accepts_the_access_token_cookie() {
    let state = two_workspaces().await;
    let session_id = state.mcp_sessions.create_session(WS_A).await;
    let token = common::access_token(&state, USER_A, WS_A);
    let cookie = format!("{}={token}", common::access_token_cookie_name());

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/mcp")
                .header(MCP_SESSION_ID_HEADER, &session_id)
                .header("cookie", cookie)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router responds");

    // `resolve_auth` falls back to the `access_token` cookie when there is no
    // Authorization header (`apps/server/src/routes/auth_shared.rs:224-234`),
    // and the gate inherits that whole path rather than re-deriving a narrower
    // one. This is the transport with a caller behind it, not a hypothetical:
    // `e2e/tests/mcp/mcp-endpoint.spec.ts:135` terminates its session through
    // `page.request.fetch`, which sends the browser context's cookies and no
    // Authorization header at all. A gate that took only the header would leave
    // that caller unable to clean up, and would fail here first.
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        state.mcp_sessions.validate_session(&session_id).await,
        None,
        "the cookie must authenticate the DELETE on the same terms the header does"
    );
}

#[tokio::test]
async fn delete_refuses_a_session_belonging_to_another_workspace() {
    let state = two_workspaces().await;
    let session_id = state.mcp_sessions.create_session(WS_A).await;

    // A real, valid token — just for the wrong tenant. This is precisely the
    // caller an authentication check alone would still serve.
    let token = common::access_token(&state, USER_B, WS_B);

    let response = app(state.clone())
        .oneshot(delete_request(&session_id, Some(&token)))
        .await
        .expect("router responds");

    // 204 rather than 403/404 on purpose: an unknown session id answers 204
    // too, so a caller cannot use the status to learn whether a session id
    // exists in a workspace they are not in.
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "the refusal must not be distinguishable from a delete of an unknown \
         session id"
    );
    assert_eq!(
        state.mcp_sessions.validate_session(&session_id).await,
        Some(WS_A.to_string()),
        "workspace B must not be able to terminate workspace A's MCP session"
    );
}

#[tokio::test]
async fn delete_in_personal_mode_needs_no_credentials() {
    let state = into_personal_mode(two_workspaces().await);
    let session_id = state.mcp_sessions.create_session(WS_A).await;

    let response = app(state.clone())
        .oneshot(delete_request(&session_id, None))
        .await
        .expect("router responds");

    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "personal mode has no credentials to present — a gate that demanded \
         them would break session cleanup for the whole desktop deployment"
    );
    assert_eq!(
        state.mcp_sessions.validate_session(&session_id).await,
        None,
        "personal mode must still terminate the session, not merely answer 204"
    );
}
