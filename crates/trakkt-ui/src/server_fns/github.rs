// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for GitHub integration settings.
//!
//! Supports the self-service GitHub App installation flow:
//! querying the current integration status, processing the OAuth callback
//! from GitHub after installation, and disconnecting.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Shared types (available on both client and server)
// ─────────────────────────────────────────────────────────────────────────────

/// A single transition rule for display in the settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRuleDisplay {
    pub rule_id: String,
    pub trigger_event: String,
    pub close_intent_required: bool,
    pub target_status_category: String,
    pub enabled: bool,
}

/// The current state of GitHub integration for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitHubIntegrationStatus {
    /// No GitHub App configured (self-hosted, no env vars set).
    NotConfigured,
    /// App automation is configured, but self-service authorization is missing.
    AuthorizationNotConfigured,
    /// App configured but workspace not connected.
    NotConnected { app_slug: String, retained_installation: bool },
    /// Workspace connected to GitHub.
    Connected {
        account_login: String,
        account_type: String,
        repos: Vec<String>,
        connected_at: String,
        github_installation_id: i64,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError, require_workspace_admin};

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Query the current GitHub integration status for the workspace.
///
/// Returns one of three variants:
/// - `NotConfigured` if the GitHub App is not set up at all
/// - `NotConnected` if the App exists but this workspace hasn't installed it
/// - `Connected` with account details if the workspace has an active installation
#[server(prefix = "/leptos-api")]
pub async fn get_github_integration_status() -> Result<GitHubIntegrationStatus, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let db = ac.db();

    let Some(client) = ac.ctx.github_client.as_deref() else {
        return Ok(GitHubIntegrationStatus::NotConfigured);
    };

    if !client.user_authorization_configured() {
        return Ok(GitHubIntegrationStatus::AuthorizationNotConfigured);
    }

    // Check if GitHub App is configured via the database
    let app = trakkt_github::schema::get_github_app(db).await.into_sfn()?;

    // Determine the app slug from the persisted or running configuration.
    let app_slug = match app {
        Some(ref a) => a.app_name.clone(),
        None => client.app_name().to_string(),
    };

    // Check if workspace has an active installation
    let installation = trakkt_github::schema::get_installation_for_workspace(db, &ac.ws_id)
        .await
        .into_sfn()?;

    match installation {
        Some(inst) if inst.suspended_at.is_none() => {
            // Parse target_repos JSON to get repo names
            let repos: Vec<String> = match inst.target_repos.as_deref() {
                None => Vec::new(),
                Some(json_str) => serde_json::from_str(json_str).map_err(|e| {
                    tracing::error!(error = %e, "target_repos JSON is corrupt");
                    ServerFnError::new(format!("stored repo list is invalid: {e}"))
                })?,
            };

            Ok(GitHubIntegrationStatus::Connected {
                account_login: inst.account_login,
                account_type: inst.account_type,
                repos,
                connected_at: inst.created_at,
                github_installation_id: inst.github_installation_id,
            })
        }
        other => Ok(GitHubIntegrationStatus::NotConnected { app_slug, retained_installation: other.is_some() }),
    }
}

/// Start a workspace-bound installation or reconnect authorization.
#[server(prefix = "/leptos-api")]
pub async fn start_github_connection(reinstall: bool) -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let client = ac
        .ctx
        .github_client
        .as_deref()
        .ok_or_else(|| ServerFnError::new("GitHub App not configured"))?;
    let key = ac
        .ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Credential encryption not configured"))?;
    trakkt_github::authorization::start_connection(
        ac.db(),
        client,
        key,
        &ac.auth.user_id,
        &ac.ws_id,
        reinstall,
    )
    .await
    .into_sfn()
}

/// Setup callback selects a candidate and returns a separate OAuth redirect.
#[server(prefix = "/leptos-api")]
pub async fn process_github_callback(
    installation_id: i64,
    setup_action: String,
    state: String,
) -> Result<String, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let client = ctx
        .github_client
        .as_deref()
        .ok_or_else(|| ServerFnError::new("GitHub App not configured"))?;
    let key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Credential encryption not configured"))?;
    trakkt_github::authorization::advance_setup(
        &ctx.db,
        client,
        key,
        &auth.user_id,
        &state,
        installation_id,
        &setup_action,
    )
    .await
    .into_sfn()
}

/// OAuth callback completes the original workspace's connection; active
/// workspace selection is intentionally irrelevant to this destination.
#[server(prefix = "/leptos-api")]
pub async fn complete_github_authorization(
    state: String,
    code: String,
) -> Result<(), ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let client = ctx
        .github_client
        .as_deref()
        .ok_or_else(|| ServerFnError::new("GitHub App not configured"))?;
    let key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Credential encryption not configured"))?;
    trakkt_github::authorization::complete_connection(
        &ctx.db,
        client,
        key,
        &auth.user_id,
        &state,
        &code,
    )
    .await
    .into_sfn()
}

/// Disconnect the GitHub integration for the current workspace.
///
/// Marks the installation as suspended (soft delete) so it can be
/// reactivated later. Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn disconnect_github() -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let db = ac.db();

    let installation = trakkt_github::schema::get_installation_for_workspace(db, &ac.ws_id)
        .await
        .into_sfn()?;

    match installation {
        Some(inst) => {
            trakkt_github::schema::suspend_installation(db, &inst.installation_id)
                .await
                .into_sfn()?;
            Ok(())
        }
        None => Err(ServerFnError::new("No GitHub integration found")),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transition rule server functions
// ─────────────────────────────────────────────────────────────────────────────

/// List all transition rules for the current workspace.
///
/// Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn get_transition_rules() -> Result<Vec<TransitionRuleDisplay>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let db = ac.db();

    let rules = trakkt_github::schema::list_transition_rules(db, &ac.ws_id)
        .await
        .into_sfn()?;

    Ok(rules
        .into_iter()
        .map(|r| TransitionRuleDisplay {
            rule_id: r.rule_id,
            trigger_event: r.trigger_event,
            close_intent_required: r.close_intent_required,
            target_status_category: r.target_status_category,
            enabled: r.enabled,
        })
        .collect())
}

/// Toggle the `enabled` flag on a single transition rule.
///
/// Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn toggle_transition_rule(rule_id: String, enabled: bool) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let db = ac.db();

    trakkt_github::schema::update_transition_rule_enabled(db, &rule_id, &ac.ws_id, enabled)
        .await
        .into_sfn()?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// GitHub link display types and server functions
// ─────────────────────────────────────────────────────────────────────────────

/// GitHub link data for display in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubLinkDisplay {
    pub link_id: String,
    pub link_type: String,
    pub repo_full_name: String,
    pub ref_identifier: String,
    pub title: Option<String>,
    pub state: Option<String>,
    pub url: String,
    pub author_login: Option<String>,
    pub close_intent: bool,
    pub created_at: String,
}

/// List all GitHub links (PRs, branches, commits) for an issue.
#[server(prefix = "/leptos-api")]
pub async fn list_github_links_for_issue(
    team_key: String,
    number: i32,
) -> Result<Vec<GitHubLinkDisplay>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let db = ac.db();

    let issue = trakkt_auth::issue_service::get_issue(db, &ac.ws_id, &team_key, number)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Issue {team_key}-{number} not found")))?;

    let links = trakkt_github::schema::list_links_for_issue(db, &issue.issue_id)
        .await
        .into_sfn()?;

    let display_links: Vec<GitHubLinkDisplay> = links
        .into_iter()
        .map(|link| GitHubLinkDisplay {
            link_id: link.link_id,
            link_type: link.link_type,
            repo_full_name: link.repo_full_name,
            ref_identifier: link.ref_identifier,
            title: link.title,
            state: link.state,
            url: link.url,
            author_login: link.author_login,
            close_intent: link.close_intent,
            created_at: link.created_at,
        })
        .collect();

    Ok(display_links)
}
