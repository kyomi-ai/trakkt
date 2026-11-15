// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::activity_service::{ActivityRecorder, IssueSnapshot};
use trakkt_auth::issue_service::{IssueSort, IssueSortField, SortDir};
use trakkt_auth::{
    activity_service, comment_service, issue_service, relation_service, search_service,
    team_service,
};
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
    let limit_raw = params.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    let limit = limit_raw.clamp(1, MAX_LIST_LIMIT);

    let sort_field = match params.sort_by.as_deref() {
        None | Some("") => IssueSortField::Priority,
        Some(s) => IssueSortField::parse(s).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Invalid sort_by '{s}': expected priority, created_at, updated_at or number"
            ))
        })?,
    };
    let sort_dir = match params.sort_dir.as_deref() {
        None | Some("") => sort_field.default_dir(),
        Some(s) => SortDir::parse(s).ok_or_else(|| {
            ApiError::BadRequest(format!("Invalid sort_dir '{s}': expected asc or desc"))
        })?,
    };
    let sort = IssueSort {
        field: sort_field,
        dir: sort_dir,
    };

    // Parse composable filter clauses from the JSON string, if provided.
    let clauses: Vec<FilterClause> = match params.filters.as_deref() {
        Some(s) if !s.is_empty() => serde_json::from_str(s)
            .map_err(|e| ApiError::BadRequest(format!("Invalid filters JSON: {e}")))?,
        _ => Vec::new(),
    };

    // A cursor is only valid for the query that produced it. Everything that
    // selects or orders rows goes into the fingerprint.
    let fingerprint = [
        params.team_key.as_deref().unwrap_or(""),
        params.team_id.as_deref().unwrap_or(""),
        params.status_id.as_deref().unwrap_or(""),
        params.status_category.as_deref().unwrap_or(""),
        &params.include_closed.unwrap_or(false).to_string(),
        &params.priority.map(|p| p.to_string()).unwrap_or_default(),
        params.assignee.as_deref().unwrap_or(""),
        params.label.as_deref().unwrap_or(""),
        params.search.as_deref().unwrap_or(""),
        params.filters.as_deref().unwrap_or(""),
        sort.field.as_str(),
        sort.dir.as_str(),
    ]
    .join("\u{1f}");
    let cursor = params
        .cursor
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(|c| decode_cursor(c, &fingerprint))
        .transpose()?;

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
        // Paging happens below, after the composable clauses, so that `total`
        // and the cursor see the complete filtered result.
        limit: None,
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

    let mut issues = issue_service::list_issues_sorted(
        ctx.db,
        &ctx.workspace_id,
        team_id.as_deref(),
        &filters,
        sort,
    )
    .await?;

    // Apply composable filter clauses post-fetch (all AND-ed).
    if !clauses.is_empty() {
        issues.retain(|issue| clauses.iter().all(|clause| apply_clause(clause, issue)));
    }

    let total = issues.len();
    // Resume after the last row the caller saw. If that issue is gone or no
    // longer matches, fall back to the position the cursor recorded.
    let start = match &cursor {
        None => 0,
        Some(c) => issues
            .iter()
            .position(|i| i.issue_id == c.anchor)
            .map_or(c.offset.min(total), |p| p + 1),
    };
    let end = start.saturating_add(limit as usize).min(total);
    let has_more = end < total;
    // `has_more` implies `start < end < total` (limit >= 1), so `end - 1` is the
    // last row of this page.
    let next_cursor = if has_more {
        issues
            .get(end - 1)
            .map(|last| encode_cursor(end, &last.issue_id, &fingerprint))
    } else {
        None
    };

    // Project to lean rows only after filtering, so clauses keep access to
    // every field of the full issue.
    let rows: Vec<IssueListRow> = issues
        .drain(start..end)
        .map(IssueListRow::from)
        .collect();

    let response = ListIssuesResponse {
        returned_count: rows.len(),
        issues: rows,
        total,
        has_more,
        next_cursor,
        limit,
        sort_by: sort.field.as_str().to_string(),
        sort_dir: sort.dir.as_str().to_string(),
        matched_count: total,
        truncated: has_more,
    };
    Ok(serde_json::to_value(&response)?)
}

/// Default and maximum page size for `list_issues`.
const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 100;

/// What an opaque `list_issues` cursor carries.
struct ListCursor {
    /// Number of rows already returned.
    offset: usize,
    /// `issue_id` of the last row returned.
    anchor: String,
}

const CURSOR_VERSION: &str = "v1";
const CURSOR_SEP: char = '\u{1e}';

fn encode_cursor(offset: usize, anchor: &str, fingerprint: &str) -> String {
    use base64::Engine;
    let raw = format!("{CURSOR_VERSION}{CURSOR_SEP}{offset}{CURSOR_SEP}{anchor}{CURSOR_SEP}{fingerprint}");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

/// Decode a cursor, rejecting anything not produced by [`encode_cursor`] for
/// this same query.
fn decode_cursor(cursor: &str, fingerprint: &str) -> ApiResult<ListCursor> {
    use base64::Engine;
    let bad = || {
        ApiError::BadRequest(
            "Invalid cursor: pass next_cursor from a previous response to the same query unchanged"
                .to_string(),
        )
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| bad())?;
    let text = String::from_utf8(bytes).map_err(|_| bad())?;
    let mut parts = text.splitn(4, CURSOR_SEP);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(CURSOR_VERSION), Some(offset), Some(anchor), Some(fp)) if fp == fingerprint => {
            Ok(ListCursor {
                offset: offset.parse().map_err(|_| bad())?,
                anchor: anchor.to_string(),
            })
        }
        _ => Err(bad()),
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
    let relations =
        relation_service::list_relations_for_issue(ctx.db, &issue.issue_id, &ctx.workspace_id)
            .await?;

    let mut issue_value = serde_json::to_value(&issue)?;
    if let serde_json::Value::Object(ref mut map) = issue_value {
        map.insert(
            "key".to_string(),
            serde_json::Value::String(format!("{}-{}", issue.team_key, issue.number)),
        );
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
    let (target_key, target_num) = parse_issue_identifier(&inline_rel.issue)
        .ok_or_else(|| format!("Invalid issue identifier format: {}", inline_rel.issue))?;

    let target_issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &target_key, target_num)
        .await
        .map_err(|e| format!("{e}"))?
        .ok_or_else(|| format!("Issue {} not found", inline_rel.issue))?;

    let (source_id, target_id, relation_type) = normalize_relation_direction(
        new_issue_id,
        &target_issue.issue_id,
        &inline_rel.relation_type,
    );

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
    create_issue_impl(ctx, params, false).await
}

/// MCP preserves an applied primary issue when the requested parent link fails.
pub async fn create_issue_with_outcomes(
    ctx: &ApiCtx<'_>,
    params: CreateIssueApiParams,
) -> ApiResult<serde_json::Value> {
    create_issue_impl(ctx, params, true).await
}

async fn create_issue_impl(
    ctx: &ApiCtx<'_>,
    params: CreateIssueApiParams,
    report_parent_failure: bool,
) -> ApiResult<serde_json::Value> {
    let (resolved_team_id, resolved_team_key) = if let Some(ref key) = params.team_key {
        let team = team_service::get_team_by_key(ctx.db, &ctx.workspace_id, key)
            .await?
            .ok_or_else(|| ApiError::BadRequest(format!("No team found with key '{key}'")))?;
        (team.team_id, team.key)
    } else if let Some(ref id) = params.team_id {
        // Scoped to the caller's workspace: an unscoped lookup would let a
        // caller file an issue against another workspace's team, and leak that
        // team's key back in the issue identifier.
        let team = team_service::get_team_in_workspace(ctx.db, id, &ctx.workspace_id).await?;
        (team.team_id, team.key)
    } else {
        let default_team = team_service::get_default_team(ctx.db, &ctx.workspace_id).await?;
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

    let issue = issue_service::create_issue(ctx.db, &create_params, ctx.ws_manager).await?;

    let mut relation_warnings = Vec::new();

    // If a parent was specified, create the parent relation after issue creation.
    if let Some(ref parent_id) = parent_issue_id {
        let result = relation_service::set_parent(
            ctx.db,
            &ctx.workspace_id,
            &issue.issue_id,
            parent_id,
            Some(&ctx.user_id),
            ctx.action_source,
            ctx.action_source_label.as_deref(),
            ctx.ws_manager,
        )
        .await;
        if let Err(error) = result {
            if !report_parent_failure {
                return Err(error.into());
            }
            tracing::warn!(issue_id = %issue.issue_id, %parent_id, %error, "Issue saved but parent link failed");
            relation_warnings.push("Issue saved; parent link failed. Retry the parent relation separately, not create_issue.".to_string());
        }
    }

    // Process inline relations — failures are warnings, not errors.
    let mut relations_created = Vec::new();

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
    let recorder = ActivityRecorder::new(
        ctx.db,
        &ctx.workspace_id,
        &ctx.user_id,
        ctx.action_source,
        ctx.action_source_label.clone(),
        ctx.ws_manager,
    );
    if let Err(e) = recorder.record(&issue.issue_id, "created", None).await {
        tracing::warn!(issue_id = %issue.issue_id, "Failed to record create activity: {e}");
    }

    let mut response = serde_json::to_value(&issue)?;
    if let serde_json::Value::Object(ref mut map) = response {
        map.insert(
            "key".to_string(),
            serde_json::Value::String(format!("{}-{}", resolved_team_key, issue.number)),
        );
        if !relations_created.is_empty() {
            map.insert(
                "relations_created".to_string(),
                serde_json::Value::Array(relations_created),
            );
        }
        if !relation_warnings.is_empty() {
            map.insert(
                "relation_warnings".to_string(),
                serde_json::to_value(&relation_warnings)?,
            );
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
    let before_snapshot = before_issue
        .as_ref()
        .map(IssueSnapshot::from_issue_with_details);

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
        let recorder = ActivityRecorder::new(
            ctx.db,
            &ctx.workspace_id,
            &ctx.user_id,
            ctx.action_source,
            ctx.action_source_label.clone(),
            ctx.ws_manager,
        );
        if let Err(e) = recorder
            .record_issue_diff(&issue.issue_id, before, &after)
            .await
        {
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
        map.insert(
            "key".to_string(),
            serde_json::Value::String(format!("{}-{}", updated.team_key, updated.number)),
        );
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

    issue_service::delete_issue(ctx.db, &ctx.workspace_id, &team_key, number, ctx.ws_manager)
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
            description: "Find issues in the workspace with optional filters. Returns `{issues, total, has_more, next_cursor, limit, sort_by, sort_dir, matched_count, returned_count, truncated}`. `total` is the exact number of matching issues ignoring `limit`; a response is complete only when `has_more` is false. Pagination: `limit` is the page size (default 50, max 100; out-of-range values are clamped and the applied value is echoed as `limit`). When `has_more` is true, call again with the same filters and `cursor` set to `next_cursor` (an opaque string; changing any filter or sort invalidates it) until `has_more` is false; each matching issue appears exactly once. Sorting: `sort_by` is priority (default), created_at, updated_at or number; `sort_dir` is asc or desc (default asc for priority, so Urgent (1), High (2), Medium (3), Low (4), None (0) last, newest first within a priority; default desc for created_at, updated_at and number). Ties are broken deterministically so pages are stable. By default, completed and cancelled issues are excluded — pass include_closed=true to include them. Supports a `filters` parameter: a JSON array of `{field, operator, values}` clauses AND-ed together. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations. Operators: any_of, none_of, all_of, not_any_of, not_all_of. `matched_count` equals `total` and `truncated` equals `has_more` (kept for older callers). Each row is lean by design — `number`, `key` (e.g. 'TRA-35'), `title`, `priority`, `priority_name` (Urgent, High, Medium, Low, or No priority), `status_id`, `status_name`, `created_at`, `updated_at`, and `labels` (id and name only) — enough to find, sort, and triage. Rows never include the issue description, comments, or activities, and there is no option to add them: descriptions are multi-KB and would dominate the payload. To read a ticket, call get_issue with the row's `key`.",
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
            description: "Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns `{results, total, has_more, next_offset, truncated}`; results are ranked by relevance with snippet context. `limit` is the page size (default 20, max 100, clamped). `total` is the number of matches ignoring limit/offset; when `has_more` is true, repeat the request with `offset` set to `next_offset` until `has_more` is false. A search considers at most its 1000 best-ranked matches: when `truncated` is true, `total` is a lower bound and lower-ranked matches cannot be reached by paging, so narrow the query (team, include_closed, include_comments). By default searches comments too — pass include_comments=false to search only titles and descriptions. By default excludes archived issues — pass include_archived=true to include them.",
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
            cursor: None,
            sort_by: None,
            sort_dir: None,
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

    /// The no-clauses path returns an envelope of lean rows.
    #[tokio::test]
    async fn unfiltered_path_returns_lean_rows() {
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

        let rows = value["issues"].as_array().expect("envelope carries issues");
        assert_eq!(rows.len(), 3);
        assert_eq!(value["total"], 3);
        assert_eq!(value["has_more"], false);
        assert!(value["next_cursor"].is_null());
        assert!(
            rows.iter().all(|r| r["created_at"].as_str().is_some_and(|c| !c.is_empty())),
            "every row carries created_at"
        );
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

    /// TRA-10096: stored priority zero must rank last before either response
    /// path applies its limit. Fixed timestamps also exercise newest-first ties.
    #[tokio::test]
    async fn priority_order_precedes_limit_for_unfiltered_and_filtered_responses() {
        let (db, workspace_id, user_id) = seeded_workspace(7).await;
        // Number 7 is the newest None; it must not displace assigned priorities.
        for (number, priority, created_at) in [
            (1, 1, "2026-07-01T00:00:00Z"),
            (2, 2, "2026-07-02T00:00:00Z"),
            (3, 3, "2026-07-03T00:00:00Z"),
            (4, 4, "2026-07-04T00:00:00Z"),
            (5, 0, "2026-07-05T00:00:00Z"),
            (6, 1, "2026-07-06T00:00:00Z"),
            (7, 0, "2026-07-07T00:00:00Z"),
        ] {
            trakkt_core::db_execute!(
                &db,
                "UPDATE issues SET priority = $1, created_at = $2 WHERE workspace_id = $3 AND number = $4",
                priority,
                created_at,
                &workspace_id,
                number
            )
            .expect("set fixture priority and creation time");
        }
        let ctx = ApiCtx::from_leptos(
            workspace_id,
            user_id,
            &db,
            None,
            None,
            None,
            "http://localhost:3100",
        );
        for clauses in [
            None,
            Some(r#"[{"field":"priority","operator":"any_of","values":["0","1","2","3","4"]}]"#),
        ] {
            for (limit, expected_numbers) in [(50, vec![6, 1, 2, 3, 4, 7, 5]), (3, vec![6, 1, 2])] {
                let mut params = list_params(clauses);
                params.limit = Some(limit);
                // Also exercise the SQL search filter without changing the matches.
                params.search = Some("Seeded issue".to_string());
                let value = list_issues(&ctx, params).await.expect("list ranked issues");
                let rows = value["issues"].as_array().expect("envelope rows");
                assert_eq!(
                    rows.iter()
                        .map(|row| row["number"].as_i64().expect("issue number"))
                        .collect::<Vec<_>>(),
                    expected_numbers
                );
                for row in rows {
                    let expected_name = match row["priority"].as_i64().expect("numeric priority") {
                        0 => "No priority",
                        1 => "Urgent",
                        2 => "High",
                        3 => "Medium",
                        4 => "Low",
                        other => panic!("unexpected priority {other}"),
                    };
                    assert_eq!(row["priority_name"], expected_name);
                }
            }
        }
    }

    /// TRA-10113 (reproduction): 260 matching issues, one request at the max
    /// limit. A capped response must say so — `total` and `has_more` — instead
    /// of looking like a complete list.
    #[tokio::test]
    async fn capped_page_reports_total_and_has_more() {
        let (db, workspace_id, user_id) = seeded_workspace(260).await;
        let ctx = ApiCtx::from_leptos(
            workspace_id,
            user_id,
            &db,
            None,
            None,
            None,
            "http://localhost:3100",
        );
        let mut params = list_params(None);
        params.limit = Some(100);
        let value = list_issues(&ctx, params).await.expect("list capped page");
        assert_eq!(value["total"], 260, "total must ignore the limit: {value}");
        assert_eq!(value["has_more"], true);
        assert_eq!(value["issues"].as_array().expect("issues").len(), 100);
    }

    fn test_ctx<'a>(db: &'a DbPool, workspace_id: &str, user_id: &str) -> ApiCtx<'a> {
        ApiCtx::from_leptos(
            workspace_id.to_string(),
            user_id.to_string(),
            db,
            None,
            None,
            None,
            "http://localhost:3100",
        )
    }

    /// Walk every page of `params`, returning the page count and every row in
    /// the order served. Asserts the paging contract on each page.
    async fn walk_pages(
        ctx: &ApiCtx<'_>,
        base: ListIssuesApiParams,
        expected_total: usize,
    ) -> (usize, Vec<serde_json::Value>) {
        let mut rows = Vec::new();
        let mut pages = 0;
        let mut cursor: Option<String> = None;
        loop {
            let mut params = base.clone();
            params.cursor = cursor.clone();
            let value = list_issues(ctx, params).await.expect("list a page");
            pages += 1;
            assert_eq!(value["total"], expected_total, "total on page {pages}");
            let page = value["issues"].as_array().expect("issues").clone();
            assert_eq!(value["returned_count"], page.len());
            rows.extend(page);
            if value["has_more"] == true {
                assert_eq!(value["truncated"], true);
                cursor = Some(
                    value["next_cursor"]
                        .as_str()
                        .expect("has_more implies next_cursor")
                        .to_string(),
                );
            } else {
                assert!(value["next_cursor"].is_null(), "no cursor on the last page");
                break;
            }
            assert!(pages < 50, "paging must terminate");
        }
        (pages, rows)
    }

    /// TRA-10113 acceptance: 260 issues, every sort, walking cursors returns
    /// each issue exactly once, in the requested order, with created_at.
    #[tokio::test]
    async fn walking_cursors_returns_every_issue_once_in_sort_order() {
        const COUNT: usize = 260;
        let (db, workspace_id, user_id) = seeded_workspace(COUNT as i32).await;
        // Spread priorities and timestamps; leave plenty of ties.
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET priority = number % 5, \
             created_at = '2026-07-01 00:00:' || substr('0' || ((number / 7) % 60), -2), \
             updated_at = '2026-08-01 00:00:' || substr('0' || ((number / 11) % 60), -2) \
             WHERE workspace_id = $1",
            &workspace_id
        )
        .expect("spread fixture priorities and timestamps");
        let ctx = test_ctx(&db, &workspace_id, &user_id);

        for (sort_by, sort_dir) in [
            (None, None),
            (Some("priority"), Some("desc")),
            (Some("created_at"), None),
            (Some("created_at"), Some("asc")),
            (Some("updated_at"), Some("desc")),
            (Some("number"), Some("asc")),
            (Some("number"), None),
        ] {
            let mut params = list_params(None);
            params.limit = Some(100);
            params.sort_by = sort_by.map(str::to_string);
            params.sort_dir = sort_dir.map(str::to_string);
            let (pages, rows) = walk_pages(&ctx, params, COUNT).await;
            assert_eq!(pages, 3, "260 rows at 100 per page ({sort_by:?} {sort_dir:?})");
            assert_eq!(rows.len(), COUNT);
            let mut numbers: Vec<i64> = rows
                .iter()
                .map(|r| r["number"].as_i64().expect("number"))
                .collect();
            numbers.sort_unstable();
            assert_eq!(numbers, (1..=COUNT as i64).collect::<Vec<_>>(), "each exactly once");
            assert!(rows.iter().all(|r| r["created_at"].as_str().is_some_and(|c| !c.is_empty())));

            let key = |r: &serde_json::Value| -> String {
                match sort_by {
                    Some("created_at") => r["created_at"].as_str().expect("created_at").to_string(),
                    Some("updated_at") => r["updated_at"].as_str().expect("updated_at").to_string(),
                    Some("number") => format!("{:08}", r["number"].as_i64().expect("number")),
                    _ => {
                        let p = r["priority"].as_i64().expect("priority");
                        format!("{}", if p == 0 { 5 } else { p })
                    }
                }
            };
            let descending = match (sort_by, sort_dir) {
                (_, Some("desc")) => true,
                (_, Some(_)) => false,
                (None | Some("priority"), None) => false,
                _ => true,
            };
            for pair in rows.windows(2) {
                let (a, b) = (key(&pair[0]), key(&pair[1]));
                assert!(
                    if descending { a >= b } else { a <= b },
                    "rows out of order for {sort_by:?} {sort_dir:?}: {a} then {b}"
                );
            }
        }

        // Ties are deterministic: two independent walks agree exactly.
        let mut params = list_params(None);
        params.limit = Some(100);
        params.sort_by = Some("created_at".to_string());
        let (_, first) = walk_pages(&ctx, params.clone(), COUNT).await;
        let (_, second) = walk_pages(&ctx, params, COUNT).await;
        assert_eq!(first, second);
    }

    /// Paging composes with filter clauses: `total` counts post-clause matches
    /// and no match is lost to the old 5x prefetch window.
    #[tokio::test]
    async fn pagination_composes_with_filter_clauses() {
        let (db, workspace_id, user_id) = seeded_workspace(260).await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET priority = CASE WHEN number <= 230 THEN 4 ELSE 2 END WHERE workspace_id = $1",
            &workspace_id
        )
        .expect("set priorities");
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(Some(
            r#"[{"field":"priority","operator":"any_of","values":["4"]}]"#,
        ));
        params.limit = Some(10);
        let (pages, rows) = walk_pages(&ctx, params, 230).await;
        assert_eq!(pages, 23);
        assert_eq!(rows.len(), 230);
    }

    #[tokio::test]
    async fn invalid_sort_and_cursor_are_rejected() {
        let (db, workspace_id, user_id) = seeded_workspace(5).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);

        for mutate in [
            (|p: &mut ListIssuesApiParams| p.sort_by = Some("title".to_string())) as fn(&mut _),
            |p| p.sort_dir = Some("sideways".to_string()),
            |p| p.cursor = Some("not-a-cursor".to_string()),
        ] {
            let mut params = list_params(None);
            mutate(&mut params);
            assert!(matches!(
                list_issues(&ctx, params).await,
                Err(ApiError::BadRequest(_))
            ));
        }

        // A cursor from one query is refused by a different query.
        let mut first = list_params(None);
        first.limit = Some(2);
        let page = list_issues(&ctx, first).await.expect("first page");
        let mut other = list_params(None);
        other.limit = Some(2);
        other.sort_by = Some("number".to_string());
        other.cursor = Some(page["next_cursor"].as_str().expect("cursor").to_string());
        assert!(matches!(
            list_issues(&ctx, other).await,
            Err(ApiError::BadRequest(_))
        ));
    }

    /// A cursor survives an insert and a delete between pages: no row is
    /// repeated and the rows not yet seen are still served.
    #[tokio::test]
    async fn cursor_resumes_after_anchor_when_rows_change_between_pages() {
        let (db, workspace_id, user_id) = seeded_workspace(10).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(4);
        params.sort_by = Some("number".to_string());
        params.sort_dir = Some("asc".to_string());
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        let seen: Vec<i64> = first["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .map(|r| r["number"].as_i64().expect("number"))
            .collect();
        assert_eq!(seen, vec![1, 2, 3, 4]);

        // Delete an already-served row: offset alone would skip number 5.
        trakkt_core::db_execute!(
            &db,
            "DELETE FROM issues WHERE workspace_id = $1 AND number = 1",
            &workspace_id
        )
        .expect("delete a served row");
        params.cursor = first["next_cursor"].as_str().map(str::to_string);
        let second = list_issues(&ctx, params).await.expect("page 2");
        let numbers: Vec<i64> = second["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .map(|r| r["number"].as_i64().expect("number"))
            .collect();
        assert_eq!(numbers, vec![5, 6, 7, 8]);
    }

    /// TRA-10113 audit: `search_issues` pages with has_more / next_offset and
    /// every match is reachable exactly once.
    #[tokio::test]
    async fn search_issues_reports_has_more_and_walks_every_match() {
        let (db, workspace_id, user_id) = seeded_workspace(35).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut offset = 0;
        let mut seen = std::collections::HashSet::new();
        let mut pages = 0;
        loop {
            let value = search_issues(
                &ctx,
                SearchIssuesApiParams {
                    query: "Seeded".to_string(),
                    team_key: None,
                    team_id: None,
                    include_closed: None,
                    include_archived: None,
                    include_comments: Some(false),
                    limit: Some(10),
                    offset: Some(offset),
                },
            )
            .await
            .expect("search a page");
            pages += 1;
            assert_eq!(value["total"], 35);
            assert_eq!(value["truncated"], false);
            for r in value["results"].as_array().expect("results") {
                assert!(seen.insert(r["issue_id"].as_str().expect("issue_id").to_string()));
            }
            if value["has_more"] == true {
                offset = value["next_offset"].as_i64().expect("next_offset when has_more");
            } else {
                assert!(value["next_offset"].is_null());
                break;
            }
        }
        assert_eq!((pages, seen.len()), (4, 35));
    }

    /// TRA-10113 audit: workspace activities answer has_more from the data,
    /// not from the page being full, and page without gaps or repeats.
    #[tokio::test]
    async fn workspace_activities_report_has_more_and_page_stably() {
        let (db, workspace_id, user_id) = seeded_workspace(1).await;
        let issue = issue_service::get_issue(&db, &workspace_id, "TRK", 1)
            .await
            .expect("load issue")
            .expect("seeded issue exists");
        let recorder = ActivityRecorder::new(
            &db,
            &workspace_id,
            &user_id,
            trakkt_types::enums::ActionSource::Api,
            None,
            None,
        );
        for _ in 0..25 {
            recorder
                .record(&issue.issue_id, "comment_added", None)
                .await
                .expect("record activity");
        }
        let ctx = test_ctx(&db, &workspace_id, &user_id);

        let mut offset = 0;
        let mut ids = Vec::new();
        loop {
            let value = crate::activities::list_workspace_activities(
                &ctx,
                trakkt_types::api::ListWorkspaceActivitiesApiParams {
                    team_key: None,
                    action_type: Some("comment_added".to_string()),
                    actor_id: None,
                    action_source: None,
                    created_after: None,
                    created_before: None,
                    limit: Some(10),
                    offset: Some(offset),
                },
            )
            .await
            .expect("list a page");
            for a in value["activities"].as_array().expect("activities") {
                ids.push(a["activity_id"].as_str().expect("activity_id").to_string());
            }
            if value["has_more"] == true {
                offset = value["next_offset"].as_i64().expect("next_offset");
            } else {
                // A last page that is exactly full must still say it is the end.
                assert!(value["next_offset"].is_null());
                break;
            }
        }
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!((ids.len(), unique.len()), (25, 25));
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

        assert_eq!(
            value["issues"].as_array().expect("issues").len(),
            PAGE_SIZE as usize
        );
        assert!(
            response_bytes < MAX_RESPONSE_BYTES,
            "a {PAGE_SIZE}-issue page must serialize under {MAX_RESPONSE_BYTES} bytes; \
             got {response_bytes} bytes lean vs {full_bytes} bytes for the full issues"
        );
    }
}
