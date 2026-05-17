// SPDX-License-Identifier: AGPL-3.0-or-later

//! Activity operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Fetches the activity log for an issue.

use axum::http::Method;

use trakkt_auth::{activity_service, issue_service};
use trakkt_types::api::ListIssueActivitiesApiParams;

use crate::context::resolve_issue_key_and_number;
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all activity entries for an issue, ordered chronologically.
///
/// Resolves the issue from either a compound identifier (e.g. `"TRA-35"`) or
/// explicit `team_key` + `issue_number`, verifies it exists in the workspace,
/// then fetches all activity rows.
pub async fn list_issue_activities(
    ctx: &ApiCtx<'_>,
    params: ListIssueActivitiesApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Issue {team_key}-{number} not found")))?;

    let activities = activity_service::list_issue_activities(ctx.db, &issue.issue_id).await?;

    Ok(serde_json::to_value(&activities)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all activity-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![ApiOperation {
        name: "list_issue_activities",
        description:
            "List all activity entries for an issue, ordered chronologically.",
        scope: "issues:read",
        rest_method: Method::GET,
        rest_path: "/issues/{identifier}/activities",
        json_schema: || schemars::schema_for!(ListIssueActivitiesApiParams),
        handler: Box::new(|ctx, value| {
            Box::pin(async move {
                let params: ListIssueActivitiesApiParams = serde_json::from_value(value)?;
                list_issue_activities(&ctx, params).await
            })
        }),
        binary_input: None,
        binary_output: None,
    }]
}
