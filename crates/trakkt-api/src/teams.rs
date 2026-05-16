// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;

use trakkt_auth::team_service;
use trakkt_types::api::ListTeamsApiParams;

use crate::{ApiCtx, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List teams the authenticated user belongs to, ordered alphabetically by name.
///
/// Ported from `tool_list_teams` in `routes/mcp.rs`.
pub async fn list_teams(
    ctx: &ApiCtx<'_>,
    _params: ListTeamsApiParams,
) -> ApiResult<serde_json::Value> {
    let teams =
        team_service::list_teams(ctx.db, &ctx.workspace_id, Some(&ctx.user_id)).await?;
    Ok(serde_json::to_value(&teams)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all team-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![ApiOperation {
        name: "list_teams",
        description:
            "List teams the authenticated user belongs to, ordered alphabetically by name.",
        scope: "teams:read",
        rest_method: Method::GET,
        rest_path: "/teams",
        json_schema: || schemars::schema_for!(ListTeamsApiParams),
        handler: Box::new(|ctx, value| {
            Box::pin(async move {
                let params: ListTeamsApiParams = serde_json::from_value(value)?;
                list_teams(&ctx, params).await
            })
        }),
    }]
}
