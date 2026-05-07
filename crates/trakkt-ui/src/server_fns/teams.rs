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

/// List all issue-tracker teams in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_teams() -> Result<Vec<Team>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let teams = trakkt_auth::team_service::list_teams(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(teams)
}

/// Get the default (first-created) team in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn get_default_team() -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::get_default_team(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(team)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Create a new issue-tracker team in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn create_team(name: String, key: String) -> Result<Team, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let team = trakkt_auth::team_service::create_team(ac.db(), &ac.ws_id, &name, &key, ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;
    Ok(team)
}
