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
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListStatusesApiParams { team_id, team_key: None };
    let result = trakkt_api::statuses::list_statuses(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}
