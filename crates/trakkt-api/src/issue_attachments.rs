// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue attachment operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Links and unlinks attachments from issues.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::{attachment_service, issue_service};
use trakkt_types::api::{
    AttachToIssueApiParams, DetachFromIssueApiParams, ListIssueAttachmentsApiParams,
};

use crate::context::resolve_issue_key_and_number;
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all attachments linked to an issue.
///
/// Resolves the issue from either a compound identifier (e.g. `"TRA-35"`) or
/// explicit `team_key` + `issue_number`, verifies it exists in the workspace,
/// then fetches all linked attachment records.
pub async fn list_issue_attachments(
    ctx: &ApiCtx<'_>,
    params: ListIssueAttachmentsApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Issue {team_key}-{number} not found")))?;

    let attachments =
        attachment_service::list_issue_attachments(ctx.db, &ctx.workspace_id, &issue.issue_id)
            .await?;

    Ok(serde_json::to_value(&attachments)?)
}

/// Attach an existing attachment to an issue.
///
/// Resolves the issue identifier, then creates the link in the
/// `issue_attachments` junction table. Idempotent — re-attaching is a no-op.
pub async fn attach_to_issue(
    ctx: &ApiCtx<'_>,
    params: AttachToIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Issue {team_key}-{number} not found")))?;

    attachment_service::attach_to_issue(
        ctx.db,
        &ctx.workspace_id,
        &issue.issue_id,
        &params.attachment_id,
        ctx.ws_manager,
    )
    .await?;

    Ok(json!({ "message": "Attachment linked to issue successfully" }))
}

/// Detach an attachment from an issue.
///
/// Removes the link from the `issue_attachments` junction table. Does NOT
/// delete the attachment record itself.
pub async fn detach_from_issue(
    ctx: &ApiCtx<'_>,
    params: DetachFromIssueApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Issue {team_key}-{number} not found")))?;

    attachment_service::detach_from_issue(
        ctx.db,
        &ctx.workspace_id,
        &issue.issue_id,
        &params.attachment_id,
        ctx.ws_manager,
    )
    .await?;

    Ok(json!({ "message": "Attachment unlinked from issue successfully" }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all issue-attachment-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_issue_attachments",
            description: "List all attachments linked to an issue.",
            scope: "attachments:read",
            rest_method: Method::GET,
            rest_path: "/issues/{identifier}/attachments",
            json_schema: || schemars::schema_for!(ListIssueAttachmentsApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListIssueAttachmentsApiParams = serde_json::from_value(value)?;
                    list_issue_attachments(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "attach_to_issue",
            description: "Attach an existing attachment to an issue. Idempotent — re-attaching is a no-op.",
            scope: "attachments:write",
            rest_method: Method::POST,
            rest_path: "/issues/{identifier}/attachments",
            json_schema: || schemars::schema_for!(AttachToIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: AttachToIssueApiParams = serde_json::from_value(value)?;
                    attach_to_issue(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "detach_from_issue",
            description: "Detach an attachment from an issue. Does not delete the attachment itself.",
            scope: "attachments:write",
            rest_method: Method::DELETE,
            rest_path: "/issues/{identifier}/attachments/{attachment_id}",
            json_schema: || schemars::schema_for!(DetachFromIssueApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: DetachFromIssueApiParams = serde_json::from_value(value)?;
                    detach_from_issue(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
    ]
}
