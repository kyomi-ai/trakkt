// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue attachment operations.
//!
//! Thin wrappers around `trakkt_auth::attachment_service` — extract auth,
//! resolve issue identifiers, call service, return.

use leptos::prelude::*;
use trakkt_types::models::Attachment;

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Server functions ─────────────────────────────────────────────────────

/// List all attachments linked to an issue, identified by team key + number.
#[server(prefix = "/leptos-api")]
pub async fn list_issue_attachments(
    team_key: String,
    issue_number: i32,
) -> Result<Vec<Attachment>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let db = ac.db();

    let issue = trakkt_auth::issue_service::get_issue(db, &ac.ws_id, &team_key, issue_number)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Issue {team_key}-{issue_number} not found")))?;

    let attachments =
        trakkt_auth::attachment_service::list_issue_attachments(db, &ac.ws_id, &issue.issue_id)
            .await
            .into_sfn()?;

    // Convert from service DTO to WASM-safe DTO
    Ok(attachments
        .into_iter()
        .map(|a| Attachment {
            attachment_id: a.attachment_id,
            filename: a.filename,
            content_type: a.content_type,
            size_bytes: a.size_bytes,
            created_at: a.created_at,
        })
        .collect())
}

/// Detach an attachment from an issue (removes the link, not the attachment itself).
#[server(prefix = "/leptos-api")]
pub async fn detach_attachment_from_issue(
    team_key: String,
    issue_number: i32,
    attachment_id: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let db = ac.db();

    let issue = trakkt_auth::issue_service::get_issue(db, &ac.ws_id, &team_key, issue_number)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Issue {team_key}-{issue_number} not found")))?;

    trakkt_auth::attachment_service::detach_from_issue(
        db,
        &ac.ws_id,
        &issue.issue_id,
        &attachment_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()?;

    Ok(())
}
