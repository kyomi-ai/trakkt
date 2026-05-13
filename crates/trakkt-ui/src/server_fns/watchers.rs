// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue watcher operations.
//!
//! Thin wrappers around `trakkt_auth::watcher_service` — extract auth,
//! call service, return.

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all issue IDs the current user is watching in the active workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_watched_issue_ids() -> Result<Vec<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ids = trakkt_auth::watcher_service::list_watched_issue_ids(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
    )
    .await
    .into_sfn()?;
    Ok(ids)
}

/// Check whether the current user is watching a specific issue (by team key + number).
#[server(prefix = "/leptos-api")]
pub async fn is_watching(team_key: String, issue_number: i32) -> Result<bool, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    let watching = trakkt_auth::watcher_service::is_watching(ac.db(), &issue_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(watching)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Start watching an issue (by team key + number).
#[server(prefix = "/leptos-api")]
pub async fn watch_issue(team_key: String, issue_number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    trakkt_auth::watcher_service::watch_issue(ac.db(), &issue_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}

/// Stop watching an issue (by team key + number).
#[server(prefix = "/leptos-api")]
pub async fn unwatch_issue(team_key: String, issue_number: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    trakkt_auth::watcher_service::unwatch_issue(ac.db(), &issue_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}
