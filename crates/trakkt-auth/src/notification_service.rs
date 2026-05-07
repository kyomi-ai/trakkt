// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notification service — operations for the `notifications` table.
//!
//! Notifications inform users about issue events (assignment, mention,
//! status change, etc.). They are user-scoped and support read/unread
//! tracking.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::Notification;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;

const DEFAULT_NOTIFICATION_LIMIT: i64 = 50;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising notification queries.
///
/// The `read` field is `bool` — sqlx handles the Postgres `boolean` and
/// SQLite `INTEGER` mapping transparently.
#[derive(sqlx::FromRow)]
struct NotificationRow {
    notification_id: String,
    workspace_id: String,
    user_id: String,
    issue_id: String,
    notification_type: String,
    read: bool,
    issue_title: Option<String>,
    issue_number: Option<i32>,
    created_at: String,
}

impl NotificationRow {
    fn into_dto(self) -> Notification {
        Notification {
            notification_id: self.notification_id,
            workspace_id: self.workspace_id,
            user_id: self.user_id,
            issue_id: self.issue_id,
            notification_type: self.notification_type,
            read: self.read,
            issue_title: self.issue_title,
            issue_number: self.issue_number,
            created_at: self.created_at,
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a notification for a user about an issue event.
pub async fn create_notification(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    issue_id: &str,
    notification_type: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let notification_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO notifications \
            (notification_id, workspace_id, user_id, issue_id, type, read, created_at) \
         VALUES ($1, $2, $3, $4, $5, {bf}, {now})",
        bf = sql_compat::bool_false(is_pg),
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &notification_id,
        workspace_id,
        user_id,
        issue_id,
        notification_type
    )?;

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::NOTIFICATION,
        &notification_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, notification_id = %notification_id, "Failed to write sync log entry for notification create");
    }

    Ok(())
}

/// List notifications for a user, optionally filtering to unread only.
///
/// Joins with `issues` to include issue title and number.
/// Results are ordered by creation date (newest first), limited to 50.
pub async fn list_notifications(
    db: &DbPool,
    user_id: &str,
    unread_only: bool,
) -> trakkt_core::Result<Vec<Notification>> {
    let is_pg = db.is_postgres();

    let unread_filter = if unread_only {
        let bf = sql_compat::bool_false(is_pg);
        format!("AND n.read = {bf}")
    } else {
        String::new()
    };

    let sql = format!(
        "SELECT n.notification_id, n.workspace_id, n.user_id, n.issue_id, \
                n.type AS notification_type, n.read, \
                i.title AS issue_title, i.number AS issue_number, \
                CAST(n.created_at AS TEXT) AS created_at \
         FROM notifications n \
         LEFT JOIN issues i ON i.issue_id = n.issue_id \
         WHERE n.user_id = $1 {unread_filter} \
         ORDER BY n.created_at DESC \
         LIMIT {DEFAULT_NOTIFICATION_LIMIT}"
    );

    let rows: Vec<NotificationRow> =
        trakkt_core::db_fetch_all!(db, NotificationRow, &sql, user_id)?;
    Ok(rows.into_iter().map(NotificationRow::into_dto).collect())
}

/// Mark a single notification as read. Verifies the user owns it.
pub async fn mark_as_read(
    db: &DbPool,
    notification_id: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    let sql = format!(
        "UPDATE notifications SET read = {bt} \
         WHERE notification_id = $1 AND user_id = $2"
    );
    let result = trakkt_core::db_execute!(db, &sql, notification_id, user_id)?;

    if result.rows_affected() == 0 {
        // Distinguish between "not found" and "not owned by user".
        let exists: i64 = trakkt_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM notifications WHERE notification_id = $1",
            notification_id
        )?;
        if exists == 0 {
            return Err(trakkt_core::Error::NotFound(format!(
                "notification {notification_id} not found"
            )));
        }
        return Err(trakkt_core::Error::Forbidden(
            "you can only mark your own notifications as read".to_string(),
        ));
    }

    Ok(())
}

/// Mark all of a user's notifications as read.
pub async fn mark_all_as_read(
    db: &DbPool,
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);

    let sql = format!(
        "UPDATE notifications SET read = {bt} \
         WHERE user_id = $1 AND read = {bf}"
    );
    trakkt_core::db_execute!(db, &sql, user_id)?;

    Ok(())
}

/// Count unread notifications for a user.
pub async fn count_unread(
    db: &DbPool,
    user_id: &str,
) -> trakkt_core::Result<i64> {
    let is_pg = db.is_postgres();
    let bf = sql_compat::bool_false(is_pg);

    let sql = format!(
        "SELECT COUNT(*) FROM notifications \
         WHERE user_id = $1 AND read = {bf}"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(db, i64, &sql, user_id)?;
    Ok(count)
}
