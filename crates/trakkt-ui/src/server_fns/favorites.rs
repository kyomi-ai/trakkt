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

/// Turn the wire string into the closed set the service layer accepts.
///
/// `target_type` arrives from an HTTP request, so this is where an arbitrary
/// string stops. Before TRA-10025 it went straight into `favorites.target_type`,
/// and a row of a type no parent's delete path handles is a favorite that
/// outlives its target forever — cached, re-streamed by every bootstrap, and
/// unremovable through the UI, which only offers a star on types it knows.
///
/// Rejecting is therefore the whole point, and it costs nothing real: every type
/// the product can pin is a [`FavoriteTarget`] variant by construction.
#[cfg(feature = "ssr")]
fn parse_target(
    target_type: &str,
) -> Result<trakkt_types::enums::FavoriteTarget, ServerFnError> {
    trakkt_types::enums::FavoriteTarget::from_wire(target_type).ok_or_else(|| {
        ServerFnError::new(format!(
            "unknown favorite target type {target_type:?} — expected one of {:?}",
            trakkt_types::enums::FavoriteTarget::ALL
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
        ))
    })
}

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

/// Add a favorite for the current user.
///
/// `target_type` must name a [`trakkt_types::enums::FavoriteTarget`]; see
/// [`parse_target`] for why anything else is a 400 rather than a stored row.
#[server(prefix = "/leptos-api")]
pub async fn add_favorite(
    target_type: String,
    target_id: String,
) -> Result<Favorite, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let target = parse_target(&target_type)?;
    let favorite = trakkt_auth::favorite_service::add_favorite(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        target,
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
    let target = parse_target(&target_type)?;
    trakkt_auth::favorite_service::remove_favorite(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        target,
        &target_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(())
}
