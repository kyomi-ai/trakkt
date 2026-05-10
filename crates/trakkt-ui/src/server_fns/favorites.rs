// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for favorite CRUD operations.
//!
//! Thin wrappers around `trakkt_auth::favorite_service` — extract auth,
//! call service, return. No business logic lives here.

use leptos::prelude::*;
use trakkt_types::models::Favorite;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all favorites for the current user in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_favorites() -> Result<Vec<Favorite>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let favorites = trakkt_auth::favorite_service::list_favorites(ac.db(), &ac.auth.user_id, &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(favorites)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Add a favorite (team, project, or view) for the current user.
#[server(prefix = "/leptos-api")]
pub async fn add_favorite(
    target_type: String,
    target_id: String,
) -> Result<Favorite, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let favorite = trakkt_auth::favorite_service::add_favorite(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        &target_type,
        &target_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(favorite)
}

/// Remove a favorite by target type and ID.
#[server(prefix = "/leptos-api")]
pub async fn remove_favorite(
    target_type: String,
    target_id: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::favorite_service::remove_favorite(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        &target_type,
        &target_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}
