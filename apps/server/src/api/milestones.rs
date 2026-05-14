// SPDX-License-Identifier: AGPL-3.0-or-later

//! Milestone operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::project_service;
use trakkt_types::api::{
    CreateMilestoneApiParams, DeleteMilestoneApiParams, ListMilestonesApiParams,
    UpdateMilestoneApiParams,
};

use super::projects::verify_project_ownership;
use super::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a milestone's project_id from the database.
///
/// Ported from `resolve_milestone_project_id` in `routes/mcp.rs`.
async fn resolve_milestone_project_id(
    ctx: &ApiCtx<'_>,
    milestone_id: &str,
) -> ApiResult<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        project_id: String,
    }
    let row: Option<Row> = trakkt_core::db_fetch_optional!(
        ctx.db,
        Row,
        "SELECT project_id FROM project_milestones WHERE milestone_id = $1",
        milestone_id
    )
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    row.map(|r| r.project_id)
        .ok_or_else(|| ApiError::NotFound("Milestone not found".into()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all milestones in a project.
///
/// Includes project ownership verification to ensure the project belongs to
/// the authenticated workspace.
///
/// Ported from `tool_list_milestones` in `routes/mcp.rs`.
pub async fn list_milestones(
    ctx: &ApiCtx<'_>,
    params: ListMilestonesApiParams,
) -> ApiResult<serde_json::Value> {
    verify_project_ownership(ctx, &params.project_id).await?;

    let milestones = project_service::list_milestones(ctx.db, &params.project_id).await?;
    Ok(serde_json::to_value(&milestones)?)
}

/// Create a new milestone in a project.
///
/// Ported from `tool_create_milestone` in `routes/mcp.rs`.
pub async fn create_milestone(
    ctx: &ApiCtx<'_>,
    params: CreateMilestoneApiParams,
) -> ApiResult<serde_json::Value> {
    let project_id = params.project_id.as_deref().ok_or_else(|| {
        ApiError::BadRequest("project_id is required".to_string())
    })?;
    verify_project_ownership(ctx, project_id).await?;

    let milestone = project_service::create_milestone(
        ctx.db,
        project_id,
        &params.name,
        params.description.as_deref(),
        params.target_date.as_deref(),
        Some(ctx.ws_manager),
        &ctx.workspace_id,
    )
    .await?;

    Ok(serde_json::to_value(&milestone)?)
}

/// Update fields on an existing milestone.
///
/// Has double-Option handling for target_date:
/// - Field absent from JSON = no change (`None`)
/// - Field set to `null` = clear the field (`Some(None)`)
/// - Field set to a value = update the field (`Some(Some(value))`)
///
/// Ported from `tool_update_milestone` in `routes/mcp.rs`.
pub async fn update_milestone(
    ctx: &ApiCtx<'_>,
    params: UpdateMilestoneApiParams,
) -> ApiResult<serde_json::Value> {
    let milestone_id = params.milestone_id.as_deref().ok_or_else(|| {
        ApiError::BadRequest("milestone_id is required".to_string())
    })?;
    // Verify the milestone's project belongs to this workspace.
    let project_id = resolve_milestone_project_id(ctx, milestone_id).await?;
    verify_project_ownership(ctx, &project_id).await?;

    let milestone = project_service::update_milestone(
        ctx.db,
        milestone_id,
        params.name.as_deref(),
        params.description.as_deref(),
        params.target_date.as_ref().map(|opt| opt.as_deref()),
        Some(ctx.ws_manager),
        &ctx.workspace_id,
    )
    .await?;

    Ok(serde_json::to_value(&milestone)?)
}

/// Delete a milestone. Issues linked to this milestone will be unlinked.
///
/// Ported from `tool_delete_milestone` in `routes/mcp.rs`.
pub async fn delete_milestone(
    ctx: &ApiCtx<'_>,
    params: DeleteMilestoneApiParams,
) -> ApiResult<serde_json::Value> {
    // Verify the milestone's project belongs to this workspace.
    let project_id = resolve_milestone_project_id(ctx, &params.milestone_id).await?;
    verify_project_ownership(ctx, &project_id).await?;

    project_service::delete_milestone(
        ctx.db,
        &params.milestone_id,
        Some(ctx.ws_manager),
        &ctx.workspace_id,
    )
    .await?;

    Ok(json!({ "message": format!("Milestone {} deleted", params.milestone_id) }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all milestone-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_milestones",
            description: "List all milestones in a project.",
            scope: "projects:read",
            rest_method: Method::GET,
            rest_path: "/projects/{id}/milestones",
            json_schema: || schemars::schema_for!(ListMilestonesApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListMilestonesApiParams = serde_json::from_value(value)?;
                    list_milestones(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "create_milestone",
            description: "Create a new milestone in a project.",
            scope: "projects:write",
            rest_method: Method::POST,
            rest_path: "/projects/{id}/milestones",
            json_schema: || schemars::schema_for!(CreateMilestoneApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: CreateMilestoneApiParams = serde_json::from_value(value)?;
                    create_milestone(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "update_milestone",
            description: "Update fields on an existing milestone.",
            scope: "projects:write",
            rest_method: Method::PATCH,
            rest_path: "/milestones/{id}",
            json_schema: || schemars::schema_for!(UpdateMilestoneApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: UpdateMilestoneApiParams = serde_json::from_value(value)?;
                    update_milestone(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "delete_milestone",
            description: "Delete a milestone. Issues linked to this milestone will be unlinked.",
            scope: "projects:write",
            rest_method: Method::DELETE,
            rest_path: "/milestones/{id}",
            json_schema: || schemars::schema_for!(DeleteMilestoneApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: DeleteMilestoneApiParams = serde_json::from_value(value)?;
                    delete_milestone(&ctx, params).await
                })
            }),
        },
    ]
}
