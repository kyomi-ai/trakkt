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
    /// Visible connections, including inactive cards awaiting reconnection or removal.
    Connections {
        connections: Vec<GitHubConnectionDisplay>,
    },
}

/// Display data for one immutable GitHub installation identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubConnectionDisplay {
    pub connection_id: String,
    pub account_login: String,
    pub account_type: String,
    pub repos: Vec<String>,
    pub repository_selection: String,
    pub connected_at: String,
    pub github_installation_id: i64,
    pub active: bool,
    pub disconnected: bool,
    pub suspended: bool,
    pub uninstalled: bool,
    pub verified: bool,
    pub scope_pending: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError, require_workspace_admin};

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Query visible GitHub connections for the workspace.
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

    let installations = trakkt_github::schema::list_visible_installations_for_workspace(db, &ac.ws_id)
        .await
        .into_sfn()?;
    let mut connections = Vec::with_capacity(installations.len());
    for inst in installations {
        let repos = match inst.target_repos.as_deref() {
            None => Vec::new(),
            Some(json) => serde_json::from_str(json).map_err(|e| {
                tracing::error!(error = %e, "target_repos JSON is corrupt");
                ServerFnError::new(format!("stored repo list is invalid: {e}"))
            })?,
        };
        connections.push(GitHubConnectionDisplay {
            active: inst.is_active(),
            disconnected: inst.disconnected_at.is_some(),
            suspended: inst.suspended_at.is_some(),
            uninstalled: inst.uninstalled_at.is_some(),
            verified: inst.authorization_verified_at.is_some(),
            scope_pending: inst.repository_scope_pending,
            connection_id: inst.installation_id,
            account_login: inst.account_login,
            account_type: inst.account_type,
            repos,
            repository_selection: inst.repository_selection,
            connected_at: inst.created_at,
            github_installation_id: inst.github_installation_id,
        });
    }
    Ok(GitHubIntegrationStatus::Connections { connections })
}

/// Start a workspace-bound installation or reconnect authorization.
#[server(prefix = "/leptos-api")]
pub async fn start_github_connection(
    connection_id: Option<String>,
    reinstall: bool,
) -> Result<String, ServerFnError> {
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
        connection_id.as_deref(),
        reinstall,
    )
    .await
    .into_sfn()
}

/// Workspace choices for direct-install confirmation, from active memberships.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubConnectWorkspace {
    pub workspace_id: String,
    pub name: String,
    pub is_current: bool,
    pub is_admin: bool,
}

#[server(prefix = "/leptos-api")]
pub async fn get_github_connect_workspaces() -> Result<Vec<GitHubConnectWorkspace>, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    Ok(trakkt_auth::workspace_service::get_user_workspaces(&ctx.db, &auth.user_id)
        .await.into_sfn()?.into_iter().map(|(workspace, membership)| GitHubConnectWorkspace {
            is_current: auth.workspace.workspace_id.as_deref() == Some(&workspace.workspace_id),
            is_admin: membership.role.to_string() == "workspace_admin",
            workspace_id: workspace.workspace_id,
            name: workspace.name.unwrap_or_else(|| "Unnamed Workspace".into()),
        }).collect())
}

/// Explicitly confirmed workspace; active-workspace switches cannot retarget it.
#[server(prefix = "/leptos-api")]
pub async fn start_direct_github_connection(installation_id: i64, workspace_id: String) -> Result<String, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let client = ctx.github_client.as_deref().ok_or_else(|| ServerFnError::new("GitHub App not configured"))?;
    let key = ctx.encryption_key.as_deref().ok_or_else(|| ServerFnError::new("Credential encryption not configured"))?;
    trakkt_github::authorization::start_direct_connection(&ctx.db, client, key, &auth.user_id, &workspace_id, installation_id).await.into_sfn()
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
    trakkt_github::authorization::complete_connection_with_delivery(
        &ctx.db,
        client,
        key,
        &auth.user_id,
        &state,
        &code,
        ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()
}

/// Disconnect one owned connection while retaining its GitHub installation and history.
#[server(prefix = "/leptos-api")]
pub async fn disconnect_github(connection_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    trakkt_github::schema::disconnect_installation_with_delivery(
        ac.db(),
        &connection_id,
        &ac.ws_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()
}

/// Remove an inactive settings card, retaining installation history and ownership.
#[server(prefix = "/leptos-api")]
pub async fn remove_github(connection_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    trakkt_github::schema::remove_installation_with_delivery(
        ac.db(),
        &connection_id,
        &ac.ws_id,
        &ac.auth.user_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()
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

    trakkt_github::schema::update_transition_rule_enabled_with_delivery(
        db,
        &rule_id,
        &ac.ws_id,
        enabled,
        ac.ctx.ws_manager.as_ref(),
    )
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
