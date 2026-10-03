// SPDX-License-Identifier: AGPL-3.0-or-later

//! Star operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Stars are per-user bookmarks for issues.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::star_service;
use trakkt_types::api::{ListStarredIssuesApiParams, StarIssueApiParams, UnstarIssueApiParams};

use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Verify that an issue belongs to the authenticated workspace.
async fn verify_issue_workspace(ctx: &ApiCtx<'_>, issue_id: &str) -> ApiResult<()> {
    #[derive(sqlx::FromRow)]
    struct Row {
        workspace_id: String,
    }
    let row: Option<Row> = trakkt_core::db_fetch_optional!(
        ctx.db,
        Row,
        "SELECT workspace_id FROM issues WHERE issue_id = $1",
        issue_id
    )
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(r) if r.workspace_id == ctx.workspace_id => Ok(()),
        Some(_) => Err(ApiError::Forbidden("Issue belongs to another workspace".into())),
        None => Err(ApiError::NotFound("Issue not found".into())),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Star an issue for the current user.
pub async fn star_issue(ctx: &ApiCtx<'_>, params: StarIssueApiParams) -> ApiResult<serde_json::Value> {
    verify_issue_workspace(ctx, &params.issue_id).await?;
    star_service::star_issue(ctx.db, &params.issue_id, &ctx.user_id).await?;
    Ok(json!({ "message": format!("Issue {} starred", params.issue_id) }))
}

/// Unstar an issue for the current user.
pub async fn unstar_issue(ctx: &ApiCtx<'_>, params: UnstarIssueApiParams) -> ApiResult<serde_json::Value> {
    verify_issue_workspace(ctx, &params.issue_id).await?;
    star_service::unstar_issue(ctx.db, &params.issue_id, &ctx.user_id).await?;
    Ok(json!({ "message": format!("Issue {} unstarred", params.issue_id) }))
}

/// List all starred issue IDs for the current user in the active workspace.
pub async fn list_starred_issues(ctx: &ApiCtx<'_>, _params: ListStarredIssuesApiParams) -> ApiResult<serde_json::Value> {
    let ids = star_service::list_starred_issue_ids(ctx.db, &ctx.user_id, &ctx.workspace_id).await?;
    Ok(json!({ "starred_issue_ids": ids }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all star-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "star_issue",
            description: "Star an issue for the current user.",
            scope: "issues:write",
            rest_method: Method::POST,
            rest_path: "/issues/{id}/star",
            json_schema: || schemars::schema_for!(StarIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: StarIssueApiParams = serde_json::from_value(value)?;
                    star_issue(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "unstar_issue",
            description: "Unstar an issue for the current user.",
            scope: "issues:write",
            rest_method: Method::DELETE,
            rest_path: "/issues/{id}/star",
            json_schema: || schemars::schema_for!(UnstarIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: UnstarIssueApiParams = serde_json::from_value(value)?;
                    unstar_issue(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "list_starred_issues",
            description: "List all issue IDs starred by the current user in the active workspace.",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/starred-issues",
            json_schema: || schemars::schema_for!(ListStarredIssuesApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListStarredIssuesApiParams = serde_json::from_value(value)?;
                    list_starred_issues(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
    ]
}
