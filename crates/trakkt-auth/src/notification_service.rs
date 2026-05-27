// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notification service — operations for the `notifications` table.
//!
//! Notifications inform users about issue events (assignment, mention,
//! status change, etc.). They are user-scoped and support read/unread
//! tracking.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;
use trakkt_types::models::Notification;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

const DEFAULT_NOTIFICATION_LIMIT: i64 = 50;

pub const TYPE_COMMENTED: &str = "commented";
pub const TYPE_STATUS_CHANGED: &str = "status_changed";
pub const TYPE_ASSIGNED: &str = "assigned";
pub const TYPE_PRIORITY_CHANGED: &str = "priority_changed";

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
    team_key: Option<String>,
    actor_id: Option<String>,
    actor_name: Option<String>,
    action_source: String,
    action_source_label: Option<String>,
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
            team_key: self.team_key,
            actor_id: self.actor_id,
            actor_name: self.actor_name,
            action_source: self.action_source
                .parse::<ActionSource>()
                .unwrap_or_else(|_| {
                    tracing::warn!(raw = %self.action_source, "Unknown action_source value; defaulting to User");
                    ActionSource::User
                }),
            action_source_label: self.action_source_label,
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
    actor_id: Option<&str>,
    action_source: ActionSource,
    action_source_label: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let notification_id = uuid::Uuid::new_v4().to_string();
    let action_source_str = action_source.as_str();

    let sql = format!(
        "INSERT INTO notifications \
            (notification_id, workspace_id, user_id, issue_id, type, actor_id, action_source, action_source_label, read, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {bf}, {now})",
        bf = sql_compat::bool_false(is_pg),
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &notification_id,
        workspace_id,
        user_id,
        issue_id,
        notification_type,
        actor_id,
        action_source_str,
        action_source_label
    )?;

    let notification = trakkt_core::db_fetch_optional!(
        db,
        NotificationRow,
        "SELECT n.notification_id, n.workspace_id, n.user_id, n.issue_id, \
                n.type AS notification_type, n.read, \
                i.title AS issue_title, i.number AS issue_number, \
                t.key AS team_key, \
                n.actor_id, \
                u_actor.name AS actor_name, \
                n.action_source, n.action_source_label, \
                CAST(n.created_at AS TEXT) AS created_at \
         FROM notifications n \
         LEFT JOIN issues i ON i.issue_id = n.issue_id \
         LEFT JOIN teams t ON t.team_id = i.team_id \
         LEFT JOIN users u_actor ON u_actor.user_id = n.actor_id \
         WHERE n.notification_id = $1",
        &notification_id
    )?;
    let notification_data = notification.map(|r| r.into_dto());

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::NOTIFICATION,
        &notification_id,
        workspace_id,
        SyncActionType::Insert,
        notification_data.as_ref().and_then(|n| serde_json::to_value(n).ok()),
    )
    .await
    {
        tracing::warn!(error = %e, notification_id = %notification_id, "Failed to write sync log entry for notification create");
    }

    // WebSocket broadcast with full entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::NOTIFICATION,
            &notification_id,
            SyncActionType::Insert,
            notification_data.and_then(|n| serde_json::to_value(&n).ok()),
        )
        .await;
    }

    Ok(())
}

/// List notifications for a user with optional filters.
///
/// Joins with `issues` to include issue title and number.
/// Results are ordered by creation date (newest first), limited to 50.
///
/// Optional filters narrow results by notification type, team key, or
/// text search (issue title / identifier). The nullable parameter pattern
/// (`$N IS NULL OR column = $N`) keeps bind positions fixed regardless
/// of which filters are active.
pub async fn list_notifications(
    db: &DbPool,
    user_id: &str,
    unread_only: bool,
    notification_type: Option<&str>,
    team_key: Option<&str>,
    search: Option<&str>,
) -> trakkt_core::Result<Vec<Notification>> {
    let is_pg = db.is_postgres();
    let cast_text = if is_pg { "::TEXT" } else { "" };

    let unread_filter = if unread_only {
        let bf = sql_compat::bool_false(is_pg);
        format!("AND n.read = {bf}")
    } else {
        String::new()
    };

    // $1 = user_id, $2 = notification_type, $3 = team_key, $4 = search pattern
    let sql = format!(
        "SELECT n.notification_id, n.workspace_id, n.user_id, n.issue_id, \
                n.type AS notification_type, n.read, \
                i.title AS issue_title, i.number AS issue_number, \
                t.key AS team_key, \
                n.actor_id, \
                u_actor.name AS actor_name, \
                n.action_source, n.action_source_label, \
                CAST(n.created_at AS TEXT) AS created_at \
         FROM notifications n \
         LEFT JOIN issues i ON i.issue_id = n.issue_id \
         LEFT JOIN teams t ON t.team_id = i.team_id \
         LEFT JOIN users u_actor ON u_actor.user_id = n.actor_id \
         WHERE n.user_id = $1 AND n.deleted_at IS NULL {unread_filter} \
           AND ($2{cast_text} IS NULL OR n.type = $2) \
           AND ($3{cast_text} IS NULL OR t.key = $3) \
           AND ($4{cast_text} IS NULL OR i.title LIKE $4 ESCAPE '\\' \
                OR (t.key || '-' || CAST(i.number AS TEXT)) LIKE $4 ESCAPE '\\') \
         ORDER BY n.created_at DESC \
         LIMIT {DEFAULT_NOTIFICATION_LIMIT}"
    );

    // Prepare the search term with wildcards, escaping LIKE special chars.
    let search_pattern = search.map(|s| {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{escaped}%")
    });

    let rows: Vec<NotificationRow> = trakkt_core::db_fetch_all!(
        db,
        NotificationRow,
        &sql,
        user_id,
        notification_type,
        team_key,
        search_pattern.as_deref()
    )?;
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
         WHERE notification_id = $1 AND user_id = $2 AND deleted_at IS NULL"
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
         WHERE user_id = $1 AND read = {bf} AND deleted_at IS NULL"
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
         WHERE user_id = $1 AND read = {bf} AND deleted_at IS NULL"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(db, i64, &sql, user_id)?;
    Ok(count)
}

/// Execute a bulk UPDATE on notifications by ID. `$1` is always `user_id`;
/// `$2..$N+1` are the notification IDs.
async fn bulk_update_notifications(
    db: &DbPool,
    user_id: &str,
    notification_ids: &[String],
    set_and_where: &str,
) -> trakkt_core::Result<()> {
    if notification_ids.is_empty() {
        return Ok(());
    }

    let (in_clause, _) = trakkt_core::db::in_clause_placeholders(notification_ids.len(), 2);
    let sql = format!(
        "UPDATE notifications SET {set_and_where} \
           AND notification_id IN {in_clause}"
    );

    trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(user_id);
        for id in notification_ids {
            query = query.bind(id);
        }
        query.execute(p).await?;
        Ok::<(), sqlx::Error>(())
    })?;

    Ok(())
}

/// Bulk mark specific notifications as read. Only affects the given user's
/// non-deleted, currently-unread notifications.
pub async fn bulk_mark_as_read(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let clause = format!(
        "read = {bt} WHERE user_id = $1 AND read = {bf} AND deleted_at IS NULL"
    );
    bulk_update_notifications(db, user_id, notification_ids, &clause).await
}

/// Bulk mark specific notifications as unread. Only affects the given user's
/// non-deleted, currently-read notifications.
pub async fn bulk_mark_as_unread(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let clause = format!(
        "read = {bf} WHERE user_id = $1 AND read = {bt} AND deleted_at IS NULL"
    );
    bulk_update_notifications(db, user_id, notification_ids, &clause).await
}

/// Soft-delete specific notifications. Only affects the given user's
/// non-deleted notifications.
pub async fn bulk_delete_notifications(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let clause = format!(
        "deleted_at = {now} WHERE user_id = $1 AND deleted_at IS NULL"
    );
    bulk_update_notifications(db, user_id, notification_ids, &clause).await
}
