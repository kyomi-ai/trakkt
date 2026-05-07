// SPDX-License-Identifier: AGPL-3.0-or-later

//! Onboarding service — business logic for the user onboarding flow.
//!
//! - [`accept_terms`] — validates a temp token (pending signup or pending
//!   terms), creates user accounts / workspace / sessions as needed, and
//!   returns the cookies that should be set on the HTTP response.

use axum::http::HeaderMap;
use trakkt_core::{Config, DbPool, KVPool};

use crate::{google_oauth, notifications, redis_ops, session, user_service};

/// Outcome of the [`accept_terms`] flow.
///
/// On success the caller must set `cookie_headers` on the HTTP response.
pub enum AcceptTermsOutcome {
    /// Terms accepted and session created. The caller must forward the
    /// included `Set-Cookie` headers to the HTTP response.
    Success { cookie_headers: HeaderMap },
    /// The token was not found — expired or invalid.
    InvalidToken,
}

// ---------------------------------------------------------------------------
// accept_terms
// ---------------------------------------------------------------------------

/// Accept terms of service, completing the signup or re-acceptance flow.
///
/// Orchestrates the full accept-terms workflow:
///
/// 1. Try **pending signup** (new user via Google OAuth):
///    - Create user account (verified, email confirmed by Google)
///    - Store OAuth credential data (encrypted)
///    - Mark terms acceptance
///    - Register `google_oauth` auth method
///    - Create personal workspace
///    - Create authenticated session
///    - Fire-and-forget admin signup notification
/// 2. Try **pending terms** (existing user needing re-acceptance):
///    - Mark terms acceptance
///    - Fetch user record
///    - Create authenticated session
/// 3. If neither token exists → [`AcceptTermsOutcome::InvalidToken`]
///
/// The caller is responsible for forwarding the `Set-Cookie` headers
/// included in [`AcceptTermsOutcome::Success`] to the HTTP response.
pub async fn accept_terms(
    pool: &DbPool,
    kv: &KVPool,
    encryption_key: &[u8; 32],
    config: &Config,
    device_info: &crate::token_service::DeviceInfo,
    temp_token: &str,
    marketing_consent: bool,
) -> trakkt_core::Result<AcceptTermsOutcome> {
    // ── Try pending signup first (new user via Google OAuth) ─────────────
    if let Some(signup_data) = redis_ops::get_pending_signup(kv, temp_token).await? {
        let email = signup_data["email"]
            .as_str()
            .ok_or_else(|| trakkt_core::Error::Internal("Missing email in signup data".into()))?;
        let name = signup_data["name"].as_str().unwrap_or("");

        // Create user (verified = true — OAuth means email is verified by Google)
        let user = user_service::create_user(pool, email, Some(name), true).await?;

        // Admin notification (Slack + email) — fire-and-forget
        let notify_webhook = config.slack_feedback_webhook_url.clone();
        let notify_support = config.support_email.clone();
        let notify_email = email.to_string();
        let notify_name = name.to_string();
        let notify_user_id = user.user_id.clone();
        tokio::spawn(async move {
            notifications::notify_signup(
                notify_webhook.as_deref(),
                &notify_support,
                &notify_email,
                &notify_name,
                &notify_user_id,
            )
            .await;
        });

        // Store OAuth data
        if let Some(oauth_data_json) = signup_data.get("oauth_data") {
            let oauth = google_oauth::OAuthData {
                google_id: oauth_data_json
                    .get("google_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                oauth_provider: oauth_data_json
                    .get("oauth_provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                picture: oauth_data_json
                    .get("picture")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
                ..Default::default()
            };
            let encrypted = google_oauth::build_oauth_data(&oauth, encryption_key)?;
            user_service::update_user_oauth_data(pool, &user.user_id, Some(&encrypted)).await?;
        }

        // Update terms acceptance
        user_service::update_terms_acceptance(
            pool,
            &user.user_id,
            trakkt_core::TERMS_VERSION,
            marketing_consent,
        )
        .await?;

        // Register google_oauth auth method
        let auth_data = serde_json::json!({
            "linked_at": chrono::Utc::now().to_rfc3339(),
        });
        user_service::upsert_auth_method(pool, &user.user_id, "google_oauth", &auth_data).await?;

        // Create personal workspace
        user_service::create_workspace_for_user(pool, &user.user_id, Some(name), email, Some(config))
            .await?;

        // Create authenticated session
        let sess =
            session::create_authenticated_session(pool, kv, &config.jwt_secret, &user, device_info)
                .await?;

        return Ok(AcceptTermsOutcome::Success {
            cookie_headers: sess.cookie_headers,
        });
    }

    // ── Try pending terms (existing user) ────────────────────────────────
    if let Some(terms_data) = redis_ops::get_pending_terms(kv, temp_token).await? {
        let user_id = terms_data["user_id"]
            .as_str()
            .ok_or_else(|| trakkt_core::Error::Internal("Missing user_id in terms data".into()))?;

        // Update terms acceptance
        user_service::update_terms_acceptance(
            pool,
            user_id,
            trakkt_core::TERMS_VERSION,
            marketing_consent,
        )
        .await?;

        // Get fresh user for session creation
        let user = user_service::get_user_by_id(pool, user_id)
            .await?
            .ok_or_else(|| trakkt_core::Error::Internal("User not found".into()))?;

        // Create authenticated session
        let sess =
            session::create_authenticated_session(pool, kv, &config.jwt_secret, &user, device_info)
                .await?;

        return Ok(AcceptTermsOutcome::Success {
            cookie_headers: sess.cookie_headers,
        });
    }

    // ── Neither found — token expired or invalid ─────────────────────────
    Ok(AcceptTermsOutcome::InvalidToken)
}

