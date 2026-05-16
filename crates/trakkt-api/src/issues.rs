// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::{comment_service, issue_service, relation_service, team_service};
use trakkt_types::api::{
    CreateIssueApiParams, DeleteIssueApiParams, GetIssueApiParams, ListIssuesApiParams,
    SearchIssuesApiParams, UpdateIssueApiParams,
};
use trakkt_types::models::{CreateIssueParams, IssueFilters, IssueUpdate};

use crate::context::{resolve_issue_key_and_number, resolve_team};
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List issues with optional filters.
///
/// Ported from `tool_list_issues` in `routes/mcp.rs`.
pub async fn list_issues(
    ctx: &ApiCtx<'_>,
    params: ListIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let limit_raw = params.limit.unwrap_or(50);
    let limit = limit_raw.clamp(1, 100);

    let status_id = params.status_id;
    let status_categories: Option<Vec<String>> = params.status_category.map(|s| {
        s.split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect()
    });
    let include_closed = params.include_closed.unwrap_or(false);

    let exclude_status_categories =
        if status_id.is_none() && status_categories.is_none() && !include_closed {
            Some(vec!["completed".to_string(), "cancelled".to_string()])
        } else {
            None
        };

    let filters = IssueFilters {
        status_id,
        status_categories,
        exclude_status_categories,
        priority: params.priority,
        assignee_id: params.assignee,
        creator_id: None,
        label_ids: params.label.map(|s| {
            s.split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect()
        }),
        search: params.search,
        limit: Some(limit),
        offset: None,
        include_archived: None,
        only_archived: None,
    };

    let team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?;

    let issues =
        issue_service::list_issues(ctx.db, &ctx.workspace_id, team_id.as_deref(), &filters)
            .await?;

    Ok(serde_json::to_value(&issues)?)
}

/// Get a single issue with details and comments.
///
/// Ported from `tool_get_issue` in `routes/mcp.rs`.
pub async fn get_issue(
    ctx: &ApiCtx<'_>,
    params: GetIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("issue {team_key}-{number} not found")))?;

    let comments = comment_service::list_comments(ctx.db, &issue.issue_id).await?;

    let result = json!({
        "issue": issue,
        "comments": comments
    });
    Ok(result)
}

/// Create a new issue in the specified or default team.
///
/// Ported from `tool_create_issue` in `routes/mcp.rs`.
pub async fn create_issue(
    ctx: &ApiCtx<'_>,
    params: CreateIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let resolved_team_id = match resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?
    {
        Some(id) => id,
        None => {
            let default_team =
                team_service::get_default_team(ctx.db, &ctx.workspace_id).await?;
            default_team.team_id
        }
    };

    let label_ids: Vec<String> = params.labels.unwrap_or_default();

    let parent_issue_id = params.parent_issue_id;

    let create_params = CreateIssueParams {
        workspace_id: ctx.workspace_id.clone(),
        team_id: resolved_team_id,
        creator_id: ctx.user_id.clone(),
        title: params.title,
        description: params.description,
        priority: params.priority.unwrap_or(0),
        assignee_id: params.assignee,
        due_date: params.due_date,
        label_ids,
        project_id: params.project_id,
        milestone_id: params.milestone_id,
    };

    let issue =
        issue_service::create_issue(ctx.db, &create_params, ctx.ws_manager).await?;

    // If a parent was specified, create the parent relation after issue creation.
    if let Some(ref parent_id) = parent_issue_id {
        relation_service::set_parent(
            ctx.db,
            &ctx.workspace_id,
            &issue.issue_id,
            parent_id,
            Some(&ctx.user_id),
            ctx.ws_manager,
        )
        .await?;
    }

    Ok(serde_json::to_value(&issue)?)
}

/// Update fields on an existing issue.
///
/// Ported from `tool_update_issue` in `routes/mcp.rs`.
pub async fn update_issue(
    ctx: &ApiCtx<'_>,
    params: UpdateIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    // Resolve "move to team" separately from the identifying team_key.
    let move_team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.move_to_team_key.as_deref(),
        params.move_to_team_id.as_deref(),
    )
    .await?;

    // Build the IssueUpdate from provided fields. Absent keys mean "no change".
    // `Some(None)` means "clear the field" for double-Option fields.
    let updates = IssueUpdate {
        title: params.title,
        description: params.description,
        status_id: params.status_id,
        priority: params.priority,
        assignee_id: params.assignee,
        due_date: params.due_date,
        project_id: params.project_id,
        milestone_id: params.milestone_id,
        sort_order: params.sort_order,
        team_id: move_team_id,
    };

    let issue = issue_service::update_issue(
        ctx.db,
        &ctx.workspace_id,
        &team_key,
        number,
        &updates,
        Some(&ctx.user_id),
        ctx.ws_manager,
    )
    .await?;

    // Handle parent_issue_id changes via relation_service.
    if let Some(ref parent_opt) = params.parent_issue_id {
        match parent_opt {
            None => {
                // Explicitly set to null — clear the parent.
                relation_service::clear_parent(
                    ctx.db,
                    &ctx.workspace_id,
                    &issue.issue_id,
                    ctx.ws_manager,
                )
                .await?;
            }
            Some(parent_id) if parent_id.is_empty() => {
                // Empty string — also clear.
                relation_service::clear_parent(
                    ctx.db,
                    &ctx.workspace_id,
                    &issue.issue_id,
                    ctx.ws_manager,
                )
                .await?;
            }
            Some(parent_id) => {
                // Set to a new parent.
                relation_service::set_parent(
                    ctx.db,
                    &ctx.workspace_id,
                    &issue.issue_id,
                    parent_id,
                    Some(&ctx.user_id),
                    ctx.ws_manager,
                )
                .await?;
            }
        }
    }

    // If labels were provided, replace them on the issue.
    if let Some(ref label_ids) = params.labels {
        issue_service::set_issue_labels(
            ctx.db,
            &issue.issue_id,
            label_ids,
            ctx.ws_manager,
        )
        .await?;
    }

    // Re-fetch with full details after label update.
    // Use get_issue_by_id since the team/number may have changed on team reassignment.
    let updated = issue_service::get_issue_by_id(ctx.db, &issue.issue_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("issue {team_key}-{number} not found")))?;

    Ok(serde_json::to_value(&updated)?)
}

/// Permanently delete an issue.
///
/// Ported from `tool_delete_issue` in `routes/mcp.rs`.
pub async fn delete_issue(
    ctx: &ApiCtx<'_>,
    params: DeleteIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    issue_service::delete_issue(
        ctx.db,
        &ctx.workspace_id,
        &team_key,
        number,
        ctx.ws_manager,
    )
    .await?;

    Ok(json!({ "message": format!("Issue {team_key}-{number} deleted") }))
}

/// Search issues by title text.
///
/// Ported from `tool_search_issues` in `routes/mcp.rs`.
pub async fn search_issues(
    ctx: &ApiCtx<'_>,
    params: SearchIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let limit_raw = params.limit.unwrap_or(20);
    let limit = limit_raw.clamp(1, 100);
    let include_closed = params.include_closed.unwrap_or(false);

    let team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?;

    let exclude_status_categories = if !include_closed {
        Some(vec!["completed".to_string(), "cancelled".to_string()])
    } else {
        None
    };

    let filters = IssueFilters {
        search: Some(params.query),
        exclude_status_categories,
        limit: Some(limit),
        ..Default::default()
    };

    let issues =
        issue_service::list_issues(ctx.db, &ctx.workspace_id, team_id.as_deref(), &filters)
            .await?;

    Ok(serde_json::to_value(&issues)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all issue-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_issues",
            description: "List issues in the workspace with optional filters. Returns issues ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them.",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/issues",
            json_schema: || schemars::schema_for!(ListIssuesApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListIssuesApiParams = serde_json::from_value(value)?;
                    list_issues(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "search_issues",
            description: "Search for issues by text query. Matches against issue titles. Returns results ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them.",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/issues/search",
            json_schema: || schemars::schema_for!(SearchIssuesApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: SearchIssuesApiParams = serde_json::from_value(value)?;
                    search_issues(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "get_issue",
            description: "Get a single issue by its team-scoped identifier (e.g. 'TRA-35'), including full details (description, labels, assignee, creator) and all comments.",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/issues/{identifier}",
            json_schema: || schemars::schema_for!(GetIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: GetIssueApiParams = serde_json::from_value(value)?;
                    get_issue(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "create_issue",
            description: "Create a new issue in the workspace. Specify team_id or team_key to assign to a specific team, otherwise uses the default team. Starts in 'backlog' status.",
            scope: "issues:write",
            rest_method: Method::POST,
            rest_path: "/issues",
            json_schema: || schemars::schema_for!(CreateIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: CreateIssueApiParams = serde_json::from_value(value)?;
                    create_issue(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "update_issue",
            description: "Update an existing issue. Only provided fields are changed; omitted fields remain unchanged. Set a field to null to clear it.",
            scope: "issues:write",
            rest_method: Method::PATCH,
            rest_path: "/issues/{identifier}",
            json_schema: || schemars::schema_for!(UpdateIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: UpdateIssueApiParams = serde_json::from_value(value)?;
                    update_issue(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "delete_issue",
            description: "Delete an issue by its team-scoped identifier (e.g. 'TRA-35'). This permanently removes the issue and all associated comments and labels.",
            scope: "issues:write",
            rest_method: Method::DELETE,
            rest_path: "/issues/{identifier}",
            json_schema: || schemars::schema_for!(DeleteIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: DeleteIssueApiParams = serde_json::from_value(value)?;
                    delete_issue(&ctx, params).await
                })
            }),
        },
    ]
}
