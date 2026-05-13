// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for comment CRUD operations.
//!
//! Thin wrappers around `trakkt_auth::comment_service` — extract auth,
//! resolve issue number, call service, return.

use leptos::prelude::*;
use trakkt_types::models::Comment;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all comments for an issue, identified by its team key + number (e.g. "ENG-42").
#[server(prefix = "/leptos-api")]
pub async fn list_comments(team_key: String, issue_number: i32) -> Result<Vec<Comment>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    let comments = trakkt_auth::comment_service::list_comments(ac.db(), &issue_id)
        .await
        .into_sfn()?;
    Ok(comments)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Create a new comment on an issue.
#[server(prefix = "/leptos-api")]
pub async fn create_comment(
    team_key: String,
    issue_number: i32,
    body: String,
    parent_id: Option<String>,
) -> Result<Comment, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, issue_number).await?;
    let comment = trakkt_auth::comment_service::create_comment(
        ac.db(),
        &issue_id,
        &ac.auth.user_id,
        &body,
        parent_id.as_deref(),
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(comment)
}

/// Update a comment's body. Only the author can edit their own comments.
#[server(prefix = "/leptos-api")]
pub async fn update_comment(
    comment_id: String,
    body: String,
) -> Result<Comment, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let comment = trakkt_auth::comment_service::update_comment(
        ac.db(),
        &comment_id,
        &ac.auth.user_id,
        &body,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;
    Ok(comment)
}

/// Delete a comment. Only the author can delete their own comments.
#[server(prefix = "/leptos-api")]
pub async fn delete_comment(comment_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::comment_service::delete_comment(ac.db(), &comment_id, &ac.auth.user_id, ac.ctx.ws_manager.as_ref())
        .await
        .into_sfn()?;
    Ok(())
}
