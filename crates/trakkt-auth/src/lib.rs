// SPDX-License-Identifier: AGPL-3.0-or-later

//! trakkt-auth — Authentication & encryption for the Trakkt backend.

pub mod activity_service;
pub mod archive_service;
pub mod auth_service;
pub mod billing_service;
pub mod comment_service;
pub mod cookies;
pub mod email_service;
pub mod favorite_service;
pub mod encryption;
pub mod google_oauth;
pub mod issue_service;
pub mod jwt;
pub mod label_service;
pub mod mcp_session_manager;
pub mod middleware;
pub mod notification_service;
pub mod notifications;
pub mod onboarding_service;
pub mod password;
pub mod project_service;
pub mod rate_limiter;
pub mod relation_service;
pub mod redis_ops;
pub mod security_service;
pub mod session;
pub mod status_service;
pub mod stripe_service;
pub mod sync_log_service;
pub mod team_service;
pub mod token_refresh;
pub mod token_service;
pub mod totp;
pub mod user_service;
pub mod view_service;
pub mod watcher_service;
pub mod webauthn;
pub mod websocket;
pub mod workspace_service;

/// Build a shared HTTP client with a proper User-Agent header.
pub fn http_client() -> trakkt_core::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Trakkt/1.0")
        .build()
        .map_err(|e| trakkt_core::Error::Internal(format!("Failed to build HTTP client: {e}")))
}
