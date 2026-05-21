// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue activity operations.
//!
//! Thin wrappers around `trakkt_api::activities` — extract auth,
//! resolve issue identifiers, call handler, return.

use leptos::prelude::*;
use trakkt_types::models::{IssueActivity, WorkspaceActivity};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Server functions ─────────────────────────────────────────────────────

/// List all activity entries for an issue, ordered chronologically.
#[server(prefix = "/leptos-api")]
pub async fn list_issue_activities(
    team_key: String,
    issue_number: i32,
) -> Result<Vec<IssueActivity>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListIssueActivitiesApiParams {
        issue_identifier: Some(format!("{team_key}-{issue_number}")),
        team_key: None,
        issue_number: None,
    };
    let result = trakkt_api::activities::list_issue_activities(&ctx, params)
        .await
        .into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// List activity entries across all teams in the workspace, ordered by most recent first.
#[server(prefix = "/leptos-api")]
pub async fn list_workspace_activities(
    team_key: Option<String>,
    action_type: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<WorkspaceActivity>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListWorkspaceActivitiesApiParams {
        team_key,
        action_type,
        limit,
        offset,
    };
    let result = trakkt_api::activities::list_workspace_activities(&ctx, params)
        .await
        .into_sfn()?;
    serde_json::from_value(result).into_sfn()
}
