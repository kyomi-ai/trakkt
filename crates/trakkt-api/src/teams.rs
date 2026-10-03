// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;

use trakkt_auth::team_service;
use trakkt_types::api::{ListTeamsApiParams, UpdateTeamSettingsApiParams};

use crate::context::resolve_team;
use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

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

/// Update a team's settings (estimate scale, auto-archive, etc.).
pub async fn update_team_settings(
    ctx: &ApiCtx<'_>,
    params: UpdateTeamSettingsApiParams,
) -> ApiResult<serde_json::Value> {
    let team_id = resolve_team(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
        params.team_id.as_deref(),
    )
    .await?
    .ok_or_else(|| ApiError::BadRequest("team_key or team_id is required".to_string()))?;

    team_service::update_team_settings(
        ctx.db,
        &team_id,
        &ctx.workspace_id,
        &params.settings,
        ctx.ws_manager,
    )
    .await?;

    // Re-fetch the team to return the updated state. Scoped to the workspace:
    // the UPDATE above is, so a team from elsewhere leaves it a no-op, and an
    // unscoped read here would hand that team's row back regardless.
    let team = team_service::get_team_in_workspace(ctx.db, &team_id, &ctx.workspace_id).await?;

    Ok(serde_json::to_value(&team)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all team-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
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
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "update_team_settings",
            description:
                "Update a team's settings including estimation scale, auto-archive, and other configuration. Provide team_key or team_id to identify the team.",
            scope: "teams:write",
            rest_method: Method::PATCH,
            rest_path: "/teams/{identifier}/settings",
            json_schema: || schemars::schema_for!(UpdateTeamSettingsApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: UpdateTeamSettingsApiParams = serde_json::from_value(value)?;
                    update_team_settings(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
    ]
}
