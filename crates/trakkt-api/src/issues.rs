// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::activity_service::{ActivityRecorder, IssueSnapshot};
use trakkt_auth::{activity_service, comment_service, issue_service, relation_service, search_service, team_service};
use trakkt_types::api::{
    CreateIssueApiParams, DeleteIssueApiParams, FilterClause, GetIssueApiParams, InlineRelation,
    IssueListRow, ListIssuesApiParams, ListIssuesResponse, SearchIssuesApiParams,
    UpdateIssueApiParams,
};
use trakkt_types::models::{CreateIssueParams, IssueFilters, IssueUpdate, IssueWithDetails};

use crate::context::{parse_issue_identifier, resolve_issue_key_and_number, resolve_team};
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Composable filter clause support
// ─────────────────────────────────────────────────────────────────────────────

/// Multiplier applied to the requested limit when composable filter clauses
/// are present. Over-fetching from the DB compensates for rows eliminated by
/// post-fetch filtering, so the final page is likely full.
const FILTER_PREFETCH_MULTIPLIER: i64 = 5;

/// Known boolean filter fields that ignore `values` — the operator alone
/// determines the match.
// Must be kept in sync with ValueKind::Boolean fields in
// crates/trakkt-ui/src/pages/issues/filters.rs.
const BOOLEAN_FIELDS: &[&str] = &[
    "is_sub_issue",
    "is_parent",
    "is_blocked",
    "is_blocking",
    "has_relations",
];

/// Apply a single filter clause to an issue, returning `true` if the issue
/// passes the filter (should be included).
///
/// Ported from `crates/trakkt-ui/src/pages/issues/filters.rs` `apply_clause`.
fn apply_clause(clause: &FilterClause, issue: &IssueWithDetails) -> bool {
    if clause.values.is_empty() && !BOOLEAN_FIELDS.contains(&clause.field.as_str()) {
        // Non-boolean fields with no values selected — pass everything.
        return true;
    }

    match (clause.field.as_str(), clause.operator.as_str()) {
        ("status", "any_of") => clause.values.contains(&issue.status_id),
        ("status", "none_of") => !clause.values.contains(&issue.status_id),
        ("priority", "any_of") => clause.values.contains(&issue.priority.to_string()),
        ("priority", "none_of") => !clause.values.contains(&issue.priority.to_string()),
        // Label: all_of — issue must have ALL selected labels.
        ("label", "all_of") => clause
            .values
            .iter()
            .all(|v| issue.labels.iter().any(|l| l.label_id == *v)),
        // Label: any_of — issue has at least one of the selected labels.
        ("label", "any_of") => issue
            .labels
            .iter()
            .any(|l| clause.values.contains(&l.label_id)),
        // Label: not_any_of / none_of — issue has NONE of the selected labels.
        // "none_of" is a backward-compat alias from pre-TRA-104 persisted filters.
        ("label", "not_any_of" | "none_of") => !issue
            .labels
            .iter()
            .any(|l| clause.values.contains(&l.label_id)),
        // Label: not_all_of — issue does NOT have all values (may have some).
        ("label", "not_all_of") => !clause
            .values
            .iter()
            .all(|v| issue.labels.iter().any(|l| l.label_id == *v)),
        ("project", "any_of") => issue
            .project_id
            .as_ref()
            .is_some_and(|pid| clause.values.contains(pid)),
        ("project", "none_of") => !issue
            .project_id
            .as_ref()
            .is_some_and(|pid| clause.values.contains(pid)),
        // Boolean relation filters — values are ignored.
        ("is_sub_issue", "any_of") => issue.parent_identifier.is_some(),
        ("is_sub_issue", "none_of") => issue.parent_identifier.is_none(),
        ("is_parent", "any_of") => issue.has_children,
        ("is_parent", "none_of") => !issue.has_children,
        ("is_blocked", "any_of") => issue.is_blocked,
        ("is_blocked", "none_of") => !issue.is_blocked,
        ("is_blocking", "any_of") => issue.is_blocking,
        ("is_blocking", "none_of") => !issue.is_blocking,
        ("has_relations", "any_of") => issue.has_relations,
        ("has_relations", "none_of") => !issue.has_relations,
        // Unknown field/operator — pass through (don't block issues).
        _ => true,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List issues with optional filters.
///
/// Returns lean [`IssueListRow`]s, never full issue records: `description`,
/// comments, and activities are projected away at this boundary so that listing
/// a mature queue stays small. Callers that need an issue's content call
/// [`get_issue`]. The projection happens *after* [`apply_clause`] filtering, so
/// no filterable field is lost.
///
/// Ported from `tool_list_issues` in `routes/mcp.rs`.
pub async fn list_issues(
    ctx: &ApiCtx<'_>,
    params: ListIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let limit_raw = params.limit.unwrap_or(50);
    let limit = limit_raw.clamp(1, 100);

    // Parse composable filter clauses from the JSON string, if provided.
    let clauses: Vec<FilterClause> = match params.filters.as_deref() {
        Some(s) if !s.is_empty() => serde_json::from_str(s).map_err(|e| {
            ApiError::BadRequest(format!("Invalid filters JSON: {e}"))
        })?,
        _ => Vec::new(),
    };
    let has_clauses = !clauses.is_empty();

    // When post-fetch filter clauses are present, fetch more rows from the DB
    // so that after filtering we still have enough results. Use a multiplier.
    let db_limit = if has_clauses {
        limit * FILTER_PREFETCH_MULTIPLIER
    } else {
        limit
    };

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
        limit: Some(db_limit),
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

    let mut issues =
        issue_service::list_issues(ctx.db, &ctx.workspace_id, team_id.as_deref(), &filters)
            .await?;

    if has_clauses {
        // Apply composable filter clauses post-fetch (all AND-ed).
        issues.retain(|issue| clauses.iter().all(|clause| apply_clause(clause, issue)));

        let total_matched = issues.len();
        let limit_usize = limit as usize;
        let truncated = total_matched > limit_usize;
        issues.truncate(limit_usize);

        // Project to lean rows only after filtering, so clauses keep access to
        // every field of the full issue.
        let rows: Vec<IssueListRow> = issues.into_iter().map(IssueListRow::from).collect();
        let returned_count = rows.len();

        let response = ListIssuesResponse {
            issues: rows,
            matched_count: total_matched,
            returned_count,
            truncated,
        };
        Ok(serde_json::to_value(&response)?)
    } else {
        // No composable clauses — return the flat array for backward compat.
        let rows: Vec<IssueListRow> = issues.into_iter().map(IssueListRow::from).collect();
        Ok(serde_json::to_value(&rows)?)
    }
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
    let activities = activity_service::list_issue_activities(ctx.db, &issue.issue_id).await?;
    let relations = relation_service::list_relations_for_issue(ctx.db, &issue.issue_id, &ctx.workspace_id).await?;

    let mut issue_value = serde_json::to_value(&issue)?;
    if let serde_json::Value::Object(ref mut map) = issue_value {
        map.insert("key".to_string(), serde_json::Value::String(format!("{}-{}", issue.team_key, issue.number)));
    }

    let result = json!({
        "issue": issue_value,
        "comments": comments,
        "activities": activities,
        "relations": relations
    });
    Ok(result)
}

/// Map directional sugar to canonical relation types.
///
/// Returns (source_issue_id, target_issue_id, canonical_relation_type).
/// "blocked_by" is sugar for "blocks" with swapped direction.
fn normalize_relation_direction<'a>(
    new_issue_id: &'a str,
    other_issue_id: &'a str,
    relation_type: &str,
) -> (&'a str, &'a str, String) {
    match relation_type {
        "blocked_by" => (other_issue_id, new_issue_id, "blocks".to_string()),
        other => (new_issue_id, other_issue_id, other.to_string()),
    }
}

/// Resolve and create a single inline relation for a newly-created issue.
async fn process_inline_relation(
    ctx: &ApiCtx<'_>,
    new_issue_id: &str,
    inline_rel: &InlineRelation,
) -> Result<serde_json::Value, String> {
    let (target_key, target_num) =
        parse_issue_identifier(&inline_rel.issue).ok_or_else(|| {
            format!("Invalid issue identifier format: {}", inline_rel.issue)
        })?;

    let target_issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &target_key, target_num)
        .await
        .map_err(|e| format!("{e}"))?
        .ok_or_else(|| format!("Issue {} not found", inline_rel.issue))?;

    let (source_id, target_id, relation_type) =
        normalize_relation_direction(new_issue_id, &target_issue.issue_id, &inline_rel.relation_type);

    let relation = relation_service::create_relation(
        ctx.db,
        &ctx.workspace_id,
        source_id,
        target_id,
        &relation_type,
        Some(&ctx.user_id),
        ctx.action_source,
        ctx.action_source_label.as_deref(),
        ctx.ws_manager,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    serde_json::to_value(&relation).map_err(|e| format!("{e}"))
}

/// Create a new issue in the specified or default team.
///
/// Ported from `tool_create_issue` in `routes/mcp.rs`.
pub async fn create_issue(
    ctx: &ApiCtx<'_>,
    params: CreateIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let (resolved_team_id, resolved_team_key) = if let Some(ref key) = params.team_key {
        let team = team_service::get_team_by_key(ctx.db, &ctx.workspace_id, key)
            .await?
            .ok_or_else(|| ApiError::BadRequest(format!("No team found with key '{key}'")))?;
        (team.team_id, team.key)
    } else if let Some(ref id) = params.team_id {
        let team = team_service::get_team(ctx.db, id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("team {id} not found")))?;
        (team.team_id, team.key)
    } else {
        let default_team =
            team_service::get_default_team(ctx.db, &ctx.workspace_id).await?;
        (default_team.team_id, default_team.key)
    };

    let label_ids: Vec<String> = params.labels.unwrap_or_default();

    let parent_issue_id = params.parent_issue_id;
    let inline_relations = params.relations;

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
        estimate: params.estimate,
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
            ctx.action_source,
            ctx.action_source_label.as_deref(),
            ctx.ws_manager,
        )
        .await?;
    }

    // Process inline relations — failures are warnings, not errors.
    let mut relations_created = Vec::new();
    let mut relation_warnings = Vec::new();

    if let Some(ref inline_rels) = inline_relations {
        for inline_rel in inline_rels {
            match process_inline_relation(ctx, &issue.issue_id, inline_rel).await {
                Ok(val) => relations_created.push(val),
                Err(warning) => relation_warnings.push(format!(
                    "{} relation with {}: {}",
                    inline_rel.relation_type, inline_rel.issue, warning
                )),
            }
        }
    }

    // Record activity — never fails the mutation.
    let recorder = ActivityRecorder::new(ctx.db, &ctx.workspace_id, &ctx.user_id, ctx.action_source, ctx.action_source_label.clone(), ctx.ws_manager);
    if let Err(e) = recorder.record(&issue.issue_id, "created", None).await {
        tracing::warn!(issue_id = %issue.issue_id, "Failed to record create activity: {e}");
    }

    let mut response = serde_json::to_value(&issue)?;
    if let serde_json::Value::Object(ref mut map) = response {
        map.insert("key".to_string(), serde_json::Value::String(format!("{}-{}", resolved_team_key, issue.number)));
        if !relations_created.is_empty() {
            map.insert("relations_created".to_string(), serde_json::Value::Array(relations_created));
        }
        if !relation_warnings.is_empty() {
            map.insert("relation_warnings".to_string(), serde_json::to_value(&relation_warnings)?);
        }
    }
    Ok(response)
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

    // Snapshot before update — used to diff changes for activity recording.
    let before_issue =
        issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number).await?;
    let before_snapshot = before_issue.as_ref().map(IssueSnapshot::from_issue_with_details);

    // Track whether a status_id change was requested (before the value is moved).
    let status_change_requested = params.status_id.is_some();

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
        estimate: params.estimate,
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
        ctx.action_source,
        ctx.action_source_label.as_deref(),
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
                    ctx.action_source,
                    ctx.action_source_label.as_deref(),
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
            Some(&ctx.user_id),
            ctx.action_source,
            ctx.action_source_label.as_deref(),
            ctx.ws_manager,
        )
        .await?;
    }

    // Re-fetch with full details after label update.
    // Use get_issue_by_id since the team/number may have changed on team reassignment.
    let updated = issue_service::get_issue_by_id(ctx.db, &issue.issue_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("issue {team_key}-{number} not found")))?;

    // Record activity diff — never fails the mutation.
    if let Some(ref before) = before_snapshot {
        let after = IssueSnapshot::from_issue_with_details(&updated);
        let recorder =
            ActivityRecorder::new(ctx.db, &ctx.workspace_id, &ctx.user_id, ctx.action_source, ctx.action_source_label.clone(), ctx.ws_manager);
        if let Err(e) = recorder.record_issue_diff(&issue.issue_id, before, &after).await {
            tracing::warn!(issue_id = %issue.issue_id, "Failed to record activity diff: {e}");
        }
    }

    // Outbound GitHub notification: when status moves to "completed" category,
    // post a comment on linked PRs (best-effort — never fails the update).
    if status_change_requested
        && updated.status_category == "completed"
        && let Some(github_client) = ctx.github_client
        && let Some(encryption_key) = ctx.encryption_key
        && let Err(e) = trakkt_github::transitions::notify_github_links_on_completion(
            ctx.db,
            github_client,
            encryption_key,
            &updated.issue_id,
            &updated.team_key,
            updated.number,
            &updated.title,
            &updated.status_name,
            ctx.frontend_url,
        )
        .await
    {
        tracing::warn!(
            issue_id = %updated.issue_id,
            error = %e,
            "Failed to notify GitHub on issue completion"
        );
    }

    let mut response = serde_json::to_value(&updated)?;
    if let serde_json::Value::Object(ref mut map) = response {
        map.insert("key".to_string(), serde_json::Value::String(format!("{}-{}", updated.team_key, updated.number)));
    }
    Ok(response)
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

/// Search issues by text query using full-text search.
///
/// On Postgres, uses tsvector with GIN indexes for ranked results and
/// snippet context. On SQLite, falls back to LIKE matching.
pub async fn search_issues(
    ctx: &ApiCtx<'_>,
    params: SearchIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let limit_raw = params.limit.unwrap_or(20);
    let limit = limit_raw.clamp(1, 100);

    let team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?;

    let search_params = search_service::SearchParams {
        query: params.query,
        workspace_id: ctx.workspace_id.clone(),
        team_id,
        include_archived: params.include_archived.unwrap_or(false),
        include_closed: params.include_closed.unwrap_or(false),
        include_comments: params.include_comments.unwrap_or(true),
        limit,
        offset: params.offset.unwrap_or(0).max(0),
    };

    let response = search_service::search(ctx.db, &search_params).await?;
    Ok(serde_json::to_value(&response)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all issue-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_issues",
            description: "Find issues in the workspace with optional filters. Returns issues ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them. Supports a `filters` parameter: a JSON array of `{field, operator, values}` clauses AND-ed together. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations. Operators: any_of, none_of, all_of, not_any_of, not_all_of. Response shape: `{issues, matched_count, returned_count, truncated}`. Each row is lean by design — `number`, `key` (e.g. 'TRA-35'), `title`, `priority`, `status_id`, `status_name`, `updated_at`, and `labels` (id and name only) — enough to find, sort, and triage. Rows never include the issue description, comments, or activities, and there is no option to add them: descriptions are multi-KB and would dominate the payload. To read a ticket, call get_issue with the row's `key`.",
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
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "search_issues",
            description: "Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns `{results, total}` where results are ranked by relevance with snippet context, and total is the full match count for pagination. Supports `offset` for pagination. By default searches comments too — pass include_comments=false to search only titles and descriptions. By default excludes archived issues — pass include_archived=true to include them.",
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
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "get_issue",
            description: "Read a single issue in full by its team-scoped identifier (e.g. 'TRA-35'). This is the way to get an issue's content: it returns the complete record (description, labels, assignee, creator), all comments, the activity log, and relations. list_issues returns lean rows without descriptions, so the normal pattern is to list first, then call get_issue for each ticket you actually need to read.",
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
            binary_input: None,
            binary_output: None,
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
            binary_input: None,
            binary_output: None,
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
            binary_input: None,
            binary_output: None,
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
            binary_input: None,
            binary_output: None,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trakkt_core::DbPool;

    /// A description the size of a real Trakkt spec ticket (~4 KB).
    fn spec_sized_description() -> String {
        "## Context\nThis ticket describes, at length, exactly what has to change and why. "
            .repeat(50)
    }

    /// An in-memory workspace with `count` spec-sized issues on the default
    /// team, plus one label applied to the first issue.
    ///
    /// Returns the pool and the workspace/user ids an [`ApiCtx`] needs.
    async fn seeded_workspace(count: i32) -> (DbPool, String, String) {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");

        let user = trakkt_auth::user_service::create_user(
            &db,
            "lister@example.test",
            Some("Lister"),
            true,
        )
        .await
        .expect("create user");

        let workspace_id = trakkt_auth::user_service::create_workspace_for_user(
            &db,
            &user.user_id,
            Some("Lister"),
            "lister@example.test",
            None,
        )
        .await
        .expect("create workspace");

        let team = trakkt_auth::team_service::get_default_team(&db, &workspace_id)
            .await
            .expect("default team");

        let label = trakkt_auth::label_service::create_label(
            &db,
            &workspace_id,
            "agent-ready",
            "#0D9488",
            Some(&team.team_id),
            None,
        )
        .await
        .expect("create label");

        let description = spec_sized_description();
        for n in 1..=count {
            let params = CreateIssueParams {
                workspace_id: workspace_id.clone(),
                team_id: team.team_id.clone(),
                creator_id: user.user_id.clone(),
                title: format!("Seeded issue {n}"),
                description: Some(description.clone()),
                priority: 2,
                assignee_id: None,
                due_date: None,
                label_ids: vec![label.label_id.clone()],
                project_id: None,
                milestone_id: None,
                estimate: None,
            };
            issue_service::create_issue(&db, &params, None)
                .await
                .expect("create issue");
        }

        (db, workspace_id, user.user_id)
    }

    fn list_params(filters: Option<&str>) -> ListIssuesApiParams {
        ListIssuesApiParams {
            team_key: None,
            team_id: None,
            status_id: None,
            status_category: None,
            include_closed: None,
            priority: None,
            assignee: None,
            label: None,
            search: None,
            limit: Some(50),
            filters: filters.map(str::to_string),
        }
    }

    /// Assert every row in `rows` is a lean row: no heavy fields, a `TEAM-123`
    /// key, and labels reduced to id and name.
    fn assert_rows_are_lean(rows: &[serde_json::Value]) {
        assert!(!rows.is_empty(), "fixture should return rows");
        for row in rows {
            let object = row.as_object().expect("row should be a JSON object");
            for forbidden in ["description", "comments", "activities"] {
                assert!(
                    !object.contains_key(forbidden),
                    "list_issues row must never carry `{forbidden}`; got keys {:?}",
                    object.keys().collect::<Vec<_>>()
                );
            }
            let key = row["key"].as_str().expect("row should carry a key");
            assert_eq!(
                key,
                format!("TRK-{}", row["number"].as_i64().expect("row number")),
                "key should be TEAM-NUMBER"
            );
            for label in row["labels"].as_array().expect("labels array") {
                let label_object = label.as_object().expect("label should be an object");
                assert_eq!(
                    label_object.keys().map(String::as_str).collect::<Vec<_>>(),
                    vec!["label_id", "name"],
                    "labels should carry id and name only"
                );
            }
        }
    }

    /// The no-clauses path returns a flat array of lean rows.
    #[tokio::test]
    async fn flat_array_path_returns_lean_rows() {
        let (db, workspace_id, user_id) = seeded_workspace(3).await;
        let ctx = ApiCtx::from_leptos(
            workspace_id,
            user_id,
            &db,
            None,
            None,
            None,
            "http://localhost:3100",
        );

        let value = list_issues(&ctx, list_params(None))
            .await
            .expect("list_issues should succeed");

        let rows = value.as_array().expect("no-clauses path returns an array");
        assert_eq!(rows.len(), 3);
        assert_rows_are_lean(rows);
    }

    /// The envelope path (filter clauses present) returns lean rows too, and
    /// filtering still sees the full issue — the clause below matches on a
    /// field that lean rows do not expose.
    #[tokio::test]
    async fn envelope_path_returns_lean_rows_and_still_filters_on_full_issue() {
        let (db, workspace_id, user_id) = seeded_workspace(3).await;
        let ctx = ApiCtx::from_leptos(
            workspace_id,
            user_id,
            &db,
            None,
            None,
            None,
            "http://localhost:3100",
        );

        let value = list_issues(
            &ctx,
            list_params(Some(
                r#"[{"field":"is_sub_issue","operator":"none_of","values":[]}]"#,
            )),
        )
        .await
        .expect("list_issues should succeed");

        assert_eq!(value["matched_count"], 3);
        assert_eq!(value["returned_count"], 3);
        assert_eq!(value["truncated"], false);
        let rows = value["issues"].as_array().expect("envelope carries issues");
        assert_eq!(rows.len(), 3);
        assert_rows_are_lean(rows);
    }

    /// TRA-9915: a full page of spec-sized issues used to serialize to ~226 KB
    /// and overflow the caller's tool-result cap.
    ///
    /// Measured on this fixture: 249,535 bytes for the full issues the service
    /// returns vs 13,126 bytes for the lean response. The 25 KB ceiling leaves
    /// room for longer titles and more labels while still failing loudly if a
    /// description-sized field ever creeps back onto a row.
    #[tokio::test]
    async fn full_page_response_stays_small() {
        const PAGE_SIZE: i32 = 48;
        const MAX_RESPONSE_BYTES: usize = 25_000;

        let (db, workspace_id, user_id) = seeded_workspace(PAGE_SIZE).await;
        let ctx = ApiCtx::from_leptos(
            workspace_id.clone(),
            user_id.clone(),
            &db,
            None,
            None,
            None,
            "http://localhost:3100",
        );

        let value = list_issues(&ctx, list_params(None))
            .await
            .expect("list_issues should succeed");
        let response_bytes = serde_json::to_string(&value)
            .expect("response should serialize")
            .len();

        // What the same page used to cost, straight off the service layer.
        let full_issues = issue_service::list_issues(
            &db,
            &workspace_id,
            None,
            &IssueFilters {
                status_id: None,
                status_categories: None,
                exclude_status_categories: Some(vec![
                    "completed".to_string(),
                    "cancelled".to_string(),
                ]),
                priority: None,
                assignee_id: None,
                creator_id: None,
                label_ids: None,
                search: None,
                limit: Some(50),
                offset: None,
                include_archived: None,
                only_archived: None,
            },
        )
        .await
        .expect("service list should succeed");
        let full_bytes = serde_json::to_string(&full_issues)
            .expect("full issues should serialize")
            .len();

        assert_eq!(value.as_array().expect("array").len(), PAGE_SIZE as usize);
        assert!(
            response_bytes < MAX_RESPONSE_BYTES,
            "a {PAGE_SIZE}-issue page must serialize under {MAX_RESPONSE_BYTES} bytes; \
             got {response_bytes} bytes lean vs {full_bytes} bytes for the full issues"
        );
    }
}
