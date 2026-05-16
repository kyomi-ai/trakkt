// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for issue relation operations.
//!
//! Thin wrappers around `trakkt_auth::relation_service` — extract auth,
//! resolve issue identifiers, call service, return.

use leptos::prelude::*;
use trakkt_types::models::{IssueRelation, IssueRelationWithDetails};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Server functions ─────────────────────────────────────────────────────

/// Add a relation between two issues, identified by compound identifiers (e.g. "ENG-42").
#[server(prefix = "/leptos-api")]
pub async fn add_relation(
    source_identifier: String,
    target_identifier: String,
    relation_type: String,
) -> Result<IssueRelation, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::AddRelationApiParams {
        source_issue: Some(source_identifier), target_issue: target_identifier, relation_type,
    };
    let result = trakkt_api::relations::add_relation(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}

/// Remove a relation by its ID.
#[server(prefix = "/leptos-api")]
pub async fn remove_relation(relation_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::RemoveRelationApiParams { relation_id };
    trakkt_api::relations::remove_relation(&ctx, params).await.into_sfn()?;
    Ok(())
}

/// List all relations for an issue, identified by team key + number.
#[server(prefix = "/leptos-api")]
pub async fn list_issue_relations(
    team_key: String,
    number: i32,
) -> Result<Vec<IssueRelationWithDetails>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ctx = ac.api_ctx();
    let params = trakkt_types::api::ListRelationsApiParams {
        issue_identifier: Some(format!("{team_key}-{number}")),
        team_key: None, issue_number: None,
    };
    let result = trakkt_api::relations::list_relations(&ctx, params).await.into_sfn()?;
    serde_json::from_value(result).into_sfn()
}
