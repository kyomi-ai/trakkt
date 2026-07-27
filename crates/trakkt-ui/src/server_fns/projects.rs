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
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListProjectsApiParams {};
    let result = trakkt_api::projects::list_projects(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// Get a single project by its ID.
#[server(prefix = "/leptos-api")]
pub async fn get_project(project_id: String) -> Result<Option<Project>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::GetProjectApiParams { project_id };
    match trakkt_api::projects::get_project(&ctx, params).await {
        Ok(value) => {
            let project: Project = serde_json::from_value(
                value.get("project").cloned().unwrap_or(serde_json::Value::Null)
            ).into_sfn()?;
            Ok(Some(project))
        }
        Err(trakkt_api::ApiError::NotFound(_)) => Ok(None),
        Err(e) => Err(ServerFnError::new(e.to_string())),
    }
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
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::CreateProjectApiParams {
        name, description, icon, color, lead_id, start_date, target_date,
    };
    let result = trakkt_api::projects::create_project(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
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
    let ctx = ac.api_ctx();
    // For clearable fields: empty string = clear, value = set, None = no change
    fn opt_clear(val: Option<String>) -> Option<Option<String>> {
        val.map(|s| if s.is_empty() { None } else { Some(s) })
    }
    let params = trakkt_types::api::UpdateProjectApiParams {
        project_id: Some(project_id), name, description, icon, color, status,
        lead_id: opt_clear(lead_id),
        start_date: opt_clear(start_date),
        target_date: opt_clear(target_date),
        archived_at: None,
    };
    let result = trakkt_api::projects::update_project(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// Archive or unarchive a project.
///
/// When `archive` is `true`, sets `archived_at` to the current timestamp.
/// When `false`, clears `archived_at` (unarchive).
#[server(prefix = "/leptos-api")]
pub async fn archive_project(
    project_id: String,
    archive: bool,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();

    let archived_at_value = if archive {
        Some(Some(chrono::Utc::now().to_rfc3339()))
    } else {
        Some(None)
    };
    let update_params = trakkt_types::api::UpdateProjectApiParams {
        project_id: Some(project_id),
        name: None,
        description: None,
        icon: None,
        color: None,
        status: None,
        lead_id: None,
        start_date: None,
        target_date: None,
        archived_at: archived_at_value,
    };
    trakkt_api::projects::update_project(&ctx, update_params)
        .await
        .into_sfn()?;
    Ok(())
}

/// Delete a project by its ID.
#[server(prefix = "/leptos-api")]
pub async fn delete_project(project_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::DeleteProjectApiParams { project_id };
    trakkt_api::projects::delete_project(&ctx, params).await.into_sfn()?;
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
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListMilestonesApiParams { project_id };
    let result = trakkt_api::milestones::list_milestones(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
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
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::CreateMilestoneApiParams {
        project_id: Some(project_id), name, description, target_date,
    };
    let result = trakkt_api::milestones::create_milestone(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
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
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::UpdateMilestoneApiParams {
        milestone_id: Some(milestone_id), name, description,
        target_date: target_date.map(|s| if s.is_empty() { None } else { Some(s) }),
    };
    let result = trakkt_api::milestones::update_milestone(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// Delete a milestone by its ID.
#[server(prefix = "/leptos-api")]
pub async fn delete_milestone(milestone_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::DeleteMilestoneApiParams { milestone_id };
    trakkt_api::milestones::delete_milestone(&ctx, params).await.into_sfn()?;
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

/// Verify that a project belongs to the user's current workspace.
///
/// This **authorizes**: the `project_id` arrives from the browser, and the
/// answer decides whether the caller's operation proceeds.
///
/// It reaches for the unscoped `project_service::get_project` and does the
/// workspace comparison itself, rather than calling
/// `project_service::get_project_in_workspace`, so that both the missing and
/// the foreign case surface the same `"Project not found"` string to the
/// client — a server-fn error message that names no project id. The comparison
/// below is load-bearing; dropping it turns this into a plain fetch.
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
