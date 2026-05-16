// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relation operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::{issue_service, relation_service};
use trakkt_types::api::{AddRelationApiParams, ListRelationsApiParams, RemoveRelationApiParams};

use crate::context::{parse_issue_identifier, resolve_issue_key_and_number};
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Add a relation between two issues.
///
/// Supports 'blocks' (source blocks target) and 'parent' (source is parent of
/// target) relation types. Both source and target are resolved from compound
/// identifiers like 'TRA-35'.
///
/// Ported from `tool_add_relation` in `routes/mcp.rs`.
pub async fn add_relation(
    ctx: &ApiCtx<'_>,
    params: AddRelationApiParams,
) -> ApiResult<serde_json::Value> {
    let source_identifier = params.source_issue.as_deref().ok_or_else(|| {
        ApiError::BadRequest("source_issue is required".to_string())
    })?;

    // Resolve source issue identifier to issue_id.
    let (source_key, source_num) =
        parse_issue_identifier(source_identifier).ok_or_else(|| {
            ApiError::BadRequest("Invalid source_issue format. Expected 'TRA-35'".to_string())
        })?;
    let source_issue =
        issue_service::get_issue(ctx.db, &ctx.workspace_id, &source_key, source_num)
            .await?
            .ok_or_else(|| {
                ApiError::NotFound(format!("Source issue {source_identifier} not found"))
            })?;

    // Resolve target issue identifier to issue_id.
    let (target_key, target_num) =
        parse_issue_identifier(&params.target_issue).ok_or_else(|| {
            ApiError::BadRequest("Invalid target_issue format. Expected 'TRA-35'".to_string())
        })?;
    let target_issue =
        issue_service::get_issue(ctx.db, &ctx.workspace_id, &target_key, target_num)
            .await?
            .ok_or_else(|| {
                ApiError::NotFound(format!("Target issue {} not found", params.target_issue))
            })?;

    let relation = relation_service::create_relation(
        ctx.db,
        &ctx.workspace_id,
        &source_issue.issue_id,
        &target_issue.issue_id,
        &params.relation_type,
        Some(&ctx.user_id),
        ctx.ws_manager,
    )
    .await?;

    Ok(serde_json::to_value(&relation)?)
}

/// Remove a relation between two issues by its relation ID.
///
/// Ported from `tool_remove_relation` in `routes/mcp.rs`.
pub async fn remove_relation(
    ctx: &ApiCtx<'_>,
    params: RemoveRelationApiParams,
) -> ApiResult<serde_json::Value> {
    relation_service::delete_relation(
        ctx.db,
        &params.relation_id,
        &ctx.workspace_id,
        ctx.ws_manager,
    )
    .await?;

    Ok(json!({ "message": "Relation removed successfully" }))
}

/// List all relations for an issue (both directions — blocks and blocked-by).
///
/// Ported from `tool_list_issue_relations` in `routes/mcp.rs`.
pub async fn list_relations(
    ctx: &ApiCtx<'_>,
    params: ListRelationsApiParams,
) -> ApiResult<serde_json::Value> {
    let (team_key, number) = resolve_issue_key_and_number(
        params.issue_identifier.as_deref(),
        params.team_key.as_deref(),
        params.issue_number,
    )?;

    let issue = issue_service::get_issue(ctx.db, &ctx.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Issue {team_key}-{number} not found")))?;

    let relations =
        relation_service::list_relations_for_issue(ctx.db, &issue.issue_id, &ctx.workspace_id)
            .await?;

    Ok(serde_json::to_value(&relations)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all relation-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "add_relation",
            description: "Add a relation between two issues. Supports 'blocks' (source blocks target) and 'parent' (source is parent of target) relation types.",
            scope: "issues:write",
            rest_method: Method::POST,
            rest_path: "/issues/{identifier}/relations",
            json_schema: || schemars::schema_for!(AddRelationApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: AddRelationApiParams = serde_json::from_value(value)?;
                    add_relation(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "remove_relation",
            description: "Remove a relation between two issues by its relation ID.",
            scope: "issues:write",
            rest_method: Method::DELETE,
            rest_path: "/relations/{id}",
            json_schema: || schemars::schema_for!(RemoveRelationApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: RemoveRelationApiParams = serde_json::from_value(value)?;
                    remove_relation(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "list_issue_relations",
            description: "List all relations for an issue (both directions — blocks and blocked-by).",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/issues/{identifier}/relations",
            json_schema: || schemars::schema_for!(ListRelationsApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListRelationsApiParams = serde_json::from_value(value)?;
                    list_relations(&ctx, params).await
                })
            }),
        },
    ]
}
