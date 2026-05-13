// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for notification operations.
//!
//! Thin wrappers around `trakkt_auth::notification_service` — extract auth,
//! call service, return.

use leptos::prelude::*;
use trakkt_types::models::Notification;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Read operations ───────────────────────────────────────────────────────

/// List notifications for the current user.
///
/// When `unread_only` is true, only unread notifications are returned.
#[server(prefix = "/leptos-api")]
pub async fn list_notifications(unread_only: bool) -> Result<Vec<Notification>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let notifications =
        trakkt_auth::notification_service::list_notifications(ac.db(), &ac.auth.user_id, unread_only)
            .await
            .into_sfn()?;
    Ok(notifications)
}

/// Count unread notifications for the current user.
#[server(prefix = "/leptos-api")]
pub async fn count_unread_notifications() -> Result<i64, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let count = trakkt_auth::notification_service::count_unread(ac.db(), &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(count)
}

// ─── Write operations ──────────────────────────────────────────────────────

/// Mark a single notification as read.
#[server(prefix = "/leptos-api")]
pub async fn mark_notification_read(notification_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::notification_service::mark_as_read(ac.db(), &notification_id, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}

/// Mark all of the current user's notifications as read.
#[server(prefix = "/leptos-api")]
pub async fn mark_all_notifications_read() -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    trakkt_auth::notification_service::mark_all_as_read(ac.db(), &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}
