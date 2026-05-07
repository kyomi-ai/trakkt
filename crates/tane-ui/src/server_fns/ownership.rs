// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Accept Ownership page.
//!
//! These handle fetching, accepting, and declining ownership transfers.
//! The page is a standalone route (`/accept-ownership/:transferId`) that
//! does not use the main layout wrapper.

use leptos::prelude::*;

// On the server, re-export the canonical definition from `tane-auth`.
// On the client (WASM), provide an identical definition for deserialization.
// The auth crate owns the struct shape; the client mirror exists only because
// `tane-auth` is not compiled for the WASM target.
#[cfg(feature = "ssr")]
pub use tane_auth::workspace_service::OwnershipTransferDetail;

/// Client-side mirror of `tane_auth::workspace_service::OwnershipTransferDetail`.
/// Must be kept in sync with the canonical definition.
#[cfg(not(feature = "ssr"))]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OwnershipTransferDetail {
    pub transfer_id: String,
    pub workspace_name: String,
    pub from_user_email: String,
    pub expires_at: String,
    pub status: String,
}

/// Convenience alias so page modules can keep importing `OwnershipTransfer`.
pub type OwnershipTransfer = OwnershipTransferDetail;

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch a specific ownership transfer by ID.
///
/// Returns `Ok(Some(transfer))` if a pending transfer exists for the
/// authenticated user, `Ok(None)` if not found / not pending / expired.
///
/// Mirrors the React flow: fetch all pending transfers for the user,
/// find the one matching `transfer_id`, verify status == "pending".
#[server(prefix = "/leptos-api")]
pub async fn get_ownership_transfer(
    transfer_id: String,
) -> Result<Option<OwnershipTransferDetail>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    tane_auth::workspace_service::get_transfer_for_recipient(
        &ctx.db,
        &transfer_id,
        &auth.user_id,
    )
    .await
    .into_sfn()
}

/// Accept an ownership transfer. Only the recipient can accept.
///
/// Mirrors `POST /api/v1/workspaces/ownership/transfer/{id}/accept`.
#[server(prefix = "/leptos-api")]
pub async fn accept_ownership_transfer(
    transfer_id: String,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let transfer = tane_auth::workspace_service::get_ownership_transfer(&ctx.db, &transfer_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Transfer not found"))?;

    // Authorization first — recipient check before status/expiry
    if transfer.to_user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the transfer recipient can accept",
        ));
    }

    if transfer.status != tane_core::enums::TransferStatus::Pending {
        return Err(ServerFnError::new("Transfer is no longer pending"));
    }

    if transfer.expires_at < chrono::Utc::now() {
        let _ = tane_auth::workspace_service::update_transfer_status(
            &ctx.db,
            &transfer_id,
            "expired",
        )
        .await;
        return Err(ServerFnError::new("Transfer has expired"));
    }

    tane_auth::workspace_service::complete_ownership_transfer(
        &ctx.db,
        &transfer_id,
        &transfer.workspace_id,
        &auth.user_id,
    )
    .await
    .into_sfn()?;

    tracing::info!(
        "Ownership transfer {} accepted: workspace {} now owned by {}",
        transfer_id,
        transfer.workspace_id,
        auth.user_id
    );

    Ok(())
}

/// Decline an ownership transfer. Only the recipient can decline.
///
/// Mirrors `POST /api/v1/workspaces/ownership/transfer/{id}/decline`.
#[server(prefix = "/leptos-api")]
pub async fn decline_ownership_transfer(
    transfer_id: String,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let transfer = tane_auth::workspace_service::get_ownership_transfer(&ctx.db, &transfer_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Transfer not found"))?;

    // Authorization first — recipient check before status
    if transfer.to_user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the transfer recipient can decline",
        ));
    }

    if transfer.status != tane_core::enums::TransferStatus::Pending {
        return Err(ServerFnError::new("Transfer is no longer pending"));
    }

    tane_auth::workspace_service::update_transfer_status(&ctx.db, &transfer_id, "declined")
        .await
        .into_sfn()?;

    tracing::info!(
        "Ownership transfer {} declined by {}",
        transfer_id,
        auth.user_id
    );

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, IntoServerFnError};
