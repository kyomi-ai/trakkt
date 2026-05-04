// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for user context.

use std::collections::HashMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, IntoServerFnError};

/// Combined user, workspace, and capability context.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub workspace_roles: Vec<String>,
    pub is_owner: bool,
    pub is_personal_mode: bool,
    pub is_self_hosted: bool,
    pub billing_enabled: bool,
    pub capabilities: HashMap<String, bool>,
}

/// Load the authenticated user's full context.
#[server(prefix = "/leptos-api")]
pub async fn get_user_context() -> Result<UserContext, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    Ok(UserContext {
        user_id: auth.user_id,
        email: auth.email,
        name: auth.name,
        workspace_id: auth.workspace.workspace_id,
        workspace_name: auth.workspace.workspace_name,
        workspace_roles: auth
            .workspace
            .workspace_roles
            .iter()
            .map(|r| r.to_string())
            .collect(),
        is_owner: auth.workspace.is_owner,
        is_personal_mode: ctx.config.is_personal(),
        is_self_hosted: ctx.config.self_hosted,
        billing_enabled: false,
        capabilities: HashMap::new(),
    })
}
