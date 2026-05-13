// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Profile settings page.
//!
//! These replace the REST API calls that ProfileSettings.jsx makes.
//! Each function calls the same service-layer code as the existing REST routes.

use leptos::prelude::*;

use crate::types::{InvitationData, ProfileData};

// ─────────────────────────────────────────────────────────────────────────────
// Read operations (called on page load via Resource)
// ─────────────────────────────────────────────────────────────────────────────

/// Load the current user's profile data.
///
/// Combines user info, preferences, and system config into a single
/// response — replacing multiple separate REST calls.
#[server(prefix = "/leptos-api")]
pub async fn get_profile() -> Result<ProfileData, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let user = trakkt_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let metadata = user.extra_metadata.as_ref().and_then(|v| v.as_object());

    let theme = metadata
        .and_then(|m| m.get("theme"))
        .and_then(|v| v.as_str())
        .unwrap_or("system")
        .to_string();

    let landing_page = metadata
        .and_then(|m| m.get("landing_page"))
        .and_then(|v| v.as_str())
        .unwrap_or("chat")
        .to_string();

    Ok(ProfileData {
        user_id: user.user_id,
        email: user.email,
        name: user.name,
        theme,
        landing_page,
        is_personal_mode: ctx.config.is_personal(),
        is_self_hosted: ctx.config.self_hosted,
    })
}

/// Load pending workspace invitations for the current user.
#[server(prefix = "/leptos-api")]
pub async fn get_pending_invitations() -> Result<Vec<InvitationData>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let invitations =
        trakkt_auth::workspace_service::get_pending_invitations_for_email(&ctx.db, &auth.email)
            .await
            .into_sfn()?;

    Ok(invitations
        .into_iter()
        .map(|inv| InvitationData {
            invitation_id: inv.invitation_id,
            workspace_id: inv.workspace_id,
            email: inv.email,
            role: inv.role.to_string(),
            created_at: inv.created_at.to_rfc3339(),
            expires_at: inv.expires_at.to_rfc3339(),
        })
        .collect())
}

// ─────────────────────────────────────────────────────────────────────────────
// Write operations (called on user interaction via Action)
// ─────────────────────────────────────────────────────────────────────────────

/// Update the user's display name.
#[server(prefix = "/leptos-api")]
pub async fn update_profile_name(name: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("Name cannot be empty"));
    }

    trakkt_auth::user_service::update_user_name(&ctx.db, &auth.user_id, &trimmed)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update user theme preference (light, dark, or system).
#[server(prefix = "/leptos-api")]
pub async fn update_theme(theme: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    if !["light", "dark", "system"].contains(&theme.as_str()) {
        return Err(ServerFnError::new("Theme must be light, dark, or system"));
    }

    let metadata = serde_json::json!({ "theme": theme });
    trakkt_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &metadata)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the user's landing page preference.
#[server(prefix = "/leptos-api")]
pub async fn update_landing_page(page: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let valid = ["issues", "settings"];
    if !valid.contains(&page.as_str()) {
        return Err(ServerFnError::new(
            "Invalid landing_page. Must be 'issues' or 'settings'.",
        ));
    }

    let metadata = serde_json::json!({ "landing_page": page });
    trakkt_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &metadata)
        .await
        .into_sfn()?;

    Ok(())
}

/// Accept a workspace invitation.
#[server(prefix = "/leptos-api")]
pub async fn accept_invitation(invitation_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    trakkt_auth::workspace_service::accept_invitation_for_user(
        &ctx.db,
        &invitation_id,
        &auth.user_id,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Decline a workspace invitation.
#[server(prefix = "/leptos-api")]
pub async fn decline_invitation(invitation_id: String) -> Result<(), ServerFnError> {
    let _auth = extract_auth().await?; // verify authenticated
    let ctx = extract_context()?;

    trakkt_auth::workspace_service::update_invitation_status(
        &ctx.db,
        &invitation_id,
        "declined",
    )
    .await
    .into_sfn()?;

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, IntoServerFnError};

