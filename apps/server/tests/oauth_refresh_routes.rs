// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exercise MCP refresh grants through the production OAuth router.
mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use tower::ServiceExt;
use trakkt_auth::{jwt, token_service};
use trakkt_core::{
    db_execute, db_fetch_scalar,
    test_helpers::{seed_user, seed_workspace},
};
use trakkt_server::{routes, state::AppState};

const USER: &str = "refresh-user";
const WORKSPACE: &str = "refresh-workspace";
const CLIENT: &str = "refresh-client";

async fn fixture() -> AppState {
    let state = common::test_state().await;
    seed_user(&state.db, USER, "refresh@example.test")
        .await
        .expect("seed refresh user");
    seed_workspace(&state.db, WORKSPACE, USER)
        .await
        .expect("seed refresh workspace");
    db_execute!(&state.db,
        "INSERT INTO oauth_clients (id, client_id, name, redirect_uris, scopes, client_type, active) VALUES ($1, $2, 'Refresh client', '[\"https://example.test/callback\"]', '[\"mcp\"]', 'public', 1)",
        &uuid::Uuid::new_v4().to_string(), CLIENT
    ).expect("seed active OAuth client");
    state
}

async fn store(state: &AppState, raw: &str, expiry: DateTime<Utc>, family: &str) -> String {
    token_service::store_refresh_token(
        &state.db,
        USER,
        &token_service::hash_refresh_token(raw),
        expiry,
        &token_service::DeviceInfo {
            oauth_client_id: Some(CLIENT.into()),
            user_agent: None,
            ip_address: None,
            country_code: None,
        },
        family,
    )
    .await
    .expect("store refresh grant")
}

async fn expiry(state: &AppState, id: &str) -> DateTime<Utc> {
    db_fetch_scalar!(
        &state.db,
        DateTime<Utc>,
        "SELECT expires_at FROM refresh_tokens WHERE token_id = $1",
        id
    )
    .expect("read refresh expiry")
}

async fn refresh(state: &AppState, raw: &str, client: &str) -> (StatusCode, Value) {
    let router = Router::new()
        .nest("/api/v1/oauth", routes::oauth::routes())
        .with_state(state.clone());
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!(
            "grant_type=refresh_token&refresh_token={raw}&client_id={client}"
        )))
        .expect("build refresh request");
    let response = router
        .oneshot(request)
        .await
        .expect("OAuth router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read OAuth response");
    (
        status,
        serde_json::from_slice(&bytes).expect("OAuth JSON response"),
    )
}

#[tokio::test]
async fn successful_refresh_slides_same_token_and_preserves_access_context() {
    let state = fixture().await;
    let original_deadline = Utc::now() + Duration::hours(1);
    let id = store(&state, "same-token", original_deadline, "same-family").await;
    let before = Utc::now();
    let (status, body) = refresh(&state, "same-token", CLIENT).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["refresh_token"], "same-token");
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["scope"], "mcp");
    // The issued bearer works at an authenticated MCP endpoint, with the
    // same workspace access as the existing browser session cookie.
    for (header, value) in [
        (
            "cookie",
            format!(
                "{}={}",
                common::access_token_cookie_name(),
                common::access_token(&state, USER, WORKSPACE)
            ),
        ),
        (
            "authorization",
            format!(
                "Bearer {}",
                body["access_token"].as_str().expect("issued bearer string")
            ),
        ),
    ] {
        let session = state.mcp_sessions.create_session(WORKSPACE).await;
        let request = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(header, value)
            .header("mcp-session-id", &session)
            .body(Body::empty())
            .expect("build authenticated MCP request");
        let response = Router::new()
            .nest("/mcp", routes::mcp::routes())
            .with_state(state.clone())
            .oneshot(request)
            .await
            .expect("MCP router response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(state.mcp_sessions.validate_session(&session).await, None);
    }
    let config = &trakkt_core::constants::get().jwt;
    assert_eq!(body["expires_in"], config.access_token_expire_minutes * 60);
    let claims = jwt::validate_token(
        body["access_token"].as_str().expect("access token string"),
        &state.config.jwt_secret,
    )
    .expect("refreshed JWT validates")
    .claims;
    assert_eq!(claims.sub, USER);
    assert_eq!(claims.extra["workspace_id"], WORKSPACE);
    assert_eq!(claims.extra["client_name"], "Refresh client");
    assert_eq!(claims.extra["email"], "refresh@example.test");
    let token_count: i64 = db_fetch_scalar!(&state.db, i64, "SELECT COUNT(*) FROM refresh_tokens")
        .expect("count refresh token rows after refresh");
    assert_eq!(
        token_count, 1,
        "MCP refresh must not rotate the stored grant"
    );
    let last_used: Option<DateTime<Utc>> = db_fetch_scalar!(
        &state.db,
        Option<DateTime<Utc>>,
        "SELECT last_used FROM refresh_tokens WHERE token_id = $1",
        &id
    )
    .expect("read refresh usage time");
    assert!(last_used.is_some());
    let renewed = expiry(&state, &id).await;
    assert!(renewed >= before + Duration::days(config.refresh_token_expire_days));
    assert!(renewed <= Utc::now() + Duration::days(config.refresh_token_expire_days));

    // Move persisted deadlines six days into the past to model elapsed time,
    // without sleeping or changing the production clock. The initial one-hour
    // deadline would now have passed; the renewed seven-day deadline has not.
    let elapsed = Duration::days(config.refresh_token_expire_days - 1);
    assert!(original_deadline - elapsed < Utc::now());
    let aged = renewed - elapsed;
    db_execute!(
        &state.db,
        "UPDATE refresh_tokens SET expires_at = $1 WHERE token_id = $2",
        &aged,
        &id
    )
    .expect("age renewed grant");
    assert_eq!(
        refresh(&state, "same-token", CLIENT).await.0,
        StatusCode::OK
    );
    assert!(expiry(&state, &id).await > aged);

    let inactive_deadline = Utc::now() - Duration::seconds(1);
    db_execute!(
        &state.db,
        "UPDATE refresh_tokens SET expires_at = $1 WHERE token_id = $2",
        &inactive_deadline,
        &id
    )
    .expect("model inactivity beyond renewed deadline");
    let (status, body) = refresh(&state, "same-token", CLIENT).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
    assert_eq!(expiry(&state, &id).await, inactive_deadline);
}

#[tokio::test]
async fn rejected_grants_never_extend_the_deadline() {
    for reason in [
        "expired",
        "revoked",
        "inactive-user",
        "inactive-client",
        "no-workspace",
        "unknown-client",
    ] {
        let state = fixture().await;
        let deadline = if reason == "expired" {
            Utc::now() - Duration::hours(1)
        } else {
            Utc::now() + Duration::hours(1)
        };
        let id = store(&state, "rejected-token", deadline, "rejected-family").await;
        match reason {
            "revoked" => {
                db_execute!(
                    &state.db,
                    "UPDATE refresh_tokens SET is_active = 0 WHERE token_id = $1",
                    &id
                )
                .expect("revoke grant");
            }
            "inactive-user" => {
                db_execute!(
                    &state.db,
                    "UPDATE users SET active = 0 WHERE user_id = $1",
                    USER
                )
                .expect("deactivate user");
            }
            "inactive-client" => {
                db_execute!(
                    &state.db,
                    "UPDATE oauth_clients SET active = 0 WHERE client_id = $1",
                    CLIENT
                )
                .expect("deactivate client");
            }
            "no-workspace" => {
                db_execute!(
                    &state.db,
                    "DELETE FROM workspace_users WHERE user_id = $1",
                    USER
                )
                .expect("remove membership");
            }
            _ => {}
        }
        let client = if reason == "unknown-client" {
            "absent-client"
        } else {
            CLIENT
        };
        let (status, body) = refresh(&state, "rejected-token", client).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{reason}: {body}");
        if !reason.ends_with("client") {
            assert_eq!(body["error"], "invalid_grant", "{reason}");
        }
        assert_eq!(expiry(&state, &id).await, deadline, "{reason}");
    }
    let state = fixture().await;
    let (status, body) = refresh(&state, "unknown-token", CLIENT).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn replaced_grace_token_is_not_extended_and_late_reuse_revokes_family() {
    let state = fixture().await;
    let deadline = Utc::now() + Duration::hours(1);
    let id = store(&state, "replaced-token", deadline, "rotated-family").await;
    let successor = store(&state, "successor-token", deadline, "rotated-family").await;
    let replaced = Utc::now();
    db_execute!(
        &state.db,
        "UPDATE refresh_tokens SET replaced_at = $1 WHERE token_id = $2",
        &replaced,
        &id
    )
    .expect("mark replaced token within grace");
    let (status, body) = refresh(&state, "replaced-token", CLIENT).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["refresh_token"], "replaced-token");
    assert_eq!(expiry(&state, &id).await, deadline);
    let late = Utc::now()
        - Duration::seconds(
            trakkt_core::constants::get()
                .jwt
                .refresh_token_grace_period_seconds
                + 1,
        );
    db_execute!(
        &state.db,
        "UPDATE refresh_tokens SET replaced_at = $1 WHERE token_id = $2",
        &late,
        &id
    )
    .expect("age replacement beyond grace");
    let (status, body) = refresh(&state, "replaced-token", CLIENT).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
    let active: bool = db_fetch_scalar!(
        &state.db,
        bool,
        "SELECT is_active FROM refresh_tokens WHERE token_id = $1",
        &successor
    )
    .expect("read successor revocation");
    assert!(!active, "late reuse must revoke the entire family");
    assert_eq!(expiry(&state, &id).await, deadline);
}
