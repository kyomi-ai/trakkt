// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue CRUD operations.
//!
//! Thin wrappers around `trakkt_auth::issue_service` — extract auth,
//! call service, return. No business logic lives here.
//!
//! Leptos server functions receive each field as a separate parameter
//! (no struct grouping), so many-argument signatures are unavoidable.
//! The workspace-root `clippy.toml` raises the threshold to 14.

use leptos::prelude::*;
use trakkt_types::models::{Issue, IssueWithDetails};

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Helpers (server-only) ─────────────────────────────────────────────────

#[cfg(feature = "ssr")]
fn parse_label_ids(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Resolve a team-scoped issue identifier (team_key + number) to its `issue_id`.
///
/// Used by server functions that need to convert from the user-facing identifier
/// (e.g. "ENG-42") to the internal UUID used by service functions.
#[cfg(feature = "ssr")]
pub(crate) async fn resolve_issue_id(
    db: &trakkt_core::DbPool,
    workspace_id: &str,
    team_key: &str,
    number: i32,
) -> Result<String, ServerFnError> {
    use super::IntoServerFnError;
    let issue = trakkt_auth::issue_service::get_issue(db, workspace_id, team_key, number)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Issue {team_key}-{number} not found")))?;
    Ok(issue.issue_id)
}

// ─── Read operations ───────────────────────────────────────────────────────

/// List issues in the current workspace with optional filters.
#[server(prefix = "/leptos-api")]
pub async fn list_issues(
    team_id: Option<String>,
    status_id: Option<String>,
    priority: Option<i32>,
    assignee_id: Option<String>,
    label_id: Option<String>,
    search: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<IssueWithDetails>, ServerFnError> {
    use trakkt_types::models::IssueFilters;

    let ac = AuthenticatedContext::extract().await?;
    let filters = IssueFilters {
        status_id,
        priority,
        assignee_id,
        label_id,
        search,
        limit,
        offset,
        ..Default::default()
    };
    let issues = trakkt_auth::issue_service::list_issues(ac.db(), &ac.ws_id, team_id.as_deref(), &filters)
        .await
        .into_sfn()?;
    Ok(issues)
}

/// Get a single issue by its team key + number (e.g. "ENG-42").
#[server(prefix = "/leptos-api")]
pub async fn get_issue(team_key: String, number: i32) -> Result<Option<IssueWithDetails>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue = trakkt_auth::issue_service::get_issue(ac.db(), &ac.ws_id, &team_key, number)
        .await
        .into_sfn()?;
    Ok(issue)
}

/// List sub-issues (direct children) of a given parent issue.
#[server(prefix = "/leptos-api")]
pub async fn list_sub_issues(
    parent_issue_id: String,
) -> Result<Vec<IssueWithDetails>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let sub_issues = trakkt_auth::issue_service::list_sub_issues(ac.db(), &parent_issue_id, &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(sub_issues)
}

/// Get the parent issue ID for a given child issue (team_key + number).
///
/// Returns `None` if the issue has no parent.
#[server(prefix = "/leptos-api")]
pub async fn get_parent_issue_id(
    team_key: String,
    number: i32,
) -> Result<Option<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = resolve_issue_id(ac.db(), &ac.ws_id, &team_key, number).await?;
    let parent_id = trakkt_auth::relation_service::get_parent_issue_id(ac.db(), &issue_id)
        .await
        .into_sfn()?;
    Ok(parent_id)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Create a new issue in the specified team, or the default team if none given.
///
/// `label_ids` is a comma-separated string of label UUIDs (per CODING_STANDARDS.md:
/// never use `Vec<String>` as a server function parameter).
#[server(prefix = "/leptos-api")]
pub async fn create_issue(
    title: String,
    description: Option<String>,
    priority: i32,
    assignee_id: Option<String>,
    due_date: Option<String>,
    label_ids: String,
    project_id: Option<String>,
    milestone_id: Option<String>,
    parent_issue_id: Option<String>,
    team_id: Option<String>,
) -> Result<Issue, ServerFnError> {
    use trakkt_types::models::CreateIssueParams;

    let ac = AuthenticatedContext::extract().await?;

    // Use the provided team_id, or fall back to the default team.
    let resolved_team_id = match team_id {
        Some(id) => id,
        None => {
            let team = trakkt_auth::team_service::get_default_team(ac.db(), &ac.ws_id)
                .await
                .into_sfn()?;
            team.team_id
        }
    };

    let parsed_label_ids = parse_label_ids(&label_ids);

    // Treat empty string as None (sentinel pattern for clearable fields).
    let resolved_parent_issue_id = parent_issue_id.filter(|s| !s.is_empty());

    let params = CreateIssueParams {
        workspace_id: ac.ws_id.clone(),
        team_id: resolved_team_id,
        creator_id: ac.auth.user_id.clone(),
        title,
        description,
        priority,
        assignee_id,
        due_date,
        label_ids: parsed_label_ids,
        project_id,
        milestone_id,
    };

    let issue = trakkt_auth::issue_service::create_issue(ac.db(), &params, ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;

    // If a parent was specified, create the parent relation after issue creation.
    if let Some(parent_id) = resolved_parent_issue_id {
        trakkt_auth::relation_service::set_parent(
            ac.db(),
            &ac.ws_id,
            &issue.issue_id,
            &parent_id,
            Some(&ac.auth.user_id),
            ac.ctx.ws_manager.as_ref(),
        )
        .await
        .into_sfn()?;
    }

    Ok(issue)
}

/// Update fields on an existing issue.
///
/// For clearable fields (description, assignee_id, due_date), use sentinel values:
/// - `None` = no change
/// - `Some("")` = clear the field (set to NULL)
/// - `Some("value")` = set to new value
///
/// This avoids `Option<Option<T>>` which cannot round-trip through Leptos form encoding.
///
/// `clear_fields` is a comma-separated list of relation fields to set to NULL:
/// `"sort_order"`, `"parent"`, `"project"`, `"milestone"` (any combination).
///
/// Parent issue changes are handled via `relation_service` rather than the issues
/// table. Pass `parent_issue_id` to set a parent, or include `"parent"` in
/// `clear_fields` to remove the current parent.
#[server(prefix = "/leptos-api")]
pub async fn update_issue(
    team_key: String,
    number: i32,
    title: Option<String>,
    description: Option<String>,
    status_id: Option<String>,
    priority: Option<i32>,
    assignee_id: Option<String>,
    due_date: Option<String>,
    project_id: Option<String>,
    milestone_id: Option<String>,
    parent_issue_id: Option<String>,
    clear_fields: Option<String>,
) -> Result<Issue, ServerFnError> {
    use trakkt_types::models::IssueUpdate;

    let ac = AuthenticatedContext::extract().await?;

    let clears: Vec<&str> = clear_fields
        .as_deref()
        .map(|s| s.split(',').map(str::trim).collect())
        .unwrap_or_default();

    let updates = IssueUpdate {
        title,
        description: description.map(|s| if s.is_empty() { None } else { Some(s) }),
        status_id,
        priority,
        assignee_id: assignee_id.map(|s| if s.is_empty() { None } else { Some(s) }),
        due_date: due_date.map(|s| if s.is_empty() { None } else { Some(s) }),
        project_id: if clears.contains(&"project") {
            Some(None)
        } else {
            project_id.map(|s| if s.is_empty() { None } else { Some(s) })
        },
        milestone_id: if clears.contains(&"milestone") {
            Some(None)
        } else {
            milestone_id.map(|s| if s.is_empty() { None } else { Some(s) })
        },
        sort_order: if clears.contains(&"sort_order") { Some(None) } else { None },
        team_id: None,
    };
    let issue = trakkt_auth::issue_service::update_issue(ac.db(), &ac.ws_id, &team_key, number, &updates, Some(&ac.auth.user_id), ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;

    // Handle parent relation changes via relation_service.
    if clears.contains(&"parent") {
        trakkt_auth::relation_service::clear_parent(
            ac.db(),
            &ac.ws_id,
            &issue.issue_id,
            ac.ctx.ws_manager.as_ref(),
        )
        .await
        .into_sfn()?;
    } else if let Some(ref new_parent_id) = parent_issue_id
        && !new_parent_id.is_empty()
    {
        trakkt_auth::relation_service::set_parent(
            ac.db(),
            &ac.ws_id,
            &issue.issue_id,
            new_parent_id,
            Some(&ac.auth.user_id),
            ac.ctx.ws_manager.as_ref(),
        )
        .await
        .into_sfn()?;
    }

    Ok(issue)
}

/// Delete an issue by its team key + number (e.g. "ENG-42").
#[server(prefix = "/leptos-api")]
pub async fn delete_issue(team_key: String, number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::issue_service::delete_issue(ac.db(), &ac.ws_id, &team_key, number, ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;
    Ok(())
}

/// Set the sort order for an issue (board drag-to-reorder).
#[server(prefix = "/leptos-api")]
pub async fn set_issue_sort_order(
    team_key: String,
    issue_number: i32,
    sort_order: f64,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::issue_service::set_sort_order(
        ac.db(),
        &ac.ws_id,
        &team_key,
        issue_number,
        sort_order,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Replace all labels on an issue.
///
/// `label_ids` is a comma-separated string of label UUIDs.
#[server(prefix = "/leptos-api")]
pub async fn set_issue_labels(
    team_key: String,
    number: i32,
    label_ids: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Resolve the team-scoped identifier to an issue_id.
    let issue_id = resolve_issue_id(ac.db(), &ac.ws_id, &team_key, number).await?;

    let parsed_label_ids = parse_label_ids(&label_ids);

    trakkt_auth::issue_service::set_issue_labels(ac.db(), &issue_id, &parsed_label_ids, ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;
    Ok(())
}
