// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for project CRUD operations.
//!
//! Thin wrappers around `trakkt_auth::project_service` — extract auth,
//! call service, return. No business logic lives here.
//!
//! Leptos server functions receive each field as a separate parameter
//! (no struct grouping), so many-argument signatures are unavoidable.
//! The workspace-root `clippy.toml` raises the threshold to 14.

use leptos::prelude::*;
use trakkt_types::models::{Project, ProjectMember, ProjectMilestone, ProjectProgress, ProjectUpdate};

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all projects in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_projects() -> Result<Vec<Project>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let projects = trakkt_auth::project_service::list_projects(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(projects)
}

/// Get a single project by its ID.
#[server(prefix = "/leptos-api")]
pub async fn get_project(project_id: String) -> Result<Option<Project>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let project = trakkt_auth::project_service::get_project(ac.db(), &project_id)
        .await
        .into_sfn()?;

    // Verify the project belongs to this workspace.
    if let Some(ref p) = project
        && p.workspace_id != ac.ws_id
    {
        return Ok(None);
    }

    Ok(project)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Create a new project in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn create_project(
    name: String,
    description: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    target_date: Option<String>,
) -> Result<Project, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let project = trakkt_auth::project_service::create_project(
        ac.db(),
        &trakkt_auth::project_service::CreateProjectParams {
            workspace_id: &ac.ws_id,
            name: &name,
            description: description.as_deref(),
            icon: icon.as_deref(),
            color: color.as_deref(),
            lead_id: lead_id.as_deref(),
            start_date: start_date.as_deref(),
            target_date: target_date.as_deref(),
        },
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(project)
}

/// Update fields on an existing project.
///
/// For clearable fields (lead_id, start_date, target_date), use sentinel values:
/// - `None` = no change
/// - `Some("")` = clear the field (set to NULL)
/// - `Some("value")` = set to new value
#[server(prefix = "/leptos-api")]
pub async fn update_project(
    project_id: String,
    name: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    status: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    target_date: Option<String>,
) -> Result<Project, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;

    let project = trakkt_auth::project_service::update_project(
        ac.db(),
        &trakkt_auth::project_service::UpdateProjectParams {
            project_id: &project_id,
            name: name.as_deref(),
            description: description.as_deref(),
            icon: icon.as_deref(),
            color: color.as_deref(),
            status: status.as_deref(),
            lead_id: lead_id.as_ref().map(|s| if s.is_empty() { None } else { Some(s.as_str()) }),
            start_date: start_date.as_ref().map(|s| if s.is_empty() { None } else { Some(s.as_str()) }),
            target_date: target_date.as_ref().map(|s| if s.is_empty() { None } else { Some(s.as_str()) }),
        },
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(project)
}

/// Delete a project by its ID.
#[server(prefix = "/leptos-api")]
pub async fn delete_project(project_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    trakkt_auth::project_service::delete_project(ac.db(), &project_id, ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;
    Ok(())
}

// ─── Members ───────────────────────────────────────────────────────────────

/// List all members of a project.
#[server(prefix = "/leptos-api")]
pub async fn list_project_members(project_id: String) -> Result<Vec<ProjectMember>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let members = trakkt_auth::project_service::list_project_members(ac.db(), &project_id)
        .await
        .into_sfn()?;
    Ok(members)
}

/// Add a user to a project.
#[server(prefix = "/leptos-api")]
pub async fn add_project_member(
    project_id: String,
    user_id: String,
    role: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    trakkt_auth::project_service::add_project_member(
        ac.db(),
        &project_id,
        &user_id,
        &role,
        &ac.ws_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Remove a user from a project.
#[server(prefix = "/leptos-api")]
pub async fn remove_project_member(
    project_id: String,
    user_id: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    trakkt_auth::project_service::remove_project_member(
        ac.db(),
        &project_id,
        &user_id,
        &ac.ws_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

// ─── Milestones ─────────────────────────────────────────────────────────────

/// List all milestones in a project.
#[server(prefix = "/leptos-api")]
pub async fn list_milestones(project_id: String) -> Result<Vec<ProjectMilestone>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let milestones = trakkt_auth::project_service::list_milestones(ac.db(), &project_id)
        .await
        .into_sfn()?;
    Ok(milestones)
}

/// Create a new milestone in a project.
#[server(prefix = "/leptos-api")]
pub async fn create_milestone(
    project_id: String,
    name: String,
    description: Option<String>,
    target_date: Option<String>,
) -> Result<ProjectMilestone, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let milestone = trakkt_auth::project_service::create_milestone(
        ac.db(),
        &project_id,
        &name,
        description.as_deref(),
        target_date.as_deref(),
        ac.ctx.ws_manager.as_ref(),
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(milestone)
}

/// Update fields on an existing milestone.
///
/// For `target_date`, uses the same sentinel pattern as `update_project`:
/// - `None` = no change
/// - `Some("")` = clear the field (set to NULL)
/// - `Some("value")` = set to new value
#[server(prefix = "/leptos-api")]
pub async fn update_milestone(
    milestone_id: String,
    name: Option<String>,
    description: Option<String>,
    target_date: Option<String>,
) -> Result<ProjectMilestone, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let project_id = resolve_milestone_project(ac.db(), &milestone_id).await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let milestone = trakkt_auth::project_service::update_milestone(
        ac.db(),
        &milestone_id,
        name.as_deref(),
        description.as_deref(),
        target_date
            .as_ref()
            .map(|s| if s.is_empty() { None } else { Some(s.as_str()) }),
        ac.ctx.ws_manager.as_ref(),
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(milestone)
}

/// Delete a milestone by its ID.
#[server(prefix = "/leptos-api")]
pub async fn delete_milestone(milestone_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let project_id = resolve_milestone_project(ac.db(), &milestone_id).await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    trakkt_auth::project_service::delete_milestone(
        ac.db(),
        &milestone_id,
        ac.ctx.ws_manager.as_ref(),
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(())
}

// ─── Project Updates ──────────────────────────────────────────────────────

/// List all status updates for a project, newest first.
#[server(prefix = "/leptos-api")]
pub async fn list_project_updates(
    project_id: String,
) -> Result<Vec<ProjectUpdate>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let updates = trakkt_auth::project_service::list_project_updates(ac.db(), &project_id)
        .await
        .into_sfn()?;
    Ok(updates)
}

/// Create a new status update on a project.
#[server(prefix = "/leptos-api")]
pub async fn create_project_update(
    project_id: String,
    health: String,
    body: Option<String>,
) -> Result<ProjectUpdate, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let update = trakkt_auth::project_service::create_project_update(
        ac.db(),
        &project_id,
        &ac.auth.user_id,
        &health,
        body.as_deref(),
        ac.ctx.ws_manager.as_ref(),
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(update)
}

// ─── Project Progress ─────────────────────────────────────────────────────

/// Get issue progress stats for a project.
#[server(prefix = "/leptos-api")]
pub async fn get_project_progress(
    project_id: String,
) -> Result<ProjectProgress, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_project_ownership(ac.db(), &ac.ws_id, &project_id).await?;
    let progress = trakkt_auth::project_service::get_project_progress(ac.db(), &project_id)
        .await
        .into_sfn()?;
    Ok(progress)
}

// ─── Helpers (server-only) ─────────────────────────────────────────────────

/// Resolve the owning project_id for a milestone.
#[cfg(feature = "ssr")]
async fn resolve_milestone_project(
    db: &trakkt_core::DbPool,
    milestone_id: &str,
) -> Result<String, ServerFnError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        project_id: String,
    }
    let row: Option<Row> = trakkt_core::db_fetch_optional!(
        db,
        Row,
        "SELECT project_id FROM project_milestones WHERE milestone_id = $1",
        milestone_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    row.map(|r| r.project_id)
        .ok_or_else(|| ServerFnError::new("Milestone not found"))
}

/// Verify that a project belongs to the user's current workspace.
#[cfg(feature = "ssr")]
async fn verify_project_ownership(
    db: &trakkt_core::DbPool,
    workspace_id: &str,
    project_id: &str,
) -> Result<(), ServerFnError> {
    use super::IntoServerFnError;
    let project = trakkt_auth::project_service::get_project(db, project_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Project not found"))?;
    if project.workspace_id != workspace_id {
        return Err(ServerFnError::new("Project not found"));
    }
    Ok(())
}
