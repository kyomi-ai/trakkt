// SPDX-License-Identifier: AGPL-3.0-or-later

//! Label operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;

use trakkt_auth::label_service;
use trakkt_types::api::{CreateLabelApiParams, ListLabelsApiParams};

use crate::context::resolve_team;
use crate::{ApiCtx, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all labels in the workspace, ordered alphabetically by name.
///
/// Ported from `tool_list_labels` in `routes/mcp.rs`.
pub async fn list_labels(
    ctx: &ApiCtx<'_>,
    _params: ListLabelsApiParams,
) -> ApiResult<serde_json::Value> {
    let labels = label_service::list_labels(ctx.db, &ctx.workspace_id).await?;
    Ok(serde_json::to_value(&labels)?)
}

/// Create a new label in the workspace, optionally scoped to a team.
///
/// Ported from `tool_create_label` in `routes/mcp.rs`. Extended to support
/// team_key/team_id params for creating team-scoped labels.
pub async fn create_label(
    ctx: &ApiCtx<'_>,
    params: CreateLabelApiParams,
) -> ApiResult<serde_json::Value> {
    let team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?;

    let label = label_service::create_label(
        ctx.db,
        &ctx.workspace_id,
        &params.name,
        &params.color,
        team_id.as_deref(),
        ctx.ws_manager,
    )
    .await?;

    Ok(serde_json::to_value(&label)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all label-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_labels",
            description: "List all labels in the workspace, ordered alphabetically by name.",
            scope: "labels:read",
            rest_method: Method::GET,
            rest_path: "/labels",
            json_schema: || schemars::schema_for!(ListLabelsApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListLabelsApiParams = serde_json::from_value(value)?;
                    list_labels(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "create_label",
            description: "Create a new label in the workspace. Optionally scope it to a team by providing team_key or team_id.",
            scope: "labels:write",
            rest_method: Method::POST,
            rest_path: "/labels",
            json_schema: || schemars::schema_for!(CreateLabelApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: CreateLabelApiParams = serde_json::from_value(value)?;
                    create_label(&ctx, params).await
                })
            }),
        },
    ]
}
