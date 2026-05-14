// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Logic is ported line-by-line from the MCP
//! tool handlers in `routes/mcp.rs` to eliminate duplication.

use axum::http::Method;
use serde_json::json;

use trakkt_auth::project_service;
use trakkt_types::api::{
    CreateProjectApiParams, DeleteProjectApiParams, GetProjectApiParams, ListProjectsApiParams,
    UpdateProjectApiParams,
};

use super::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Verify a project belongs to the given workspace, returning the project.
///
/// Ported from `verify_project_ownership` in `routes/mcp.rs`.
pub async fn verify_project_ownership(
    ctx: &ApiCtx<'_>,
    project_id: &str,
) -> ApiResult<trakkt_types::models::Project> {
    let project = project_service::get_project(ctx.db, project_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Project not found".into()))?;
    if project.workspace_id != ctx.workspace_id {
        return Err(ApiError::NotFound("Project not found".into()));
    }
    Ok(project)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all projects in the workspace.
///
/// Ported from `tool_list_projects` in `routes/mcp.rs`.
pub async fn list_projects(
    ctx: &ApiCtx<'_>,
    _params: ListProjectsApiParams,
) -> ApiResult<serde_json::Value> {
    let projects = project_service::list_projects(ctx.db, &ctx.workspace_id).await?;
    Ok(serde_json::to_value(&projects)?)
}

/// Get a single project by its ID, including milestones.
///
/// Ported from `tool_get_project` in `routes/mcp.rs`.
pub async fn get_project(
    ctx: &ApiCtx<'_>,
    params: GetProjectApiParams,
) -> ApiResult<serde_json::Value> {
    let project = verify_project_ownership(ctx, &params.project_id).await?;
    let milestones = project_service::list_milestones(ctx.db, &params.project_id).await?;
    let result = json!({ "project": project, "milestones": milestones });
    Ok(result)
}

/// Create a new project in the workspace.
///
/// Ported from `tool_create_project` in `routes/mcp.rs`.
pub async fn create_project(
    ctx: &ApiCtx<'_>,
    params: CreateProjectApiParams,
) -> ApiResult<serde_json::Value> {
    let create_params = project_service::CreateProjectParams {
        workspace_id: &ctx.workspace_id,
        name: &params.name,
        description: params.description.as_deref(),
        icon: params.icon.as_deref(),
        color: params.color.as_deref(),
        lead_id: params.lead_id.as_deref(),
        start_date: params.start_date.as_deref(),
        target_date: params.target_date.as_deref(),
    };

    let project =
        project_service::create_project(ctx.db, &create_params, Some(ctx.ws_manager)).await?;

    Ok(serde_json::to_value(&project)?)
}

/// Update fields on an existing project.
///
/// Has double-Option handling for lead_id, start_date, and target_date:
/// - Field absent from JSON = no change (`None`)
/// - Field set to `null` = clear the field (`Some(None)`)
/// - Field set to a value = update the field (`Some(Some(value))`)
///
/// Ported from `tool_update_project` in `routes/mcp.rs`.
pub async fn update_project(
    ctx: &ApiCtx<'_>,
    params: UpdateProjectApiParams,
) -> ApiResult<serde_json::Value> {
    let project_id = params.project_id.as_deref().ok_or_else(|| {
        ApiError::BadRequest("project_id is required".to_string())
    })?;
    verify_project_ownership(ctx, project_id).await?;

    let update_params = project_service::UpdateProjectParams {
        project_id,
        name: params.name.as_deref(),
        description: params.description.as_deref(),
        icon: params.icon.as_deref(),
        color: params.color.as_deref(),
        status: params.status.as_deref(),
        lead_id: params.lead_id.as_ref().map(|opt| opt.as_deref()),
        start_date: params.start_date.as_ref().map(|opt| opt.as_deref()),
        target_date: params.target_date.as_ref().map(|opt| opt.as_deref()),
    };

    let project =
        project_service::update_project(ctx.db, &update_params, Some(ctx.ws_manager)).await?;

    Ok(serde_json::to_value(&project)?)
}

/// Permanently delete a project and its milestones.
///
/// Ported from `tool_delete_project` in `routes/mcp.rs`.
pub async fn delete_project(
    ctx: &ApiCtx<'_>,
    params: DeleteProjectApiParams,
) -> ApiResult<serde_json::Value> {
    verify_project_ownership(ctx, &params.project_id).await?;

    project_service::delete_project(ctx.db, &params.project_id, Some(ctx.ws_manager)).await?;

    Ok(json!({ "message": format!("Project {} deleted", params.project_id) }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all project-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_projects",
            description: "List all projects in the workspace.",
            scope: "projects:read",
            rest_method: Method::GET,
            rest_path: "/projects",
            json_schema: || schemars::schema_for!(ListProjectsApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListProjectsApiParams = serde_json::from_value(value)?;
                    list_projects(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "get_project",
            description: "Get a single project by its ID, including milestones.",
            scope: "projects:read",
            rest_method: Method::GET,
            rest_path: "/projects/{id}",
            json_schema: || schemars::schema_for!(GetProjectApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: GetProjectApiParams = serde_json::from_value(value)?;
                    get_project(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "create_project",
            description: "Create a new project in the workspace.",
            scope: "projects:write",
            rest_method: Method::POST,
            rest_path: "/projects",
            json_schema: || schemars::schema_for!(CreateProjectApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: CreateProjectApiParams = serde_json::from_value(value)?;
                    create_project(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "update_project",
            description: "Update fields on an existing project. Only provided fields are changed.",
            scope: "projects:write",
            rest_method: Method::PATCH,
            rest_path: "/projects/{id}",
            json_schema: || schemars::schema_for!(UpdateProjectApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: UpdateProjectApiParams = serde_json::from_value(value)?;
                    update_project(&ctx, params).await
                })
            }),
        },
        ApiOperation {
            name: "delete_project",
            description: "Permanently delete a project and its milestones.",
            scope: "projects:write",
            rest_method: Method::DELETE,
            rest_path: "/projects/{id}",
            json_schema: || schemars::schema_for!(DeleteProjectApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: DeleteProjectApiParams = serde_json::from_value(value)?;
                    delete_project(&ctx, params).await
                })
            }),
        },
    ]
}
