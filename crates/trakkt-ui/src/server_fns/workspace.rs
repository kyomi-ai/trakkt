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

    Ok(WorkspaceSettingsData {
        workspace_name: workspace.name.unwrap_or_default(),
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

    trakkt_auth::workspace_service::update_workspace_name(ac.db(), &ac.ws_id, trimmed)
        .await
        .into_sfn()?;

    Ok(())
}


// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};
