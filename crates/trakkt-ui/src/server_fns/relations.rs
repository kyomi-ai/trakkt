// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue relation operations.
//!
//! Thin wrappers around `trakkt_auth::relation_service` — extract auth,
//! resolve issue identifiers, call service, return.

use leptos::prelude::*;
use trakkt_types::models::{IssueRelation, IssueRelationWithDetails};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Helpers (server-only) ─────────────────────────────────────────────────

/// Parse a compound identifier like "TRA-123" into (team_key, number).
#[cfg(feature = "ssr")]
fn parse_identifier(identifier: &str) -> Result<(&str, i32), ServerFnError> {
    let (key, num_str) = identifier
        .rsplit_once('-')
        .ok_or_else(|| ServerFnError::new(format!("Invalid issue identifier: {identifier} (expected format: TEAM-123)")))?;
    let number: i32 = num_str
        .parse()
        .map_err(|_| ServerFnError::new(format!("Invalid issue number in identifier: {identifier}")))?;
    Ok((key, number))
}

/// Resolve a compound identifier (e.g. "TRA-123") to an issue_id.
#[cfg(feature = "ssr")]
async fn resolve_issue_id_from_identifier(
    db: &trakkt_core::DbPool,
    workspace_id: &str,
    identifier: &str,
) -> Result<String, ServerFnError> {
    let (team_key, number) = parse_identifier(identifier)?;
    super::issues::resolve_issue_id(db, workspace_id, team_key, number).await
}

// ─── Server functions ─────────────────────────────────────────────────────

/// Add a relation between two issues, identified by compound identifiers (e.g. "ENG-42").
#[server(prefix = "/leptos-api")]
pub async fn add_relation(
    source_identifier: String,
    target_identifier: String,
    relation_type: String,
) -> Result<IssueRelation, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let source_id = resolve_issue_id_from_identifier(ac.db(), &ac.ws_id, &source_identifier).await?;
    let target_id = resolve_issue_id_from_identifier(ac.db(), &ac.ws_id, &target_identifier).await?;

    trakkt_auth::relation_service::create_relation(
        ac.db(),
        &ac.ws_id,
        &source_id,
        &target_id,
        &relation_type,
        Some(&ac.auth.user_id),
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()
}

/// Remove a relation by its ID.
#[server(prefix = "/leptos-api")]
pub async fn remove_relation(relation_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::relation_service::delete_relation(
        ac.db(),
        &relation_id,
        &ac.ws_id,
        ac.ctx.ws_manager.as_ref(),
    )
    .await
    .into_sfn()
}

/// List all relations for an issue, identified by team key + number.
#[server(prefix = "/leptos-api")]
pub async fn list_issue_relations(
    team_key: String,
    number: i32,
) -> Result<Vec<IssueRelationWithDetails>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let issue_id = super::issues::resolve_issue_id(ac.db(), &ac.ws_id, &team_key, number).await?;
    trakkt_auth::relation_service::list_relations_for_issue(
        ac.db(),
        &issue_id,
        &ac.ws_id,
    )
    .await
    .into_sfn()
}
