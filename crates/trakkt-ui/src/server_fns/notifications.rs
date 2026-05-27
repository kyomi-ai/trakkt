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
/// Optional filters narrow by notification type, team key, or text search.
#[server(prefix = "/leptos-api")]
pub async fn list_notifications(
    unread_only: bool,
    notification_type: Option<String>,
    team_key: Option<String>,
    search: Option<String>,
) -> Result<Vec<Notification>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let notifications = trakkt_auth::notification_service::list_notifications(
        ac.db(),
        &ac.auth.user_id,
        unread_only,
        notification_type.as_deref(),
        team_key.as_deref(),
        search.as_deref(),
    )
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

const MAX_BULK_IDS: usize = 100;

#[cfg(feature = "ssr")]
fn parse_notification_ids(raw: &str) -> Result<Vec<String>, ServerFnError> {
    let ids: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.len() > MAX_BULK_IDS {
        return Err(ServerFnError::new(format!(
            "bulk operations are limited to {MAX_BULK_IDS} notifications"
        )));
    }
    Ok(ids)
}

/// Bulk mark notifications as read.
///
/// `notification_ids` is a comma-separated string of notification UUIDs.
#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_notifications_read(notification_ids: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ids = parse_notification_ids(&notification_ids)?;
    trakkt_auth::notification_service::bulk_mark_as_read(ac.db(), &ids, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}

/// Bulk mark notifications as unread.
///
/// `notification_ids` is a comma-separated string of notification UUIDs.
#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_notifications_unread(notification_ids: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ids = parse_notification_ids(&notification_ids)?;
    trakkt_auth::notification_service::bulk_mark_as_unread(ac.db(), &ids, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}

/// Bulk soft-delete notifications.
///
/// `notification_ids` is a comma-separated string of notification UUIDs.
#[server(prefix = "/leptos-api")]
pub async fn bulk_delete_notifications(notification_ids: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let ids = parse_notification_ids(&notification_ids)?;
    trakkt_auth::notification_service::bulk_delete_notifications(ac.db(), &ids, &ac.auth.user_id)
        .await
        .into_sfn()?;
    Ok(())
}
