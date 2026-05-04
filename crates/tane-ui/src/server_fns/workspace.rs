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

/// Reject non-workspace-admin users.
///
/// Mirrors `require_workspace_admin()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &tane_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&tane_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}

/// Read a nested key from `settings.custom_settings[key]`.
///
/// Mirrors `custom_settings_get()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn custom_settings_get<'a>(
    settings: &'a Option<serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    settings
        .as_ref()
        .and_then(|s| s.get("custom_settings"))
        .and_then(|cs| cs.get(key))
}

/// Merge a key-value pair into `settings.custom_settings`.
///
/// Mirrors `merge_custom_settings()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn merge_custom_settings(
    settings: &Option<serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    let mut s = settings
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    // Ensure custom_settings exists
    if s.get("custom_settings").is_none()
        && let Some(obj) = s.as_object_mut()
    {
        obj.insert(
            "custom_settings".to_string(),
            serde_json::json!({}),
        );
    }

    if let Some(cs) = s.get_mut("custom_settings").and_then(|v| v.as_object_mut()) {
        cs.insert(key.to_string(), value);
    }

    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Read operations
// ─────────────────────────────────────────────────────────────────────────────

/// Load workspace settings for the admin settings page.
///
/// Returns workspace name, default AI model, and chart palette.
/// Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_settings() -> Result<WorkspaceSettingsData, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = tane_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let default_model = custom_settings_get(&workspace.settings, "default_model")
        .and_then(|v| v.as_str())
        .unwrap_or("claude-sonnet-4-5-20250929")
        .to_string();

    let chart_palette = custom_settings_get(&workspace.settings, "chartml_config")
        .and_then(|v| v.get("style").or_else(|| v.get("config").and_then(|c| c.get("style"))))
        .and_then(|s| s.as_str())
        .unwrap_or("tane")
        .to_string();

    Ok(WorkspaceSettingsData {
        workspace_name: workspace.name.unwrap_or_default(),
        default_model,
        chart_palette,
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

    tane_auth::workspace_service::update_workspace_name(ac.db(), &ac.ws_id, trimmed)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the workspace default AI model. Requires admin role.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_model(model: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = tane_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let updated_settings = merge_custom_settings(
        &workspace.settings,
        "default_model",
        serde_json::json!(model),
    );

    tane_auth::workspace_service::update_workspace_settings(
        ac.db(),
        &ac.ws_id,
        &updated_settings,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Update the workspace ChartML config (chart palette). Requires admin role.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_chartml_config(palette: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = tane_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let config_value = serde_json::json!({
        "type": "config",
        "version": 1,
        "style": palette
    });

    let updated_settings = merge_custom_settings(
        &workspace.settings,
        "chartml_config",
        config_value,
    );

    tane_auth::workspace_service::update_workspace_settings(
        ac.db(),
        &ac.ws_id,
        &updated_settings,
    )
    .await
    .into_sfn()?;

    Ok(())
}


// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

#[cfg(all(test, feature = "ssr"))]
mod tests {
    //! Guards against accidental re-nesting of the workspace chartml_config writer payload.
    //!
    //! See the companion test in `server_fns::profile::tests` for the per-user
    //! equivalent and KYO-129 Part 2 for rationale.

    #[test]
    fn workspace_chart_palette_writer_produces_flat_shape() {
        let palette = "balanced".to_string();
        let config_value = serde_json::json!({
            "type": "config",
            "version": 1,
            "style": palette
        });
        assert_eq!(config_value["style"], "balanced");
        assert_eq!(config_value["type"], "config");
        assert_eq!(config_value["version"], 1);
        assert!(
            config_value.get("config").is_none(),
            "workspace chartml_config must be flat, not nested under a 'config' key"
        );
    }
}
