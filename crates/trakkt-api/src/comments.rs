// SPDX-License-Identifier: AGPL-3.0-or-later

//! Comment operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::activity_service::ActivityRecorder;
use trakkt_auth::{comment_service, issue_service};
use trakkt_types::api::AddCommentApiParams;

use crate::context::resolve_issue_key_and_number;
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Add a comment to an issue.
///
/// Ported from `tool_add_comment` in `routes/mcp.rs`.
pub async fn add_comment(
    ctx: &ApiCtx<'_>,
    params: AddCommentApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    // Resolve issue_id from team-scoped identifier.
    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("issue {team_key}-{number} not found")))?;

    let comment = comment_service::create_comment(
        ctx.db,
        &issue.issue_id,
        &ctx.user_id,
        &params.body,
        params.parent_id.as_deref(),
        ctx.ws_manager,
    )
    .await?;

    // Record activity — never fails the mutation.
    let recorder = ActivityRecorder::new(ctx.db, &ctx.workspace_id, &ctx.user_id, ctx.ws_manager);
    let meta = json!({ "comment_id": comment.comment_id });
    if let Err(e) = recorder.record(&issue.issue_id, "comment_added", Some(&meta)).await {
        tracing::warn!(issue_id = %issue.issue_id, "Failed to record comment activity: {e}");
    }

    Ok(serde_json::to_value(&comment)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all comment-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![ApiOperation {
        name: "add_comment",
        description: "Add a comment to an issue. Comments support markdown formatting.",
        scope: "comments:write",
        rest_method: Method::POST,
        rest_path: "/issues/{identifier}/comments",
        json_schema: || schemars::schema_for!(AddCommentApiParams),
        handler: Box::new(|ctx, value| {
            Box::pin(async move {
                let params: AddCommentApiParams = serde_json::from_value(value)?;
                add_comment(&ctx, params).await
            })
        }),
        binary_input: None,
        binary_output: None,
    }]
}
