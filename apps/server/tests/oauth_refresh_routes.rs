// SPDX-License-Identifier: AGPL-3.0-or-later

mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use serde_json::Value;
use tower::ServiceExt;
use trakkt_auth::token_service::{self, DeviceInfo};
use trakkt_core::test_helpers::seed_user;
use trakkt_server::routes;

// Clients use the exact OAuth error code to decide whether to reauthorize.
// Exercise the token endpoint with a real expired token and a missing token.
#[tokio::test]
async fn expired_and_unknown_refresh_tokens_return_invalid_grant() {
    let state = common::test_state().await;
    seed_user(&state.db, "oauth-user", "oauth@example.test")
        .await
        .unwrap();
    trakkt_core::db_execute!(
        &state.db,
        "INSERT INTO oauth_clients (id, client_id, name) VALUES ($1, $2, $3)",
        "00000000-0000-0000-0000-000000000001",
        "test-client",
        "Test MCP client"
    )
    .unwrap();
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
    .unwrap();
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
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let error: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["error"], "invalid_grant");
        assert_eq!(
            error["error_description"],
            "refresh token invalid or expired"
        );
    }
}
