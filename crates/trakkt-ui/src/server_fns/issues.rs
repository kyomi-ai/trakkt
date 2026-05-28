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
        label_ids: label_id.map(|s| s.split(',').map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect()),
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
    estimate: Option<i32>,
) -> Result<Issue, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let parsed_labels: Vec<String> = label_ids.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let params = trakkt_types::api::CreateIssueApiParams {
        title, team_key: None, team_id, description,
        priority: Some(priority),
        assignee: assignee_id,
        labels: if parsed_labels.is_empty() { None } else { Some(parsed_labels) },
        due_date, project_id, milestone_id,
        parent_issue_id: parent_issue_id.filter(|s| !s.is_empty()),
        estimate,
        relations: None,
    };
    let result = trakkt_api::issues::create_issue(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
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
    estimate: Option<i32>,
    clear_fields: Option<String>,
) -> Result<Issue, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();

    let clears: Vec<&str> = clear_fields.as_deref()
        .map(|s| s.split(',').map(str::trim).collect())
        .unwrap_or_default();

    fn sentinel_to_opt(val: Option<String>, clear: bool) -> Option<Option<String>> {
        if clear { Some(None) }
        else { val.map(|s| if s.is_empty() { None } else { Some(s) }) }
    }

    let estimate_param = if clears.contains(&"estimate") {
        Some(None)
    } else {
        estimate.map(Some)
    };

    let params = trakkt_types::api::UpdateIssueApiParams {
        issue_identifier: Some(format!("{team_key}-{number}")),
        team_key: None, issue_number: None,
        title,
        description: description.map(|s| if s.is_empty() { None } else { Some(s) }),
        status_id, priority,
        assignee: sentinel_to_opt(assignee_id, clears.contains(&"assignee")),
        labels: None,
        due_date: sentinel_to_opt(due_date, clears.contains(&"due_date")),
        move_to_team_key: None, move_to_team_id: None,
        project_id: sentinel_to_opt(project_id, clears.contains(&"project")),
        milestone_id: sentinel_to_opt(milestone_id, clears.contains(&"milestone")),
        parent_issue_id: if clears.contains(&"parent") {
            Some(None)
        } else {
            parent_issue_id.filter(|s| !s.is_empty()).map(Some)
        },
        estimate: estimate_param,
        sort_order: if clears.contains(&"sort_order") { Some(None) } else { None },
    };
    let result = trakkt_api::issues::update_issue(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// Delete an issue by its team key + number (e.g. "ENG-42").
#[server(prefix = "/leptos-api")]
pub async fn delete_issue(team_key: String, number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::DeleteIssueApiParams {
        issue_identifier: Some(format!("{team_key}-{number}")),
        team_key: None, issue_number: None,
    };
    trakkt_api::issues::delete_issue(&ctx, params).await.into_sfn()?;
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

/// Fetch archived issues for a given team (or all teams if team_id is empty).
///
/// Returns only issues where `archived_at IS NOT NULL`. Used by the "Show
/// archived" toggle to fetch issues that have been swept by the server-side
/// archiver and are no longer present in the client-side SyncStore.
#[server(prefix = "/leptos-api")]
pub async fn get_archived_issues(
    team_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<IssueWithDetails>, ServerFnError> {
    use trakkt_types::models::IssueFilters;

    let ac = AuthenticatedContext::extract().await?;
    let clamped_limit = limit.unwrap_or(100).clamp(1, 200);
    let team = if team_id.is_empty() { None } else { Some(team_id.as_str()) };

    if team.is_some() {
        // Team-scoped: return all archived issues for the team.
        let filters = IssueFilters {
            only_archived: Some(true),
            limit: Some(clamped_limit),
            offset,
            ..Default::default()
        };
        let issues = trakkt_auth::issue_service::list_issues(ac.db(), &ac.ws_id, team, &filters)
            .await
            .into_sfn()?;
        return Ok(issues);
    }

    // Cross-team (My Issues): fetch issues assigned to OR created by the user.
    let assigned_filters = IssueFilters {
        only_archived: Some(true),
        assignee_id: Some(ac.auth.user_id.clone()),
        limit: Some(clamped_limit),
        ..Default::default()
    };
    let mut issues = trakkt_auth::issue_service::list_issues(ac.db(), &ac.ws_id, None, &assigned_filters)
        .await
        .into_sfn()?;

    let creator_filters = IssueFilters {
        only_archived: Some(true),
        creator_id: Some(ac.auth.user_id.clone()),
        limit: Some(clamped_limit),
        ..Default::default()
    };
    let created = trakkt_auth::issue_service::list_issues(ac.db(), &ac.ws_id, None, &creator_filters)
        .await
        .into_sfn()?;

    let existing_ids: std::collections::HashSet<String> =
        issues.iter().map(|i| i.issue_id.clone()).collect();
    for issue in created {
        if !existing_ids.contains(&issue.issue_id) {
            issues.push(issue);
        }
    }

    Ok(issues)
}

/// Unarchive an issue by clearing its `archived_at` timestamp.
///
/// Uses `update_issue` with an empty update — the service layer always sets
/// `archived_at = NULL` on any update (see issue_service.rs ~line 635).
#[server(prefix = "/leptos-api")]
pub async fn unarchive_issue(team_key: String, number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::UpdateIssueApiParams {
        issue_identifier: Some(format!("{team_key}-{number}")),
        team_key: None,
        issue_number: None,
        title: None,
        description: None,
        status_id: None,
        priority: None,
        assignee: None,
        labels: None,
        due_date: None,
        move_to_team_key: None,
        move_to_team_id: None,
        project_id: None,
        milestone_id: None,
        parent_issue_id: None,
        estimate: None,
        sort_order: None,
    };
    trakkt_api::issues::update_issue(&ctx, params).await.into_sfn()?;
    Ok(())
}

// ─── Search ───────────────────────────────────────────────────────────────

/// DTO for search results passed between server and client.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchResultItem {
    pub issue_id: String,
    pub number: i64,
    pub team_key: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: i32,
    pub snippet: Option<String>,
    pub match_field: String,
    pub rank: f64,
}

/// Full-text search across issues and comments.
///
/// Delegates to `trakkt_auth::search_service::search` which uses tsvector/GIN
/// on Postgres and LIKE fallback on SQLite.
#[server(prefix = "/leptos-api")]
pub async fn search_issues(
    query: String,
    team_id: Option<String>,
    include_closed: Option<bool>,
    include_archived: Option<bool>,
    include_comments: Option<bool>,
    limit: Option<i64>,
) -> Result<Vec<SearchResultItem>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let params = trakkt_auth::search_service::SearchParams {
        query,
        workspace_id: ac.ws_id.clone(),
        team_id,
        include_archived: include_archived.unwrap_or(false),
        include_closed: include_closed.unwrap_or(false),
        include_comments: include_comments.unwrap_or(true),
        limit: limit.unwrap_or(50),
        offset: 0,
    };
    let results = trakkt_auth::search_service::search(ac.db(), &params)
        .await
        .into_sfn()?;
    Ok(results
        .into_iter()
        .map(|r| SearchResultItem {
            issue_id: r.issue_id,
            number: r.number,
            team_key: r.team_key,
            title: r.title,
            status_name: r.status_name,
            status_category: r.status_category,
            priority: r.priority,
            snippet: r.snippet,
            match_field: match r.match_field.as_str() {
                "issue" => "description".to_string(),
                other => other.to_string(),
            },
            rank: r.rank,
        })
        .collect())
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
