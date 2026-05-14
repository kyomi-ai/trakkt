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
    let teams = trakkt_auth::team_service::list_teams(ac.db(), &ac.ws_id, Some(&ac.auth.user_id))
        .await
        .into_sfn()?;
    Ok(teams)
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

/// Set the current user's personal default team.
#[server(prefix = "/leptos-api")]
pub async fn set_my_default_team(team_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::get_team(ac.db(), &team_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Team not found"))?;
    if team.workspace_id != ac.ws_id {
        return Err(ServerFnError::new("Team does not belong to this workspace"));
    }
    trakkt_auth::user_service::update_default_team(
        ac.db(),
        &ac.auth.user_id,
        Some(&team_id),
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
