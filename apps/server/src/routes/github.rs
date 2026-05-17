// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub webhook endpoint — receives and verifies GitHub App webhook events.
//!
//! This module handles incoming webhook deliveries from GitHub. The endpoint is
//! exempt from auth middleware because GitHub POSTs to it directly — it relies
//! on HMAC-SHA256 signature verification instead.
//!
//! Flow:
//! 1. Read raw body for signature verification
//! 2. Verify X-Hub-Signature-256 against the webhook secret
//! 3. Check idempotency via X-GitHub-Delivery header
//! 4. Parse and dispatch the event to the appropriate handler
//! 5. Always return 200 to prevent GitHub from retrying

use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};

use trakkt_github::schema;
use trakkt_github::webhook::verify_signature;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the GitHub webhook router.
///
/// The webhook endpoint does NOT use auth middleware — it relies on
/// HMAC-SHA256 signature verification instead.
pub fn routes() -> Router<AppState> {
    Router::new().route("/github", post(github_webhook))
}

// ===========================================================================
// Webhook handler
// ===========================================================================

/// POST /webhooks/github — GitHub webhook handler (no auth).
async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, trakkt_core::Error> {
    // ── 1. Resolve webhook secret ──────────────────────────────────────────

    let webhook_secret = match resolve_webhook_secret(&state).await {
        Some(secret) => secret,
        None => {
            tracing::error!("GitHub webhook secret not configured — cannot verify signatures");
            return Err(trakkt_core::Error::Internal(
                "GitHub webhook secret not configured".into(),
            ));
        }
    };

    // ── 2. Verify signature ────────────────────────────────────────────────

    let signature = match headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok()) {
        Some(sig) => sig,
        None => {
            tracing::warn!("GitHub webhook missing X-Hub-Signature-256 header");
            return Err(trakkt_core::Error::Unauthorized(
                "Missing webhook signature".into(),
            ));
        }
    };

    if !verify_signature(webhook_secret.as_bytes(), &body, signature) {
        tracing::warn!("GitHub webhook signature verification failed");
        return Err(trakkt_core::Error::Unauthorized(
            "Invalid webhook signature".into(),
        ));
    }

    // ── 3. Check idempotency via delivery ID ───────────────────────────────

    let delivery_id = match headers.get("x-github-delivery").and_then(|v| v.to_str().ok()) {
        Some(id) => id.to_string(),
        None => {
            tracing::warn!("GitHub webhook missing X-GitHub-Delivery header");
            return Err(trakkt_core::Error::BadRequest(
                "Missing X-GitHub-Delivery header".into(),
            ));
        }
    };

    match schema::event_exists(&state.db, &delivery_id).await {
        Ok(true) => {
            tracing::debug!(delivery_id = %delivery_id, "Duplicate webhook delivery — already processed");
            return Ok(Json(json!({})));
        }
        Ok(false) => {}
        Err(e) => {
            tracing::error!(delivery_id = %delivery_id, error = %e, "Failed to check event idempotency");
            return Err(trakkt_core::Error::Internal(
                "Failed to check event idempotency".into(),
            ));
        }
    }

    // ── 4. Parse event type and payload ────────────────────────────────────

    let event_type = match headers.get("x-github-event").and_then(|v| v.to_str().ok()) {
        Some(t) => t.to_string(),
        None => {
            tracing::warn!("GitHub webhook missing X-GitHub-Event header");
            return Err(trakkt_core::Error::BadRequest(
                "Missing X-GitHub-Event header".into(),
            ));
        }
    };

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "GitHub webhook payload is not valid JSON");
            return Err(trakkt_core::Error::BadRequest(
                "Invalid JSON payload".into(),
            ));
        }
    };

    let action = payload.get("action").and_then(|v| v.as_str()).map(|s| s.to_string());

    // ── 5. Extract installation ID and look up in DB ───────────────────────

    let github_installation_id = payload
        .get("installation")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_i64());

    let installation = match github_installation_id {
        Some(gid) => match schema::get_installation_by_github_id(&state.db, gid).await {
            Ok(inst) => inst,
            Err(e) => {
                tracing::warn!(
                    github_installation_id = gid,
                    error = %e,
                    "Failed to look up GitHub installation"
                );
                None
            }
        },
        None => {
            tracing::debug!(
                event_type = %event_type,
                "Webhook has no installation.id — may be an app-level event"
            );
            None
        }
    };

    let internal_installation_id = installation.as_ref().map(|i| i.installation_id.as_str());

    // ── 6. Record the event ────────────────────────────────────────────────

    // Build a summary object with key fields for debugging (avoid storing full payload).
    let payload_summary = build_payload_summary(&event_type, &action, &payload);

    let event_id = match schema::create_event(
        &state.db,
        &delivery_id,
        internal_installation_id,
        &event_type,
        action.as_deref(),
        Some(&payload_summary),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(
                delivery_id = %delivery_id,
                error = %e,
                "Failed to record GitHub webhook event"
            );
            return Err(trakkt_core::Error::Internal(
                "Failed to record webhook event".into(),
            ));
        }
    };

    // ── 7. Dispatch event ──────────────────────────────────────────────────

    let event_key = match &action {
        Some(a) => format!("{event_type}.{a}"),
        None => event_type.clone(),
    };

    tracing::info!(
        delivery_id = %delivery_id,
        event = %event_key,
        installation_id = ?internal_installation_id,
        "Processing GitHub webhook"
    );

    let result = match event_key.as_str() {
        // Installation lifecycle events — handle synchronously
        "installation.deleted" | "installation.suspend" => {
            Some(handle_installation_suspend(&state, &installation).await)
        }
        "installation.unsuspend" => {
            Some(handle_installation_unsuspend(&state, &installation).await)
        }
        "installation_repositories.added" | "installation_repositories.removed" => {
            Some(handle_installation_repos_changed(&state, &installation, &payload).await)
        }
        // Events for TRA-91 queue — leave processed_at = NULL
        _ => {
            tracing::info!(
                event = %event_key,
                delivery_id = %delivery_id,
                "GitHub event recorded — awaiting processor (TRA-91)"
            );
            None
        }
    };

    // ── 8. Mark event outcome ──────────────────────────────────────────────

    match result {
        Some(Ok(())) => {
            if let Err(e) = schema::mark_event_processed(&state.db, &event_id).await {
                tracing::error!(event_id = %event_id, error = %e, "Failed to mark event as processed");
            }
        }
        Some(Err(e)) => {
            tracing::error!(event_id = %event_id, error = %e, "GitHub event processing failed");
            if let Err(mark_err) = schema::mark_event_failed(&state.db, &event_id, &e.to_string()).await {
                tracing::error!(event_id = %event_id, error = %mark_err, "Failed to mark event as failed");
            }
        }
        None => {} // Not handled here — left for TRA-91 queue worker
    }

    // Always return 200 — GitHub retries on non-200 status codes.
    Ok(Json(json!({})))
}

// ===========================================================================
// Event handlers
// ===========================================================================

/// Handle `installation.deleted` and `installation.suspend` — mark the
/// installation as suspended so we stop processing events for it.
async fn handle_installation_suspend(
    state: &AppState,
    installation: &Option<schema::GitHubInstallation>,
) -> Result<(), trakkt_core::Error> {
    let inst = match installation {
        Some(i) => i,
        None => {
            tracing::warn!("installation.suspend/deleted event but no matching installation in DB");
            return Ok(());
        }
    };

    schema::suspend_installation(&state.db, &inst.installation_id).await?;
    tracing::info!(
        installation_id = %inst.installation_id,
        account = %inst.account_login,
        "GitHub installation suspended"
    );
    Ok(())
}

/// Handle `installation.unsuspend` — clear the suspended timestamp.
async fn handle_installation_unsuspend(
    state: &AppState,
    installation: &Option<schema::GitHubInstallation>,
) -> Result<(), trakkt_core::Error> {
    let inst = match installation {
        Some(i) => i,
        None => {
            tracing::warn!("installation.unsuspend event but no matching installation in DB");
            return Ok(());
        }
    };

    schema::unsuspend_installation(&state.db, &inst.installation_id).await?;
    tracing::info!(
        installation_id = %inst.installation_id,
        account = %inst.account_login,
        "GitHub installation unsuspended"
    );
    Ok(())
}

/// Handle `installation_repositories.added` and `.removed` — update the
/// target_repos JSONB column with the current repository selection.
async fn handle_installation_repos_changed(
    state: &AppState,
    installation: &Option<schema::GitHubInstallation>,
    payload: &Value,
) -> Result<(), trakkt_core::Error> {
    let inst = match installation {
        Some(i) => i,
        None => {
            tracing::warn!("installation_repositories event but no matching installation in DB");
            return Ok(());
        }
    };

    // GitHub's `installation_repositories` webhook sends `repository_selection`
    // ("all" or "selected") plus delta arrays `repositories_added` / `repositories_removed`.
    // It does NOT send the full current repo list for the "selected" case, so we must
    // read-modify-write against the DB to maintain an accurate `target_repos`.
    let repository_selection = payload
        .get("repository_selection")
        .and_then(|v| v.as_str());

    let target_repos: Option<Value> = match repository_selection {
        Some("all") => {
            // "all" means every repo — store NULL per schema convention.
            None
        }
        _ => {
            // Extract full_name from the delta arrays
            let added: Vec<String> = payload
                .get("repositories_added")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| r.get("full_name").and_then(|n| n.as_str()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();

            let removed: Vec<String> = payload
                .get("repositories_removed")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| r.get("full_name").and_then(|n| n.as_str()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();

            // Read current repo list from DB and apply the delta
            let mut current_repos: Vec<String> = match inst.target_repos.as_deref() {
                None => Vec::new(),
                Some(s) => match serde_json::from_str::<Vec<String>>(s) {
                    Ok(repos) => repos,
                    Err(e) => {
                        tracing::warn!(
                            installation_id = %inst.installation_id,
                            error = %e,
                            "Failed to parse target_repos JSON — treating as empty"
                        );
                        Vec::new()
                    }
                },
            };

            // Add new repos (avoid duplicates)
            for repo in &added {
                if !current_repos.contains(repo) {
                    current_repos.push(repo.clone());
                }
            }

            // Remove repos
            current_repos.retain(|r| !removed.contains(r));

            current_repos.sort();
            Some(json!(current_repos))
        }
    };

    schema::update_installation_repos(
        &state.db,
        &inst.installation_id,
        target_repos.as_ref(),
    )
    .await?;

    tracing::info!(
        installation_id = %inst.installation_id,
        account = %inst.account_login,
        selection = ?repository_selection,
        "GitHub installation repositories updated"
    );
    Ok(())
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Resolve the webhook secret from environment or database.
///
/// Checks `GITHUB_WEBHOOK_SECRET` env var first (simpler self-hosted path),
/// then falls back to the `webhook_secret_encrypted` field in `github_apps`.
async fn resolve_webhook_secret(state: &AppState) -> Option<String> {
    // Prefer environment variable for self-hosted simplicity
    if let Ok(secret) = std::env::var("GITHUB_WEBHOOK_SECRET")
        && !secret.is_empty()
    {
        return Some(secret);
    }

    // Fall back to database-stored app config
    match schema::get_github_app(&state.db).await {
        Ok(Some(app)) => {
            if app.webhook_secret_encrypted.is_empty() {
                tracing::warn!("GitHub app webhook_secret_encrypted is empty");
                None
            } else {
                match trakkt_auth::encryption::decrypt(
                    &app.webhook_secret_encrypted,
                    &state.encryption_key,
                ) {
                    Ok(secret) => Some(secret),
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to decrypt GitHub webhook secret");
                        None
                    }
                }
            }
        }
        Ok(None) => {
            tracing::debug!("No GitHub app configured in database");
            None
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to load GitHub app from database");
            None
        }
    }
}

/// Build a compact payload summary for storage in the github_events table.
///
/// Extracts only the fields useful for debugging — avoids storing the full
/// (potentially large) webhook payload.
fn build_payload_summary(event_type: &str, action: &Option<String>, payload: &Value) -> Value {
    let mut summary = json!({
        "event_type": event_type,
    });

    if let Some(a) = action {
        summary["action"] = json!(a);
    }

    // Include sender login if present
    if let Some(login) = payload.pointer("/sender/login").and_then(|v| v.as_str()) {
        summary["sender"] = json!(login);
    }

    // Include repository full name if present
    if let Some(repo) = payload.pointer("/repository/full_name").and_then(|v| v.as_str()) {
        summary["repository"] = json!(repo);
    }

    // Include installation account if present
    if let Some(account) = payload.pointer("/installation/account/login").and_then(|v| v.as_str()) {
        summary["installation_account"] = json!(account);
    }

    summary
}
