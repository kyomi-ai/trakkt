// SPDX-License-Identifier: AGPL-3.0-or-later

//! Route-level tests for MCP session termination and OAuth token expiry.
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

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
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

// Clients use the exact OAuth error code to decide whether to reauthorize.
// Exercise the token endpoint with a real expired token and a missing token.
#[tokio::test]
async fn expired_and_unknown_refresh_tokens_return_invalid_grant() {
    use axum::body::to_bytes;
    use chrono::{Duration, Utc};
    use serde_json::Value;
    use trakkt_auth::token_service::{self, DeviceInfo};
    let state = common::test_state().await;
    seed_user(&state.db, "oauth-user", "oauth@example.test")
        .await
        .expect("seed OAuth user");
    trakkt_core::db_execute!(
        &state.db,
        "INSERT INTO oauth_clients (id, client_id, name) VALUES ($1, $2, $3)",
        "00000000-0000-0000-0000-000000000001",
        "test-client",
        "Test MCP client"
    )
    .expect("seed OAuth client");
    token_service::store_refresh_token(
        &state.db,
        "oauth-user",
        &token_service::hash_refresh_token("expired-token"),
        Utc::now() - Duration::days(2),
        &DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: Some("test-client".into()),
        },
        "test-family",
    )
    .await
    .expect("store expired refresh token");
    let app = Router::new()
        .nest("/api/v1/oauth", routes::oauth::routes())
        .with_state(state);
    for token in ["expired-token", "unknown-token"] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/oauth/token")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "grant_type=refresh_token&client_id=test-client&refresh_token={token}"
                    )))
                    .expect("build refresh request"),
            )
            .await
            .expect("dispatch refresh request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 4096)
            .await
            .expect("read OAuth error body");
        let error: Value = serde_json::from_slice(&body).expect("decode OAuth error JSON");
        assert_eq!(error["error"], "invalid_grant");
        assert_eq!(
            error["error_description"],
            "refresh token invalid or expired"
        );
    }
}
