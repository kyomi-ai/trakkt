// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for label CRUD operations.
//!
//! Thin wrappers around `trakkt_auth::label_service` — extract auth,
//! call service, return.

use leptos::prelude::*;
use trakkt_types::models::Label;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List all labels in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_labels() -> Result<Vec<Label>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let labels = trakkt_auth::label_service::list_labels(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    Ok(labels)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Create a new label in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn create_label(name: String, color: String) -> Result<Label, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let label = trakkt_auth::label_service::create_label(ac.db(), &ac.ws_id, &name, &color)
        .await
        .into_sfn()?;
    Ok(label)
}

/// Update a label's name and color.
#[server(prefix = "/leptos-api")]
pub async fn update_label(
    label_id: String,
    name: String,
    color: String,
) -> Result<Label, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_label_ownership(ac.db(), &ac.ws_id, &label_id).await?;
    let label = trakkt_auth::label_service::update_label(ac.db(), &label_id, &name, &color)
        .await
        .into_sfn()?;
    Ok(label)
}

/// Delete a label.
#[server(prefix = "/leptos-api")]
pub async fn delete_label(label_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    verify_label_ownership(ac.db(), &ac.ws_id, &label_id).await?;
    trakkt_auth::label_service::delete_label(ac.db(), &label_id)
        .await
        .into_sfn()?;
    Ok(())
}

#[cfg(feature = "ssr")]
async fn verify_label_ownership(
    db: &trakkt_core::DbPool,
    workspace_id: &str,
    label_id: &str,
) -> Result<(), ServerFnError> {
    use super::IntoServerFnError;
    let label = trakkt_auth::label_service::get_label_by_id(db, label_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Label not found"))?;
    if label.workspace_id != workspace_id {
        return Err(ServerFnError::new("Label not found"));
    }
    Ok(())
}
