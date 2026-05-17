// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub link operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Fetches the GitHub links for an issue.

use axum::http::Method;

use trakkt_auth::issue_service;
use trakkt_types::api::ListGitHubLinksApiParams;

use crate::context::resolve_issue_key_and_number;
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all GitHub links for an issue (PRs, branches, commits).
///
/// Resolves the issue from either a compound identifier (e.g. `"TRA-35"`) or
/// explicit `team_key` + `issue_number`, verifies it exists in the workspace,
/// then fetches all GitHub link rows.
pub async fn list_issue_github_links(
    ctx: &ApiCtx<'_>,
    params: ListGitHubLinksApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Issue {team_key}-{number} not found")))?;

    let links = trakkt_github::schema::list_links_for_issue(ctx.db, &issue.issue_id).await?;

    Ok(serde_json::to_value(&links)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all GitHub-link-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![ApiOperation {
        name: "list_issue_github_links",
        description:
            "List all GitHub links (PRs, branches, commits) associated with an issue.",
        scope: "issues:read",
        rest_method: Method::GET,
        rest_path: "/issues/{identifier}/github-links",
        json_schema: || schemars::schema_for!(ListGitHubLinksApiParams),
        handler: Box::new(|ctx, value| {
            Box::pin(async move {
                let params: ListGitHubLinksApiParams = serde_json::from_value(value)?;
                list_issue_github_links(&ctx, params).await
            })
        }),
        binary_input: None,
        binary_output: None,
    }]
}
