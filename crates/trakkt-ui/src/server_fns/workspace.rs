// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Workspace settings page.
//!
//! These replace the REST API calls that WorkspaceSettings.jsx makes
//! to `/api/v1/workspaces/*` endpoints. Each function calls the same
//! service-layer code as the existing REST route handlers in
//! `apps/server/src/routes/workspaces.rs`.

use leptos::prelude::*;

use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::require_workspace_admin;

// ─────────────────────────────────────────────────────────────────────────────
// Read operations
// ─────────────────────────────────────────────────────────────────────────────

/// Load workspace settings for the admin settings page.
///
/// Returns workspace name. Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_settings() -> Result<WorkspaceSettingsData, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = trakkt_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    // Parse workspace settings JSON to extract default_auto_archive_days.
    let default_auto_archive_days = workspace
        .settings
        .as_ref()
        .and_then(|val| {
            match serde_json::from_value::<trakkt_types::models::WorkspaceSettings>(val.clone()) {
                Ok(ws) => ws.default_auto_archive_days,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse workspace settings JSON");
                    None
                }
            }
        });

    Ok(WorkspaceSettingsData {
        workspace_name: workspace.name.unwrap_or_default(),
        default_auto_archive_days,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Write operations
// ─────────────────────────────────────────────────────────────────────────────

/// Update the workspace name. Requires admin role.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_name(name: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("Workspace name cannot be empty"));
    }

    trakkt_auth::workspace_service::update_workspace_name(
        ac.db(),
        &ac.ws_id,
        trimmed,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;

    Ok(())
}


/// Update the workspace-level default auto-archive days. Requires admin role.
///
/// Pass `None` to clear (no workspace-level default — each team uses its own
/// setting or the compile-time fallback). Pass `Some(0)` to explicitly disable
/// archiving workspace-wide. Pass `Some(N)` for a specific duration.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_auto_archive(
    days: Option<u32>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    // Read current workspace to get existing settings.
    let workspace = trakkt_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    // Parse existing settings (or start fresh).
    let mut ws_settings = workspace
        .settings
        .as_ref()
        .and_then(|val| {
            match serde_json::from_value::<trakkt_types::models::WorkspaceSettings>(val.clone()) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse workspace settings, starting fresh");
                    None
                }
            }
        })
        .unwrap_or_default();

    ws_settings.default_auto_archive_days = days;

    let settings_value = match serde_json::to_value(&ws_settings) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to serialize workspace settings");
            return Err(ServerFnError::new(format!("Failed to serialize settings: {e}")));
        }
    };

    trakkt_auth::workspace_service::update_workspace_settings(
        ac.db(),
        &ac.ws_id,
        &settings_value,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;

    Ok(())
}


// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};
