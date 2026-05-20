// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue-tracker team operations.
//!
//! Thin wrappers around `trakkt_auth::team_service` — extract auth,
//! call service, return.
//!
//! Note: these are *issue-tracker* teams (e.g. "Engineering", "Design"),
//! not the workspace membership team managed by `server_fns::team`.

use leptos::prelude::*;
use trakkt_types::models::Team;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List issue-tracker teams the current user belongs to.
#[server(prefix = "/leptos-api")]
pub async fn list_teams() -> Result<Vec<Team>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListTeamsApiParams {};
    let result = trakkt_api::teams::list_teams(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// List all issue-tracker teams in the workspace (for admin settings).
#[server(prefix = "/leptos-api")]
pub async fn list_all_teams() -> Result<Vec<Team>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let teams = trakkt_auth::team_service::list_teams(ac.db(), &ac.ws_id, None)
        .await
        .into_sfn()?;
    Ok(teams)
}

/// List issue-tracker teams the current user can join (not yet a member of).
#[server(prefix = "/leptos-api")]
pub async fn list_joinable_teams() -> Result<Vec<Team>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let teams = trakkt_auth::team_service::list_joinable_teams(ac.db(), &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(teams)
}

/// Get the default team for the current user in the current workspace.
///
/// Uses three-tier resolution: user's personal default, workspace default,
/// then first-created team as fallback.
#[server(prefix = "/leptos-api")]
pub async fn get_default_team() -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::get_user_default_team(ac.db(), &ac.auth.user_id, &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(team)
}

/// Get an issue-tracker team by its short key (e.g. "ENG").
#[server(prefix = "/leptos-api")]
pub async fn get_team_by_key(key: String) -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::get_team_by_key(ac.db(), &ac.ws_id, &key)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Team with key '{key}' not found")))?;
    Ok(team)
}

/// Join a team as a member. No-op if already a member.
#[server(prefix = "/leptos-api")]
pub async fn join_team(team_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::team_service::add_team_member(
        ac.db(),
        &team_id,
        &ac.auth.user_id,
        "member",
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Leave a team. Removes the current user from the team's membership.
#[server(prefix = "/leptos-api")]
pub async fn leave_team(team_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::team_service::remove_team_member(
        ac.db(),
        &team_id,
        &ac.auth.user_id,
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Get the workspace-level default team ID (not user-resolved).
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_default_team_id() -> Result<Option<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let id = trakkt_auth::workspace_service::get_workspace_default_team_id(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(id)
}

/// Get the current user's personal default team ID (raw, not resolved).
#[server(prefix = "/leptos-api")]
pub async fn get_my_default_team_id() -> Result<Option<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let user = trakkt_auth::user_service::get_user_by_id(ac.db(), &ac.auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("User not found"))?;
    Ok(user.default_team_id)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Update an issue-tracker team's name and/or key.
#[server(prefix = "/leptos-api")]
pub async fn update_team(
    team_id: String,
    name: Option<String>,
    key: Option<String>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::team_service::update_team(
        ac.db(),
        &team_id,
        &ac.ws_id,
        name,
        key,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Delete a team, optionally reassigning its issues to another team.
#[server(prefix = "/leptos-api")]
pub async fn delete_team(
    team_id: String,
    reassign_to_team_id: Option<String>,
    new_workspace_default_id: Option<String>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::team_service::delete_team(
        ac.db(),
        &team_id,
        &ac.ws_id,
        reassign_to_team_id.as_deref(),
        new_workspace_default_id.as_deref(),
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Set (or clear) the current user's personal default team.
///
/// Pass `Some(team_id)` to set a personal default, or `None` to clear it
/// and fall back to the workspace default.
#[server(prefix = "/leptos-api")]
pub async fn set_my_default_team(team_id: Option<String>) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    if let Some(ref tid) = team_id {
        let team = trakkt_auth::team_service::get_team(ac.db(), tid)
            .await
            .into_sfn()?
            .ok_or_else(|| ServerFnError::new("Team not found"))?;
        if team.workspace_id != ac.ws_id {
            return Err(ServerFnError::new("Team does not belong to this workspace"));
        }
    }
    trakkt_auth::user_service::update_default_team(
        ac.db(),
        &ac.auth.user_id,
        team_id.as_deref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Set the workspace-level default team.
#[server(prefix = "/leptos-api")]
pub async fn set_workspace_default_team(team_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::workspace_service::set_workspace_default_team(
        ac.db(),
        &ac.ws_id,
        &team_id,
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Update a team's icon to a preset icon (or clear it by passing all None).
#[server(prefix = "/leptos-api")]
pub async fn update_team_icon(
    team_id: String,
    icon_type: Option<String>,
    icon_name: Option<String>,
    icon_color: Option<String>,
) -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::update_team_icon(
        ac.db(),
        &team_id,
        &ac.ws_id,
        icon_type.as_deref(),
        icon_name.as_deref(),
        icon_color.as_deref(),
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(team)
}

/// Clear a team's icon entirely (removes both preset and custom icons).
#[server(prefix = "/leptos-api")]
pub async fn clear_team_icon(
    team_id: String,
) -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::delete_team_icon(
        ac.db(),
        &team_id,
        &ac.ws_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(team)
}

/// Update a team's settings (estimate scale, toggles, etc.).
///
/// Accepts the settings as a JSON string because Leptos server functions
/// cannot deserialize complex nested enums directly from URL-encoded form data.
#[server(prefix = "/leptos-api")]
pub async fn update_team_settings(
    team_id: String,
    settings_json: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let settings: trakkt_types::models::TeamSettings =
        serde_json::from_str(&settings_json).map_err(|e| {
            tracing::warn!(error = %e, "Failed to parse team settings JSON");
            ServerFnError::new(format!("Invalid settings: {e}"))
        })?;
    trakkt_auth::team_service::update_team_settings(
        ac.db(),
        &team_id,
        &ac.ws_id,
        &settings,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

/// Create a new issue-tracker team in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn create_team(
    name: String,
    key: String,
    description: Option<String>,
    icon: Option<String>,
) -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::create_team(
        ac.db(),
        &trakkt_auth::team_service::CreateTeamParams {
            workspace_id: &ac.ws_id,
            name: &name,
            key: &key,
            description: description.as_deref(),
            icon: icon.as_deref(),
            creator_id: Some(&ac.auth.user_id),
        },
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(team)
}
