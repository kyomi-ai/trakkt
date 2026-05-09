// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for status read operations.

use leptos::prelude::*;
use trakkt_types::models::Status;

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

/// List statuses for the current workspace (global + optional team-specific).
#[server(prefix = "/leptos-api")]
pub async fn list_statuses(team_id: Option<String>) -> Result<Vec<Status>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let statuses = trakkt_auth::status_service::list_statuses(
        ac.db(),
        &ac.ws_id,
        team_id.as_deref(),
    )
    .await
    .into_sfn()?;
    Ok(statuses)
}
