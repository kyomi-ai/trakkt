// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue star operations.
//!
//! Thin wrappers around `trakkt_auth::star_service` — extract auth,
//! call service, return.

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all issue IDs the current user has starred in the active workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_starred_issue_ids() -> Result<Vec<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ids = trakkt_auth::star_service::list_starred_issue_ids(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(ids)
}

/// Check whether the current user has starred a specific issue (by team key + number).
#[server(prefix = "/leptos-api")]
pub async fn is_starred(team_key: String, issue_number: i32) -> Result<bool, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    let starred = trakkt_auth::star_service::is_starred(ac.db(), &issue_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(starred)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Star an issue (by team key + number).
#[server(prefix = "/leptos-api")]
pub async fn star_issue(team_key: String, issue_number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    trakkt_auth::star_service::star_issue(ac.db(), &issue_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}

/// Unstar an issue (by team key + number).
#[server(prefix = "/leptos-api")]
pub async fn unstar_issue(team_key: String, issue_number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    trakkt_auth::star_service::unstar_issue(ac.db(), &issue_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}
