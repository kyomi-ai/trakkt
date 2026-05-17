// SPDX-License-Identifier: AGPL-3.0-or-later

//! Status operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;

use trakkt_auth::status_service;
use trakkt_types::api::ListStatusesApiParams;

use crate::context::resolve_team;
use crate::{ApiCtx, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all statuses in the workspace, grouped by category.
///
/// Returns global statuses and optionally team-specific statuses when a team
/// is specified.
///
/// Ported from `tool_list_statuses` in `routes/mcp.rs`.
pub async fn list_statuses(
    ctx: &ApiCtx<'_>,
    params: ListStatusesApiParams,
) -> ApiResult<serde_json::Value> {
    let team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?;

    let statuses =
        status_service::list_statuses(ctx.db, &ctx.workspace_id, team_id.as_deref()).await?;

    Ok(serde_json::to_value(&statuses)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all status-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![ApiOperation {
        name: "list_statuses",
        description: "List all statuses in the workspace, grouped by category (backlog, unstarted, started, completed, cancelled). Returns both global and optionally team-specific statuses.",
        scope: "issues:read",
        rest_method: Method::GET,
        rest_path: "/statuses",
        json_schema: || schemars::schema_for!(ListStatusesApiParams),
        handler: Box::new(|ctx, value| {
            Box::pin(async move {
                let params: ListStatusesApiParams = serde_json::from_value(value)?;
                list_statuses(&ctx, params).await
            })
        }),
        binary_input: None,
        binary_output: None,
    }]
}
