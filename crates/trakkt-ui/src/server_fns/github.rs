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
    /// App configured but workspace not connected.
    NotConnected { app_slug: String },
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
use super::{require_workspace_admin, AuthenticatedContext, IntoServerFnError};

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
pub async fn get_github_integration_status(
) -> Result<GitHubIntegrationStatus, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let db = ac.db();

    // Check if GitHub App is configured via the database
    let app = trakkt_github::schema::get_github_app(db)
        .await
        .into_sfn()?;

    // Determine the app slug — from DB row or env-based config
    let app_slug = match app {
        Some(ref a) => a.app_name.clone(),
        None => {
            // No DB row — check if env-based config exists
            match std::env::var("GITHUB_APP_ID") {
                Ok(_) => std::env::var("GITHUB_APP_NAME")
                    .unwrap_or_else(|_| "trakkt".to_string()),
                Err(_) => return Ok(GitHubIntegrationStatus::NotConfigured),
            }
        }
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
        _ => Ok(GitHubIntegrationStatus::NotConnected { app_slug }),
    }
}

/// Process the GitHub App installation callback.
///
/// Called after the user installs (or reinstalls) the GitHub App on their
/// organization/account. Verifies the installation via GitHub's API, then
/// creates or reactivates the local installation record.
///
/// Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn process_github_callback(
    installation_id: i64,
    setup_action: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let db = ac.db();

    if setup_action != "install" {
        tracing::warn!(setup_action = %setup_action, "unexpected GitHub setup_action; only 'install' is handled");
        return Err(ServerFnError::new(format!(
            "Unsupported setup action '{}'; only 'install' is accepted",
            setup_action
        )));
    }

    // Build GitHubClient from env
    let client = trakkt_github::from_env()
        .ok_or_else(|| ServerFnError::new("GitHub App not configured"))?;

    // Call GitHub API to verify installation exists and get details
    let details = client
        .get_installation_details(installation_id as u64)
        .await
        .into_sfn()?;

    // Get the github_app row (must exist if from_env() succeeded)
    let app = trakkt_github::schema::get_github_app(db)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("GitHub App not configured in database"))?;

    // Resolve the repository list: None for "all", or a JSON array of full names
    let repos_json = resolve_repos_json(&client, installation_id as u64, &details).await?;

    // Check if installation already exists for this workspace (e.g. reconnecting)
    let existing = trakkt_github::schema::get_installation_for_workspace(db, &ac.ws_id)
        .await
        .into_sfn()?;

    if let Some(existing) = existing {
        // Reactivate the existing installation with updated details
        trakkt_github::schema::reactivate_installation(
            db,
            &existing.installation_id,
            installation_id,
            &details.account.login,
            &details.account.account_type,
            repos_json.as_ref(),
        )
        .await
        .into_sfn()?;
    } else {
        // Create new installation record
        trakkt_github::schema::create_installation(
            db,
            &ac.ws_id,
            &app.github_app_id,
            installation_id,
            &details.account.login,
            &details.account.account_type,
            repos_json.as_ref(),
        )
        .await
        .into_sfn()?;

        // Seed default transition rules for the workspace
        trakkt_github::schema::seed_default_transition_rules(db, &ac.ws_id)
            .await
            .into_sfn()?;
    }

    Ok(())
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
pub async fn toggle_transition_rule(
    rule_id: String,
    enabled: bool,
) -> Result<(), ServerFnError> {
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

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the target repos for an installation.
///
/// If the installation has access to "all" repos, returns `None`.
/// Otherwise, fetches the list from GitHub and returns a JSON array of full names.
#[cfg(feature = "ssr")]
async fn resolve_repos_json(
    client: &trakkt_github::GitHubClient,
    installation_id: u64,
    details: &trakkt_github::GitHubInstallationDetails,
) -> Result<Option<serde_json::Value>, ServerFnError> {
    if details.repository_selection == "all" {
        return Ok(None);
    }

    let token = client
        .request_installation_token(installation_id)
        .await
        .into_sfn()?;
    let repos = client
        .list_installation_repos(&token.token)
        .await
        .into_sfn()?;
    let repo_names: Vec<String> = repos.into_iter().map(|r| r.full_name).collect();

    match serde_json::to_value(repo_names) {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize repo names to JSON");
            Err(ServerFnError::new(format!("JSON serialization error: {e}")))
        }
    }
}
