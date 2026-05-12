// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for saved view CRUD operations.
//!
//! Thin wrappers around `trakkt_auth::view_service` — extract auth,
//! call service, return. No business logic lives here.

use leptos::prelude::*;
use trakkt_types::models::View;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all views visible to the current user (own + shared) in the workspace.
///
/// When `team_id` is provided, only views scoped to that team are returned.
#[server(prefix = "/leptos-api")]
pub async fn list_views(team_id: Option<String>) -> Result<Vec<View>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let views = trakkt_auth::view_service::list_views(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        team_id.as_deref(),
    )
    .await
    .into_sfn()?;
    Ok(views)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Create a new saved view in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn create_view(
    name: String,
    icon: Option<String>,
    filters: String,
    display_options: String,
    is_shared: bool,
    team_id: Option<String>,
    position: i32,
) -> Result<View, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let view = trakkt_auth::view_service::create_view(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        &name,
        icon.as_deref(),
        &filters,
        &display_options,
        is_shared,
        team_id.as_deref(),
        position,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(view)
}

/// Update fields on an existing view.
#[server(prefix = "/leptos-api")]
pub async fn update_view(
    view_id: String,
    name: Option<String>,
    icon: Option<String>,
    filters: Option<String>,
    display_options: Option<String>,
    is_shared: Option<bool>,
    sort_order: Option<f64>,
    position: Option<i32>,
) -> Result<View, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_view_ownership(ac.db(), &ac.ws_id, &ac.auth.user_id, &view_id).await?;

    let view = trakkt_auth::view_service::update_view(
        ac.db(),
        &view_id,
        name.as_deref(),
        icon.as_deref(),
        filters.as_deref(),
        display_options.as_deref(),
        is_shared,
        sort_order,
        None, // team_id changes not needed in MVP
        position,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(view)
}

/// Delete a view by its ID.
#[server(prefix = "/leptos-api")]
pub async fn delete_view(view_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_view_ownership(ac.db(), &ac.ws_id, &ac.auth.user_id, &view_id).await?;
    trakkt_auth::view_service::delete_view(
        ac.db(),
        &view_id,
        &ac.ws_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}

// ─── Helpers (server-only) ─────────────────────────────────────────────────

/// Verify that a view belongs to the requesting user in the current workspace.
#[cfg(feature = "ssr")]
async fn verify_view_ownership(
    db: &trakkt_core::DbPool,
    workspace_id: &str,
    user_id: &str,
    view_id: &str,
) -> Result<(), ServerFnError> {
    use super::IntoServerFnError;
    let view = trakkt_auth::view_service::get_view(db, view_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("View not found"))?;
    if view.workspace_id != workspace_id || view.created_by != user_id {
        return Err(ServerFnError::new("View not found"));
    }
    Ok(())
}
