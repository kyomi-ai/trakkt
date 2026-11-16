// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::activity_service::{ActivityRecorder, IssueSnapshot};
use trakkt_auth::issue_service::{IssueSort, IssueSortField, IssueSortKey, SortDir};
use trakkt_auth::{
    activity_service, comment_service, issue_service, relation_service, search_service,
    team_service,
};
use trakkt_types::api::{
    CreateIssueApiParams, DeleteIssueApiParams, FilterClause, GetIssueApiParams, InlineRelation,
    IssueListRow, ListIssuesApiParams, ListIssuesResponse, SearchIssuesApiParams,
    UpdateIssueApiParams,
};
use trakkt_types::models::{CreateIssueParams, IssueFilters, IssueUpdate};

use crate::context::{parse_issue_identifier, resolve_issue_key_and_number, resolve_team};
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List issues with optional filters.
///
/// Returns lean [`IssueListRow`]s, never full issue records: `description`,
/// comments, and activities are projected away at this boundary so that listing
/// a mature queue stays small. Callers that need an issue's content call
/// [`get_issue`].
///
/// Ported from `tool_list_issues` in `routes/mcp.rs`.
pub async fn list_issues(
    ctx: &ApiCtx<'_>,
    params: ListIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let limit = clamp_limit(params.limit, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT);

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
    let clauses = parse_filter_clauses(params.filters.as_deref())?;

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
    // Blank input keeps its historical meaning (supplied, selects nothing),
    // so it still suppresses the closed default below.
    let status_categories = params
        .status_category
        .as_deref()
        .map(|raw| parse_csv(Some(raw)).unwrap_or_default());
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
        label_ids: params
            .label
            .as_deref()
            .map(|raw| parse_csv(Some(raw)).unwrap_or_default()),
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

    let resume = cursor.as_ref().map(|c| issue_service::IssueWalk {
        after: &c.after,
        snapshot_at: &c.snapshot_at,
        total: c.total,
        served: c.served,
    });
    let page = issue_service::list_issues_page(
        ctx.db,
        &issue_service::IssuePageRequest {
            workspace_id: &ctx.workspace_id,
            team_id: team_id.as_deref(),
            filters: &filters,
            clauses: &clauses,
            sort,
            resume,
            limit,
        },
    )
    .await?;

    let next_cursor = if page.has_more {
        page.issues.last().map(|last| {
            encode_cursor(
                &ListCursor {
                    after: IssueSortKey::from(last),
                    snapshot_at: page.snapshot_at.clone(),
                    total: page.total,
                    served: page.served_count,
                },
                &fingerprint,
            )
        })
    } else {
        None
    };

    let rows: Vec<IssueListRow> = page.issues.into_iter().map(IssueListRow::from).collect();

    let response = ListIssuesResponse {
        returned_count: rows.len(),
        issues: rows,
        total: page.total as usize,
        snapshot_at: page.snapshot_at,
        served_count: page.served_count,
        has_more: page.has_more,
        next_cursor,
        limit,
        sort_by: sort.field.as_str().to_string(),
        sort_dir: sort.dir.as_str().to_string(),
        matched_count: page.total as usize,
        truncated: page.has_more,
    };
    Ok(serde_json::to_value(&response)?)
}

/// Default and maximum page size for `list_issues`.
const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 100;

/// Page size for a list operation: the default when unset, else clamped to
/// `1..=max`.
pub(crate) fn clamp_limit(raw: Option<i64>, default: i64, max: i64) -> i64 {
    raw.unwrap_or(default).clamp(1, max)
}

const CURSOR_VERSION: &str = "v4";
/// Upper bound on a cursor's `total`: far above any real backlog. A cursor's
/// `served` is bounded by [`issue_service::MAX_WALK_SERVED`].
const MAX_WALK_COUNT: i64 = 1_000_000_000;
const CURSOR_SEP: char = '\u{1e}';

/// What an opaque `list_issues` cursor carries.
struct ListCursor {
    /// Sort key of the last row served.
    after: IssueSortKey,
    /// The walk's snapshot instant (database clock, first page).
    snapshot_at: String,
    /// Matching rows in the whole walk, counted on the first page.
    total: i64,
    /// Rows the walk has returned, this page included.
    served: i64,
}

fn encode_cursor(c: &ListCursor, fingerprint: &str) -> String {
    use base64::Engine;
    let k = &c.after;
    let raw = [
        CURSOR_VERSION,
        &k.priority.to_string(),
        &k.created_at,
        &k.updated_at,
        &k.number.to_string(),
        &k.team_id,
        &k.issue_id,
        &c.snapshot_at,
        &c.total.to_string(),
        &c.served.to_string(),
        fingerprint,
    ]
    .join(&CURSOR_SEP.to_string());
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
    let parts: Vec<&str> = text.splitn(11, CURSOR_SEP).collect();
    match parts.as_slice() {
        [CURSOR_VERSION, priority, created_at, updated_at, number, team_id, issue_id, snapshot_at, total, served, fp]
            if *fp == fingerprint =>
        {
            let priority: i32 = priority.parse().map_err(|_| bad())?;
            let total: i64 = total.parse().map_err(|_| bad())?;
            let served: i64 = served.parse().map_err(|_| bad())?;
            // The timestamps are cast to a typed timestamp by the database, and
            // the numbers are bound as integers: reject anything the database
            // would not accept as this cursor's own output, so a forged cursor
            // is a 400 on every backend rather than a 500 on Postgres.
            // Priority and number are plain integers the write paths accept
            // without a range, so they are not range-checked here: a cursor this
            // endpoint generated must always be accepted back. The counts do
            // come from this endpoint's own responses; bound them so a forged
            // cursor cannot overflow the per-page accumulation.
            if total < 0
                || served < 0
                || total > MAX_WALK_COUNT
                || served > issue_service::MAX_WALK_SERVED
                || ![created_at, updated_at, snapshot_at]
                    .iter()
                    .all(|t| is_db_timestamp(t))
            {
                return Err(bad());
            }
            Ok(ListCursor {
                after: IssueSortKey {
                    priority,
                    created_at: created_at.to_string(),
                    updated_at: updated_at.to_string(),
                    number: number.parse().map_err(|_| bad())?,
                    team_id: team_id.to_string(),
                    issue_id: issue_id.to_string(),
                },
                snapshot_at: snapshot_at.to_string(),
                total,
                served,
            })
        }
        _ => Err(bad()),
    }
}

/// Whether `t` is a timestamp the database renders and accepts back:
/// `YYYY-MM-DD[ T]HH:MM:SS[.f…]` optionally followed by `Z` or a `±HH[:MM]`
/// offset (SQLite renders no offset, Postgres `+00`-style ones).
fn is_db_timestamp(t: &str) -> bool {
    let b = t.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = b.get(r)?;
        if part.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(part).ok()?.parse().ok()
        } else {
            None
        }
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(min), Some(sec)) = (
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19),
    ) else {
        return false;
    };
    let separators_ok = b.get(4) == Some(&b'-')
        && b.get(7) == Some(&b'-')
        && matches!(b.get(10), Some(b' ' | b'T'))
        && b.get(13) == Some(&b':')
        && b.get(16) == Some(&b':');
    // Postgres has no year 0 (`'0000-01-01'::timestamptz` is out of range), so
    // accepting it would let a forged cursor reach a cast error — a 500.
    if year == 0 {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if !separators_ok || day == 0 || day > days_in_month || hour > 23 || min > 59 || sec > 59 {
        return false;
    }
    let mut rest = &t[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 9 {
            return false;
        }
        rest = &frac[n..];
    }
    match rest {
        "" | "Z" => true,
        _ => {
            let Some(sign_rest) = rest.strip_prefix(['+', '-']) else {
                return false;
            };
            let (hh, mm) = match sign_rest.split_once(':') {
                Some((hh, mm)) => (hh, Some(mm)),
                None => (sign_rest, None),
            };
            hh.len() == 2
                && hh.bytes().all(|c| c.is_ascii_digit())
                && hh.parse::<u32>().is_ok_and(|h| h <= 15)
                && mm.is_none_or(|m| m.len() == 2 && m.bytes().all(|c| c.is_ascii_digit()) && m.parse::<u32>().is_ok_and(|m| m <= 59))
        }
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

/// Parse the JSON `filters` parameter shared by `list_issues` and `search_issues`.
fn parse_filter_clauses(raw: Option<&str>) -> ApiResult<Vec<FilterClause>> {
    FilterClause::parse_list(raw)
        .map_err(|e| ApiError::BadRequest(format!("Invalid filters JSON: {e}")))
}

/// Split a comma-separated filter value; blank or empty input means "no filter".
fn parse_csv(raw: Option<&str>) -> Option<Vec<String>> {
    let values: Vec<String> = raw?
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    (!values.is_empty()).then_some(values)
}

/// Search issues by text query using full-text search.
///
/// On Postgres, uses tsvector with GIN indexes for ranked results and
/// snippet context. On SQLite, falls back to LIKE matching.
pub async fn search_issues(
    ctx: &ApiCtx<'_>,
    params: SearchIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let limit = clamp_limit(params.limit, 20, 100);
    let clauses = parse_filter_clauses(params.filters.as_deref())?;

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
        // No hidden defaults: only filters the caller set are applied.
        exclude_archived: params.include_archived == Some(false),
        exclude_closed: params.include_closed == Some(false),
        status_id: params.status_id.filter(|s| !s.is_empty()),
        status_categories: parse_csv(params.status_category.as_deref()),
        clauses,
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
            description: "Find issues in the workspace with optional filters. Returns `{issues, total, snapshot_at, served_count, has_more, next_cursor, limit, sort_by, sort_dir, matched_count, returned_count, truncated}`. `total` is the exact number of matching issues ignoring `limit`; a response is complete only when `has_more` is false. Pagination: `limit` is the page size (default 50, max 100; out-of-range values are clamped and the applied value is echoed as `limit`). When `has_more` is true, call again with the same filters and `cursor` set to `next_cursor` (an opaque string; changing any filter or sort invalidates it) until `has_more` is false. A walk is a snapshot as of `snapshot_at` (the database clock when its first page was read). A write stamped at or before `snapshot_at` counts as of the walk's start: `total` is counted once then and is the same on every page, and every page lists issues last modified at or before `snapshot_at`. A write stamped at or before `snapshot_at` that moves an already-served issue past the cursor can return that issue once more, and one that moves an issue the walk had not reached to before the cursor can leave it out of that walk (it appears in a fresh walk). Both are visible in `served_count`. An ordinary edit does not move an issue in a `number` or `created_at` walk; only a write that changes its key (`number` by a team move or a delete-team reassignment, `created_at` never) can, and those stamp `updated_at` and follow the same rules. A priority walk moves on a priority edit, which also stamps `updated_at`. `served_count` reports how many rows this walk has returned so far: it is `total` plus any repeat, minus rows that stopped matching before they were served, minus rows a pre-snapshot write moved before the cursor before they were served, plus rows that started matching without a write that moves `updated_at` and were served. `has_more` false ends the walk whatever it says. An issue modified after `snapshot_at` is not part of that walk; it appears in a fresh walk (a request without `cursor`). An issue deleted, archived or otherwise no longer matching during the walk is absent from later pages. Sorting: `sort_by` is priority (default), created_at, updated_at or number; `sort_dir` is asc or desc (default asc for priority, so Urgent (1), High (2), Medium (3), Low (4), None (0) last, newest first within a priority; default desc for created_at, updated_at and number). Ties are broken deterministically so pages are stable. By default, completed and cancelled issues are excluded — pass include_closed=true to include them. Supports a `filters` parameter: a JSON array of `{field, operator, values}` clauses AND-ed together. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations, issue_id (a set of issue IDs, used by the Starred view). Operators: any_of, none_of, all_of, not_any_of, not_all_of. `matched_count` equals `total` and `truncated` equals `has_more` (kept for older callers). Each row is lean by design — `number`, `key` (e.g. 'TRA-35'), `title`, `priority`, `priority_name` (Urgent, High, Medium, Low, or No priority), `status_id`, `status_name`, `created_at`, `updated_at`, and `labels` (id and name only) — enough to find, sort, and triage. Rows never include the issue description, comments, or activities, and there is no option to add them: descriptions are multi-KB and would dominate the payload. To read a ticket, call get_issue with the row's `key`.",
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
            description: "Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns `{results, total, has_more, next_offset, truncated}`; results are ranked by relevance with snippet context. `limit` is the page size (default 20, max 100, clamped). A query returns up to its 1000 best-ranked matches: `total` is the number of distinct matching issues within that window — exact when `truncated` is false — and `truncated` true means more matches exist beyond it. When `has_more` is true, repeat the request with `offset` set to `next_offset` until `has_more` is false. Search applies only the filters you pass and no hidden defaults: with no status filter it covers every status, including Done and Cancelled, and archived issues. Narrow with status_id, status_category (comma-separated: backlog, unstarted, started, completed, cancelled), the same composable `filters` as list_issues (a JSON array of `{field, operator, values}` clauses AND-ed together; fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations, issue_id (a set of issue IDs, used by the Starred view); operators: any_of, none_of, all_of, not_any_of, not_all_of), applied before ranking and paging, team_key/team_id, include_archived=false, or include_comments=false (comments are searched by default). An exact identifier such as 'TRA-35' always matches that issue unless a filter you set excludes it. include_closed is deprecated and kept for compatibility: omitted or true means no status filtering; only an explicit false excludes completed and cancelled issues.",
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

    fn search_params(query: &str) -> SearchIssuesApiParams {
        SearchIssuesApiParams {
            query: query.to_string(),
            team_key: None,
            team_id: None,
            status_id: None,
            status_category: None,
            filters: None,
            include_closed: None,
            include_archived: None,
            include_comments: Some(false),
            limit: Some(50),
            offset: None,
        }
    }

    fn search_ctx<'a>(db: &'a DbPool, workspace_id: &str, user_id: &str) -> ApiCtx<'a> {
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

    /// Issue 1 stays Backlog, 2 is Done, 3 is Cancelled; all titles share "Seeded".
    async fn workspace_with_closed_issues() -> (DbPool, String, String) {
        let (db, workspace_id, user_id) = seeded_workspace(3).await;
        for (number, status) in [(2, "done"), (3, "cancelled")] {
            let status_id = format!("{workspace_id}::{status}");
            trakkt_core::db_execute!(
                &db,
                "UPDATE issues SET status_id = $1 WHERE workspace_id = $2 AND number = $3",
                &status_id,
                &workspace_id,
                number
            )
            .expect("move fixture issue to a closed status");
        }
        (db, workspace_id, user_id)
    }

    fn result_numbers(value: &serde_json::Value) -> Vec<i64> {
        let mut numbers: Vec<i64> = value["results"]
            .as_array()
            .expect("results array")
            .iter()
            .map(|r| r["number"].as_i64().expect("number"))
            .collect();
        numbers.sort_unstable();
        numbers
    }

    /// TRA-10114: with no filters set, search covers every status.
    #[tokio::test]
    async fn search_without_filters_returns_done_and_cancelled_issues() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        let ctx = search_ctx(&db, &workspace_id, &user_id);

        let value = search_issues(&ctx, search_params("Seeded")).await.expect("search");
        assert_eq!(result_numbers(&value), vec![1, 2, 3]);
    }

    /// TRA-10114: an exact identifier matches a Done issue.
    #[tokio::test]
    async fn search_by_identifier_matches_done_issue() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        let ctx = search_ctx(&db, &workspace_id, &user_id);

        let value = search_issues(&ctx, search_params("TRK-2")).await.expect("search");
        assert_eq!(result_numbers(&value), vec![2]);
    }

    /// TRA-10114: a status filter restricts results to exactly those statuses.
    #[tokio::test]
    async fn search_status_filters_restrict_results() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        let ctx = search_ctx(&db, &workspace_id, &user_id);

        let mut by_id = search_params("Seeded");
        by_id.status_id = Some(format!("{workspace_id}::done"));
        let value = search_issues(&ctx, by_id).await.expect("search");
        assert_eq!(result_numbers(&value), vec![2]);

        let mut by_category = search_params("Seeded");
        by_category.status_category = Some("completed, cancelled".to_string());
        let value = search_issues(&ctx, by_category).await.expect("search");
        assert_eq!(result_numbers(&value), vec![2, 3]);

        // Identifier lookups honour filters the user set explicitly.
        let mut identifier = search_params("TRK-1");
        identifier.status_category = Some("completed".to_string());
        let value = search_issues(&ctx, identifier).await.expect("search");
        assert!(result_numbers(&value).is_empty());

        // Combined with a team filter the bind positions still line up.
        let mut with_team = search_params("TRK-2");
        with_team.team_key = Some("TRK".to_string());
        with_team.status_category = Some("completed".to_string());
        let value = search_issues(&ctx, with_team).await.expect("search");
        assert_eq!(result_numbers(&value), vec![2]);
    }

    /// TRA-10114: composable filter clauses are applied by the search itself,
    /// before ranking and paging.
    #[tokio::test]
    async fn search_applies_filter_clauses_server_side() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        for (number, priority) in [(1, 1), (2, 1), (3, 4)] {
            trakkt_core::db_execute!(
                &db,
                "UPDATE issues SET priority = $1 WHERE workspace_id = $2 AND number = $3",
                priority,
                &workspace_id,
                number
            )
            .expect("set fixture priority");
        }
        let ctx = search_ctx(&db, &workspace_id, &user_id);

        let params: SearchIssuesApiParams = serde_json::from_value(serde_json::json!({
            "query": "Seeded",
            "include_comments": false,
            "filters": r#"[{"field":"priority","operator":"any_of","values":["1"]}]"#,
        }))
        .expect("params");
        let value = search_issues(&ctx, params).await.expect("search");
        assert_eq!(result_numbers(&value), vec![1, 2]);

        // A page size of one still pages through only the filtered matches.
        let params: SearchIssuesApiParams = serde_json::from_value(serde_json::json!({
            "query": "Seeded",
            "include_comments": false,
            "limit": 1,
            "filters": r#"[{"field":"priority","operator":"none_of","values":["1"]}]"#,
        }))
        .expect("params");
        let value = search_issues(&ctx, params).await.expect("search");
        assert_eq!(value["total"], 1);
        assert_eq!(result_numbers(&value), vec![3]);

        let bad: SearchIssuesApiParams = serde_json::from_value(serde_json::json!({
            "query": "Seeded",
            "filters": "not json",
        }))
        .expect("params");
        assert!(matches!(
            search_issues(&ctx, bad).await,
            Err(ApiError::BadRequest(_))
        ));
    }

    /// `include_closed` stays accepted: only an explicit false filters.
    #[tokio::test]
    async fn search_include_closed_is_compatible() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        let ctx = search_ctx(&db, &workspace_id, &user_id);

        for (flag, expected) in [(Some(true), vec![1, 2, 3]), (Some(false), vec![1]), (None, vec![1, 2, 3])] {
            let mut params = search_params("Seeded");
            params.include_closed = flag;
            let value = search_issues(&ctx, params).await.expect("search");
            assert_eq!(result_numbers(&value), expected, "include_closed={flag:?}");
        }
    }

    /// Archived issues are neither hidden implicitly nor ignored when asked.
    #[tokio::test]
    async fn search_archived_is_an_explicit_filter() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET archived_at = '2026-01-01T00:00:00Z' WHERE workspace_id = $1 AND number = 2",
            &workspace_id
        )
        .expect("archive fixture issue");
        let ctx = search_ctx(&db, &workspace_id, &user_id);

        let value = search_issues(&ctx, search_params("Seeded")).await.expect("search");
        assert_eq!(result_numbers(&value), vec![1, 2, 3]);

        let mut params = search_params("Seeded");
        params.include_archived = Some(false);
        let value = search_issues(&ctx, params).await.expect("search");
        assert_eq!(result_numbers(&value), vec![1, 3]);
    }


    /// TRA-10114 shared `parse_csv`: a blank `status_category` is still
    /// "supplied, selects nothing", so it keeps including closed issues.
    #[tokio::test]
    async fn list_issues_blank_status_category_keeps_its_meaning() {
        let (db, workspace_id, user_id) = workspace_with_closed_issues().await;
        let ctx = ApiCtx::from_leptos(
            workspace_id,
            user_id,
            &db,
            None,
            None,
            None,
            "http://localhost:3100",
        );

        for blank in ["", " ", ","] {
            let mut params = list_params(None);
            params.status_category = Some(blank.to_string());
            let value = list_issues(&ctx, params).await.expect("list_issues");
            let numbers: Vec<i64> = value["issues"]
                .as_array()
                .expect("rows")
                .iter()
                .map(|row| row["number"].as_i64().expect("number"))
                .collect();
            assert_eq!(numbers.len(), 3, "blank status_category {blank:?} must not filter");
        }

        // Omitting it keeps the documented default.
        let value = list_issues(&ctx, list_params(None)).await.expect("list_issues");
        assert_eq!(value["issues"].as_array().expect("rows").len(), 1);
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
                    status_id: None,
                    status_category: None,
                    filters: None,
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

    /// The search window is a stated contract: below 1000 matches `total` is
    /// exact and `truncated` is false; past it, `total` counts the 1000
    /// best-ranked matches and `truncated` says more exist.
    #[tokio::test]
    async fn search_reports_the_cap_explicitly() {
        let (db, workspace_id, user_id) = seeded_workspace(
            (trakkt_auth::search_service::SEARCH_FETCH_CAP + 5) as i32,
        )
        .await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let params = |limit| SearchIssuesApiParams {
            query: "Seeded".to_string(),
            status_id: None,
            status_category: None,
            team_key: None,
            team_id: None,
            include_closed: None,
            include_archived: None,
            include_comments: Some(false),
            filters: None,
            limit: Some(limit),
            offset: Some(0),
        };

        let past = search_issues(&ctx, params(10)).await.expect("search");
        assert_eq!(past["truncated"], true);
        assert_eq!(past["total"], trakkt_auth::search_service::SEARCH_FETCH_CAP);
        assert_eq!(past["has_more"], true);
        assert_eq!(past["results"].as_array().expect("results").len(), 10);

        // Narrowing the query below the cap makes total exact again.
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET title = 'aardvark zebra' WHERE workspace_id = $1 AND number = 3",
            &workspace_id
        )
        .expect("give one issue a unique title");
        let mut narrow = params(10);
        narrow.query = "aardvark".to_string();
        let below = search_issues(&ctx, narrow).await.expect("search");
        assert_eq!(below["truncated"], false);
        assert_eq!(below["total"], 1);
        assert_eq!(below["has_more"], false);
        assert!(below["next_offset"].is_null());
    }

    /// The documented contract for a mutable sort: a write stamped exactly at
    /// `snapshot_at` (the `<=` boundary) counts as of the walk's start, so
    /// moving an already-served issue past the cursor makes it appear once
    /// more, and `served_count` shows it. Under a stable-key sort the same
    /// write changes nothing.
    #[tokio::test]
    async fn a_write_stamped_at_the_snapshot_can_repeat_a_served_row() {
        let (db, workspace_id, user_id) = seeded_workspace(9).await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET updated_at = '2026-08-01 00:00:' || substr('0' || number, -2) WHERE workspace_id = $1",
            &workspace_id
        )
        .expect("distinct updated_at");
        let ctx = test_ctx(&db, &workspace_id, &user_id);

        for (sort_by, repeats) in [("updated_at", true), ("number", false)] {
            let mut params = list_params(None);
            params.limit = Some(3);
            params.sort_by = Some(sort_by.to_string());
            params.sort_dir = Some("asc".to_string());
            let first = list_issues(&ctx, params.clone()).await.expect("page 1");
            let served: Vec<i64> = numbers(&first);
            let total = first["total"].as_i64().expect("total");
            let snapshot_at = first["snapshot_at"].as_str().expect("snapshot_at").to_string();

            // The last served row is stamped exactly at the walk's snapshot:
            // the `<=` boundary counts it as of the walk's start. Under the
            // ascending mutable sort it now sorts after the cursor and is
            // served again; under the stable sort its position is unchanged.
            trakkt_core::db_execute!(
                &db,
                "UPDATE issues SET updated_at = $1 WHERE workspace_id = $2 AND number = $3",
                &snapshot_at,
                &workspace_id,
                *served.last().expect("a served row") as i32
            )
            .expect("write a served row at the snapshot instant");

            let mut seen = served.clone();
            let mut last_page = first.clone();
            let mut cursor = first["next_cursor"].as_str().map(str::to_string);
            while let Some(c) = cursor {
                let mut p = params.clone();
                p.cursor = Some(c);
                let page = list_issues(&ctx, p).await.expect("next page");
                assert_eq!(page["total"], total, "total is fixed for the walk");
                seen.extend(numbers(&page));
                cursor = page["next_cursor"].as_str().map(str::to_string);
                last_page = page;
            }
            let unique: std::collections::HashSet<_> = seen.iter().collect();
            let served_count = last_page["served_count"].as_i64().expect("served_count");
            assert_eq!(last_page["has_more"], false);
            assert_eq!(served_count, seen.len() as i64, "served_count counts every row returned");
            if repeats {
                assert_eq!(
                    seen.len() as i64,
                    total + 1,
                    "one repeat at the snapshot boundary: {seen:?}"
                );
                assert_eq!(unique.len() as i64, total);
                assert_eq!(served_count, total + 1);
            } else {
                assert_eq!(seen.len() as i64, total, "stable-key walks are exact: {seen:?}");
                assert_eq!(unique.len() as i64, total);
                assert_eq!(served_count, total);
            }
        }
    }

    /// A row that stops matching mid-walk (deleted or archived, neither of
    /// which moves `updated_at`) is absent from later pages while `total`
    /// stays the walk's size at its start, and no other row is disturbed.
    #[tokio::test]
    async fn rows_that_stop_matching_mid_walk_are_absent_from_later_pages() {
        let (db, workspace_id, user_id) = seeded_workspace(12).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(4);
        params.sort_by = Some("number".to_string());
        params.sort_dir = Some("asc".to_string());
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        assert_eq!(numbers(&first), vec![1, 2, 3, 4]);

        trakkt_core::db_execute!(
            &db,
            "DELETE FROM issues WHERE workspace_id = $1 AND number = 7",
            &workspace_id
        )
        .expect("delete an unreturned row");
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET archived_at = '2026-01-01 00:00:00' WHERE workspace_id = $1 AND number = 10",
            &workspace_id
        )
        .expect("archive an unreturned row");

        let mut seen = numbers(&first);
        let mut cursor = first["next_cursor"].as_str().map(str::to_string);
        let mut last_page = first.clone();
        while let Some(c) = cursor {
            let mut p = params.clone();
            p.cursor = Some(c);
            let page = list_issues(&ctx, p).await.expect("next page");
            assert_eq!(page["total"], 12, "total is the walk's size at its start");
            seen.extend(numbers(&page));
            cursor = page["next_cursor"].as_str().map(str::to_string);
            last_page = page;
        }
        assert_eq!(seen, vec![1, 2, 3, 4, 5, 6, 8, 9, 11, 12]);
        assert_eq!(
            last_page["served_count"], 10,
            "served_count counts rows returned: total minus the two that stopped matching"
        );
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

    /// TRA-10113 follow-up (reproduction): an already-served row edited so it
    /// now sorts *after* the cursor must not be served a second time, and the
    /// walk's `total` must not change under the edit.
    #[tokio::test]
    async fn a_served_row_edited_to_sort_later_is_not_repeated() {
        let (db, workspace_id, user_id) = seeded_workspace(12).await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET priority = 2, created_at = '2026-07-01 00:00:' || substr('0' || number, -2), \
             updated_at = '2026-08-01 00:00:00' WHERE workspace_id = $1",
            &workspace_id
        )
        .expect("fix the fixture's sort keys");
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(4);
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        let served: Vec<i64> = first["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .map(|r| r["number"].as_i64().expect("number"))
            .collect();

        // Demote a served row to Low: it now sorts after every other row.
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET priority = 4, updated_at = '2999-01-01 00:00:00' WHERE workspace_id = $1 AND number = $2",
            &workspace_id,
            served[0] as i32
        )
        .expect("demote a served row");

        let mut seen = served.clone();
        let mut cursor = first["next_cursor"].as_str().map(str::to_string);
        while let Some(c) = cursor {
            let mut p = params.clone();
            p.cursor = Some(c);
            let page = list_issues(&ctx, p).await.expect("next page");
            assert_eq!(page["total"], first["total"], "total is fixed for the walk");
            for r in page["issues"].as_array().expect("issues") {
                seen.push(r["number"].as_i64().expect("number"));
            }
            cursor = page["next_cursor"].as_str().map(str::to_string);
        }
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(seen.len(), unique.len(), "a row was served twice: {seen:?}");
    }

    fn numbers(v: &serde_json::Value) -> Vec<i64> {
        v["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .map(|r| r["number"].as_i64().expect("number"))
            .collect()
    }

    /// The walk is a snapshot by modification time: a row modified after the
    /// first page is absent from that walk, present in a fresh one, every
    /// unchanged row is served exactly once, and `total` and `snapshot_at`
    /// never change.
    #[tokio::test]
    async fn walk_is_a_snapshot_by_modification_time() {
        let (db, workspace_id, user_id) = seeded_workspace(12).await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET updated_at = '2026-08-01 00:00:' || substr('0' || number, -2) WHERE workspace_id = $1",
            &workspace_id
        )
        .expect("distinct updated_at");
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(4);
        params.sort_by = Some("number".to_string());
        params.sort_dir = Some("asc".to_string());
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        assert_eq!(numbers(&first), vec![1, 2, 3, 4]);
        assert_eq!(first["total"], 12);
        let snapshot_at = first["snapshot_at"].as_str().expect("snapshot_at").to_string();

        // Modify an unreturned row (7) after the walk began, the way a real
        // edit does: stamped with the database clock. SQLite stamps whole
        // seconds, so wait one out.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET title = 'edited', updated_at = datetime('now') WHERE workspace_id = $1 AND number = 7",
            &workspace_id
        )
        .expect("modify an unreturned row mid-walk");

        let mut seen = numbers(&first);
        let mut cursor = first["next_cursor"].as_str().map(str::to_string);
        while let Some(c) = cursor {
            let mut p = params.clone();
            p.cursor = Some(c);
            let page = list_issues(&ctx, p).await.expect("next page");
            assert_eq!(page["total"], 12, "total is constant for the walk");
            assert_eq!(page["snapshot_at"], snapshot_at.as_str());
            seen.extend(numbers(&page));
            cursor = page["next_cursor"].as_str().map(str::to_string);
        }
        assert_eq!(
            seen,
            vec![1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12],
            "every unchanged row once, the modified row not part of this walk"
        );

        // A fresh walk sees it, and counts it.
        let mut fresh = params.clone();
        fresh.limit = Some(100);
        let fresh = list_issues(&ctx, fresh).await.expect("fresh walk");
        assert_eq!(fresh["total"], 12);
        assert!(numbers(&fresh).contains(&7));
    }

    /// A row that becomes visible after page 1 but carries an *earlier*
    /// `updated_at` (a transaction that committed late) is an ordinary row of
    /// the walk: never served twice, and the walk is otherwise unaffected.
    #[tokio::test]
    async fn a_late_committing_row_with_an_earlier_timestamp_never_repeats() {
        let (db, workspace_id, user_id) = seeded_workspace(8).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(3);
        params.sort_by = Some("number".to_string());
        params.sort_dir = Some("asc".to_string());
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");

        // Insert a ninth issue now, stamped long before the snapshot.
        let team = trakkt_auth::team_service::get_default_team(&db, &workspace_id)
            .await
            .expect("default team");
        issue_service::create_issue(
            &db,
            &CreateIssueParams {
                workspace_id: workspace_id.clone(),
                team_id: team.team_id,
                creator_id: user_id.clone(),
                title: "late".to_string(),
                description: None,
                priority: 2,
                assignee_id: None,
                due_date: None,
                label_ids: Vec::new(),
                project_id: None,
                milestone_id: None,
                estimate: None,
            },
            None,
        )
        .await
        .expect("create the late row");
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET updated_at = '2020-01-01 00:00:00' WHERE workspace_id = $1 AND number = 9",
            &workspace_id
        )
        .expect("back-date the late row");

        let mut seen = numbers(&first);
        let mut cursor = first["next_cursor"].as_str().map(str::to_string);
        while let Some(c) = cursor {
            let mut p = params.clone();
            p.cursor = Some(c);
            let page = list_issues(&ctx, p).await.expect("next page");
            seen.extend(numbers(&page));
            cursor = page["next_cursor"].as_str().map(str::to_string);
        }
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(seen.len(), unique.len(), "a row repeated: {seen:?}");
        assert!((1..=8).all(|n| seen.contains(&n)), "an original row was lost: {seen:?}");
    }

    /// SQLite stores timestamps in two accepted shapes: the writer's
    /// `datetime('now')` (`YYYY-MM-DD HH:MM:SS`) and the schema default's
    /// `strftime(...,'%Y-%m-%dT%H:%M:%SZ')`. A walk must order and bound both
    /// as the same instant, so the paging SQL normalises them through
    /// `datetime()`. This inserts one row in the default shape beside the
    /// writer-shaped ones and walks it.
    #[tokio::test]
    async fn sqlite_walks_rows_stored_in_either_timestamp_shape() {
        let (db, workspace_id, user_id) = seeded_workspace(9).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let team = trakkt_auth::team_service::get_default_team(&db, &workspace_id)
            .await
            .expect("default team");
        let teen_id = format!("{workspace_id}::backlog");
        // An insert that names no timestamps, as the e2e fixtures do.
        trakkt_core::db_execute!(
            &db,
            "INSERT INTO issues (issue_id, workspace_id, team_id, number, title, status_id, \
             priority, creator_id) \
             VALUES ('default-stamped', $1, $2, 10, 'default stamp', $3, 2, $4)",
            &workspace_id,
            &team.team_id,
            &teen_id,
            &user_id
        )
        .expect("insert a row stamped by the schema default");
        let shape: String = trakkt_core::db_fetch_scalar!(
            &db,
            String,
            "SELECT updated_at FROM issues WHERE issue_id = 'default-stamped'"
        )
        .expect("read the default stamp");
        assert!(shape.contains('T') && shape.ends_with('Z'), "schema default shape: {shape}");

        for (sort_by, sort_dir) in [(Some("updated_at"), "asc"), (Some("updated_at"), "desc"), (Some("number"), "asc")] {
            let mut params = list_params(None);
            params.limit = Some(3);
            params.sort_by = sort_by.map(str::to_string);
            params.sort_dir = Some(sort_dir.to_string());
            let (_, rows) = walk_pages(&ctx, params, 10).await;
            let numbers: Vec<i64> = rows.iter().map(|r| r["number"].as_i64().expect("number")).collect();
            assert_eq!(numbers.len(), 10, "both shapes are inside the walk: {numbers:?}");
            assert!(numbers.contains(&10), "the default-stamped row was skipped: {numbers:?}");
        }
    }

    /// A cursor is the caller's input: every field the database will see is
    /// validated, so a forged cursor is a 400 on every backend.
    #[tokio::test]
    async fn forged_cursor_fields_are_bad_requests() {
        use base64::Engine;
        let (db, workspace_id, user_id) = seeded_workspace(6).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(2);
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        let cursor = first["next_cursor"].as_str().expect("next_cursor");
        let raw = String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(cursor)
                .expect("cursor is base64"),
        )
        .expect("cursor is utf-8");
        let parts: Vec<&str> = raw.split('\u{1e}').collect();
        assert_eq!(parts.len(), 11);

        // (field index, forged value): version, priority, created_at,
        // updated_at, number, team, id, snapshot_at, total, served.
        for (idx, forged) in [
            (0, "v1"),
            (2, "yesterday"),
            (2, "2026-02-30 00:00:00"),
            (3, "2026-13-01 00:00:00"),
            (4, "many"),
            (7, "now()"),
            (7, "2026-11-16"),
            (8, "-5"),
            (8, "lots"),
            (8, "2000000000"),
            (9, "-1"),
            (9, "2000000000"),
        ] {
            let mut forged_parts = parts.clone();
            forged_parts[idx] = forged;
            let mut p = params.clone();
            p.cursor = Some(
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(forged_parts.join("\u{1e}")),
            );
            assert!(
                matches!(list_issues(&ctx, p).await, Err(ApiError::BadRequest(_))),
                "forged field {idx} = {forged:?} must be a bad request"
            );
        }
        // The untouched cursor still works.
        let mut ok = params;
        ok.cursor = Some(cursor.to_string());
        list_issues(&ctx, ok).await.expect("the genuine cursor");
    }

    /// A Postgres-rendered cursor carries a bare `+00` offset, with or without
    /// fractional seconds; SQLite's `datetime()` needs minutes, so the bound
    /// text is repaired for it. A page walked across either form must still
    /// serve rows, not silently end.
    #[tokio::test]
    async fn a_bare_offset_in_the_snapshot_still_pages_on_sqlite() {
        use base64::Engine;
        let (db, workspace_id, user_id) = seeded_workspace(6).await;
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(2);
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        let snapshot = first["snapshot_at"].as_str().expect("snapshot").to_string();
        let cursor = first["next_cursor"].as_str().expect("cursor");
        let raw = String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(cursor)
                .expect("base64"),
        )
        .expect("utf8");
        let parts: Vec<String> = raw.split('\u{1e}').map(str::to_string).collect();

        for bare in [
            format!("{snapshot}+00"),
            format!("{snapshot}.123456+00"),
        ] {
            let mut forged = parts.clone();
            forged[7] = bare.clone();
            let mut p = params.clone();
            p.cursor = Some(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(forged.join("\u{1e}")),
            );
            let page = list_issues(&ctx, p).await.expect("page 2");
            assert!(
                !page["issues"].as_array().expect("issues").is_empty(),
                "a cursor carrying {bare} must still page"
            );
        }
    }

    #[test]
    fn db_timestamps_are_recognised_in_every_rendering() {
        for ok in [
            "2026-11-16 00:30:05",
            "2026-11-16 00:30:05.123456+00",
            "2026-11-01 01:30:00-04",
            "2026-11-01 01:30:00.5-05:00",
            "2026-11-16T00:30:05Z",
            "2024-02-29 12:00:00",
        ] {
            assert!(is_db_timestamp(ok), "{ok} should be accepted");
        }
        for bad in [
            "",
            "2026-11-16",
            "2026-02-29 00:00:00",
            "2026-11-16 24:00:00",
            "2026-11-16 00:60:00",
            "2026-11-16 00:00:00.",
            "2026-11-16 00:00:00+",
            "2026-11-16 00:00:00+1",
            "2026-11-16 00:00:00 UTC",
            "'; DROP TABLE issues; --",
        ] {
            assert!(!is_db_timestamp(bad), "{bad:?} should be rejected");
        }
    }

    /// The unsorted service list keeps loading descriptions (UI and sync use it).
    #[tokio::test]
    async fn service_list_issues_still_loads_descriptions() {
        let (db, workspace_id, _user_id) = seeded_workspace(2).await;
        let rows = issue_service::list_issues(
            &db,
            &workspace_id,
            None,
            &IssueFilters {
                status_id: None,
                status_categories: None,
                exclude_status_categories: None,
                priority: None,
                assignee_id: None,
                creator_id: None,
                label_ids: None,
                search: None,
                limit: None,
                offset: None,
                include_archived: None,
                only_archived: None,
            },
        )
        .await
        .expect("list");
        assert!(rows.iter().all(|r| r.description.is_some()));
    }

    /// An anchor that is edited (moving to the top of an `updated_at` walk) or
    /// deleted along with other served rows must not replay or skip the rest.
    #[tokio::test]
    async fn cursor_survives_anchor_edit_and_deletions() {
        let (db, workspace_id, user_id) = seeded_workspace(12).await;
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET updated_at = '2026-08-01 00:00:' || substr('0' || number, -2) WHERE workspace_id = $1",
            &workspace_id
        )
        .expect("distinct updated_at");
        let ctx = test_ctx(&db, &workspace_id, &user_id);
        let mut params = list_params(None);
        params.limit = Some(4);
        params.sort_by = Some("updated_at".to_string());
        params.sort_dir = Some("desc".to_string());
        let nums = |v: &serde_json::Value| -> Vec<i64> {
            v["issues"]
                .as_array()
                .expect("issues")
                .iter()
                .map(|r| r["number"].as_i64().expect("number"))
                .collect()
        };
        let first = list_issues(&ctx, params.clone()).await.expect("page 1");
        assert_eq!(nums(&first), vec![12, 11, 10, 9]);

        // Edit the anchor (9) so it now sorts first; delete two served rows.
        trakkt_core::db_execute!(
            &db,
            "UPDATE issues SET updated_at = '2026-09-01 00:00:00' WHERE workspace_id = $1 AND number = 9",
            &workspace_id
        )
        .expect("edit anchor");
        trakkt_core::db_execute!(
            &db,
            "DELETE FROM issues WHERE workspace_id = $1 AND number IN (11, 12)",
            &workspace_id
        )
        .expect("delete served rows");
        params.cursor = first["next_cursor"].as_str().map(str::to_string);
        let second = list_issues(&ctx, params).await.expect("page 2");
        assert_eq!(nums(&second), vec![8, 7, 6, 5]);
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
