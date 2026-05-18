// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relation operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::activity_service::{ActivityRecorder, IssueSnapshot};
use trakkt_auth::{issue_service, relation_service, status_service};
use trakkt_types::models::IssueUpdate;
use trakkt_types::api::{AddRelationApiParams, ListRelationsApiParams, RemoveRelationApiParams};

use crate::context::{parse_issue_identifier, resolve_issue_key_and_number};
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Add a relation between two issues.
///
/// Supports 'blocks' (source blocks target), 'parent' (source is parent of
/// target), and 'duplicate' (source is duplicate of target) relation types.
/// Both source and target are resolved from compound identifiers like 'TRA-35'.
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

    // Record activity on both issues — never fails the mutation.
    let recorder = ActivityRecorder::new(ctx.db, &ctx.workspace_id, &ctx.user_id, ctx.ws_manager);

    let source_identifier = format!("{source_key}-{source_num}");
    let target_identifier = format!("{target_key}-{target_num}");

    let source_meta = json!({
        "relation_type": params.relation_type,
        "direction": "outward",
        "related_issue_id": target_issue.issue_id,
        "related_identifier": target_identifier,
        "related_title": target_issue.title,
    });
    if let Err(e) = recorder
        .record(&source_issue.issue_id, "relation_added", Some(&source_meta))
        .await
    {
        tracing::warn!(issue_id = %source_issue.issue_id, "Failed to record relation activity on source: {e}");
    }

    let target_meta = json!({
        "relation_type": params.relation_type,
        "direction": "inward",
        "related_issue_id": source_issue.issue_id,
        "related_identifier": source_identifier,
        "related_title": source_issue.title,
    });
    if let Err(e) = recorder
        .record(&target_issue.issue_id, "relation_added", Some(&target_meta))
        .await
    {
        tracing::warn!(issue_id = %target_issue.issue_id, "Failed to record relation activity on target: {e}");
    }

    // Auto-close: when a duplicate relation is created, transition the source
    // (duplicate) issue to the first global status in the "cancelled" category.
    // Best-effort — don't fail the relation creation if this errors.
    if params.relation_type == "duplicate" && source_issue.status_category != "cancelled" {
        match status_service::get_status_by_category(ctx.db, &ctx.workspace_id, "cancelled").await {
            Ok(Some(cancelled_status)) => {
                let before = IssueSnapshot::from_issue_with_details(&source_issue);
                let update = IssueUpdate {
                    status_id: Some(cancelled_status.status_id),
                    ..Default::default()
                };
                match issue_service::update_issue(
                    ctx.db,
                    &ctx.workspace_id,
                    &source_key,
                    source_num,
                    &update,
                    Some(&ctx.user_id),
                    ctx.ws_manager,
                )
                .await
                {
                    Ok(_) => {
                        match issue_service::get_issue(
                            ctx.db, &ctx.workspace_id, &source_key, source_num
                        ).await {
                            Ok(Some(updated)) => {
                                let after = IssueSnapshot::from_issue_with_details(&updated);
                                if let Err(e) = recorder.record_issue_diff(
                                    &source_issue.issue_id, &before, &after
                                ).await {
                                    tracing::warn!(
                                        issue_id = %source_issue.issue_id,
                                        "Failed to record auto-close activity: {e}"
                                    );
                                }
                            }
                            Ok(None) => {
                                tracing::warn!(
                                    issue_id = %source_issue.issue_id,
                                    "Re-fetch after auto-close returned None"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    issue_id = %source_issue.issue_id,
                                    "Failed to re-fetch issue after auto-close: {e}"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            issue_id = %source_issue.issue_id,
                            "Failed to auto-close duplicate issue: {e}"
                        );
                    }
                }
            }
            Ok(None) => {
                tracing::warn!(
                    workspace_id = %ctx.workspace_id,
                    "No cancelled status found — skipping auto-close for duplicate"
                );
            }
            Err(e) => {
                tracing::warn!(
                    workspace_id = %ctx.workspace_id,
                    "Failed to look up cancelled status: {e}"
                );
            }
        }
    }

    Ok(serde_json::to_value(&relation)?)
}

/// Remove a relation between two issues by its relation ID.
///
/// Ported from `tool_remove_relation` in `routes/mcp.rs`.
pub async fn remove_relation(
    ctx: &ApiCtx<'_>,
    params: RemoveRelationApiParams,
) -> ApiResult<serde_json::Value> {
    // Fetch the relation before deletion so we can record activity on both issues.
    let relation_before =
        relation_service::get_relation_by_id(ctx.db, &params.relation_id, &ctx.workspace_id)
            .await;

    relation_service::delete_relation(
        ctx.db,
        &params.relation_id,
        &ctx.workspace_id,
        ctx.ws_manager,
    )
    .await?;

    // Record activity on both issues — never fails the mutation.
    // Look up both issues to get their identifiers for rich activity metadata
    // (matches the shape of relation_added metadata).
    let recorder = ActivityRecorder::new(ctx.db, &ctx.workspace_id, &ctx.user_id, ctx.ws_manager);
    match relation_before {
        Ok(Some(rel)) => {
            let source_issue = issue_service::get_issue_by_id(ctx.db, &rel.source_issue_id).await;
            let target_issue = issue_service::get_issue_by_id(ctx.db, &rel.target_issue_id).await;

            let source_identifier = source_issue.as_ref().ok().and_then(|o| o.as_ref())
                .map(|i| format!("{}-{}", i.team_key, i.number));
            let target_identifier = target_issue.as_ref().ok().and_then(|o| o.as_ref())
                .map(|i| format!("{}-{}", i.team_key, i.number));

            let source_meta = json!({
                "relation_id": params.relation_id,
                "relation_type": rel.relation_type,
                "direction": "outward",
                "related_issue_id": rel.target_issue_id,
                "related_identifier": target_identifier,
                "related_title": target_issue.ok().flatten().map(|i| i.title),
            });
            if let Err(e) = recorder
                .record(&rel.source_issue_id, "relation_removed", Some(&source_meta))
                .await
            {
                tracing::warn!(relation_id = %params.relation_id, "Failed to record relation_removed on source: {e}");
            }

            let target_meta = json!({
                "relation_id": params.relation_id,
                "relation_type": rel.relation_type,
                "direction": "inward",
                "related_issue_id": rel.source_issue_id,
                "related_identifier": source_identifier,
                "related_title": source_issue.ok().flatten().map(|i| i.title),
            });
            if let Err(e) = recorder
                .record(&rel.target_issue_id, "relation_removed", Some(&target_meta))
                .await
            {
                tracing::warn!(relation_id = %params.relation_id, "Failed to record relation_removed on target: {e}");
            }
        }
        Ok(None) => {
            tracing::warn!(relation_id = %params.relation_id, "relation_removed: pre-fetch returned None");
        }
        Err(e) => {
            tracing::warn!(relation_id = %params.relation_id, error = %e, "relation_removed: pre-fetch failed, activity not recorded");
        }
    }

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
            description: "Add a relation between two issues. Supports 'blocks' (source blocks target), 'parent' (source is parent of target), and 'duplicate' (source is duplicate of target) relation types.",
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
            binary_input: None,
            binary_output: None,
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
            binary_input: None,
            binary_output: None,
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
            binary_input: None,
            binary_output: None,
        },
    ]
}
