// SPDX-License-Identifier: AGPL-3.0-or-later

//! Comment service — CRUD operations for the `comments` table.
//!
//! Comments belong to issues and support threading via an optional `parent_id`.
//! Write operations verify ownership before allowing edits/deletes.

use trakkt_core::db::DbTx;
use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;
use trakkt_types::models::Comment;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising comment queries with joined user data.
#[derive(sqlx::FromRow)]
struct CommentRow {
    comment_id: String,
    issue_id: String,
    user_id: String,
    body: String,
    parent_id: Option<String>,
    author_name: Option<String>,
    author_avatar: Option<String>,
    action_source: String,
    action_source_label: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl CommentRow {
    fn into_dto(self) -> Comment {
        Comment {
            comment_id: self.comment_id,
            issue_id: self.issue_id,
            user_id: self.user_id,
            body: self.body,
            parent_id: self.parent_id,
            author_name: self.author_name,
            author_avatar: self.author_avatar,
            action_source: self.action_source
                .parse::<ActionSource>()
                .unwrap_or_else(|_| {
                    tracing::warn!(raw = %self.action_source, "Unknown action_source value; defaulting to User");
                    ActionSource::User
                }),
            action_source_label: self.action_source_label,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// The query behind [`get_workspace_for_issue`] and its transaction-scoped twin.
const WORKSPACE_FOR_ISSUE_SELECT: &str =
    "SELECT workspace_id FROM issues WHERE issue_id = $1";

/// Fetch the workspace_id for a given issue. Used for sync log entries.
async fn get_workspace_for_issue(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<String> {
    let ws_id: String =
        trakkt_core::db_fetch_scalar!(db, String, WORKSPACE_FOR_ISSUE_SELECT, issue_id)?;
    Ok(ws_id)
}

/// Fetch the workspace_id for a given issue on an open transaction.
///
/// Transaction-scoped [`get_workspace_for_issue`] — the sync entry needs the
/// workspace, and the pool is unreachable while a transaction is open (see
/// [`DbTx`]).
async fn get_workspace_for_issue_tx(
    tx: &mut DbTx,
    issue_id: &str,
) -> trakkt_core::Result<String> {
    let ws_id: String =
        trakkt_core::tx_fetch_scalar!(&mut *tx, String, WORKSPACE_FOR_ISSUE_SELECT, issue_id)?;
    Ok(ws_id)
}

/// The comment SELECT with its joined author, keyed by `comment_id` as `$1`.
const COMMENT_BY_ID_SELECT: &str = "\
    SELECT c.comment_id, c.issue_id, c.user_id, c.body, c.parent_id, \
           u.name AS author_name, NULL AS author_avatar, \
           c.action_source, c.action_source_label, \
           c.created_at, \
           c.updated_at \
    FROM comments c \
    JOIN users u ON u.user_id = c.user_id \
    WHERE c.comment_id = $1";

/// Serialise a comment into its sync payload.
///
/// A payload that cannot be serialised is logged and dropped: the sync entry is
/// still written, so the change keeps its place in the sequence.
fn comment_payload_value(comment: &Comment) -> Option<serde_json::Value> {
    match serde_json::to_value(comment) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(error = %e, comment_id = %comment.comment_id,
                "Failed to serialize comment for sync payload");
            None
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new comment on an issue.
///
/// The INSERT and its `sync_log` entry commit as one transaction, so a comment
/// never exists without the sync row that carries it to other clients. The
/// workspace is resolved up front: without it there is no sync entry to write,
/// which makes it a precondition of the write rather than something to warn
/// about afterwards.
pub async fn create_comment(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
    body: &str,
    parent_id: Option<&str>,
    action_source: ActionSource,
    action_source_label: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Comment> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let comment_id = uuid::Uuid::new_v4().to_string();
    let action_source_str = action_source.as_str();

    // Resolve the workspace once — needed for the sync log, the broadcast and
    // the notifications. Runs on the pool, so it happens before the transaction
    // opens.
    let workspace_id = get_workspace_for_issue(db, issue_id).await?;

    let sql = format!(
        "INSERT INTO comments (comment_id, issue_id, user_id, body, parent_id, action_source, action_source_label, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, {now}, {now})"
    );

    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(&mut tx, &sql, &comment_id, issue_id, user_id, body, parent_id, action_source_str, action_source_label)?;

    // Re-fetch with joined user data (needed for sync broadcast and return value).
    let row = trakkt_core::tx_fetch_one!(&mut tx, CommentRow, COMMENT_BY_ID_SELECT, &comment_id)?;
    let comment = row.into_dto();
    let payload = comment_payload_value(&comment);

    let sync_id = sync_log_service::write_sync_entry_in_tx(
        &mut tx,
        entity_types::COMMENT,
        &comment_id,
        &workspace_id,
        None,
        SyncActionType::Insert,
        payload.clone(),
    )
    .await?;

    tx.commit().await?;

    // Everything below reaches for the pool or the socket, so it has to follow
    // the commit.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &workspace_id,
            entity_types::COMMENT,
            &comment_id,
            SyncActionType::Insert,
            payload,
            sync_id,
        )
        .await;
    }

    // Auto-watch: commenter watches the issue they commented on (best-effort).
    if let Err(e) = crate::watcher_service::watch_issue(db, issue_id, user_id).await {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to auto-watch issue for commenter");
    }

    // Notify watchers about the new comment (best-effort).
    match crate::watcher_service::list_watchers_of_issue(db, issue_id).await {
        Ok(watchers) => {
            let prefs_map = match crate::notification_service::batch_get_preferences(
                db, &watchers, &workspace_id,
            )
            .await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to fetch notification preferences, using defaults");
                    std::collections::HashMap::new()
                }
            };

            for watcher_id in &watchers {
                // Self-exclusion check with preference awareness
                if crate::notification_service::should_suppress_self_notification(
                    watcher_id, user_id, action_source, &prefs_map,
                ) {
                    continue;
                }

                // Event type preference check
                let type_enabled = prefs_map
                    .get(watcher_id.as_str())
                    .is_none_or(|p| p.notify_comments);
                if !type_enabled {
                    continue;
                }

                if let Err(e) = crate::notification_service::create_notification(
                    db,
                    &workspace_id,
                    watcher_id,
                    issue_id,
                    crate::notification_service::TYPE_COMMENTED,
                    Some(user_id),
                    Some(&comment_id),
                    action_source,
                    action_source_label,
                    ws_manager,
                )
                .await
                {
                    tracing::warn!(error = %e, "Failed to create comment notification");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, issue_id = %issue_id, "Failed to list watchers for comment notification");
        }
    }

    Ok(comment)
}

/// List all comments for an issue, ordered by creation date (oldest first).
///
/// Joins with `users` to include author name and avatar.
pub async fn list_comments(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<Vec<Comment>> {
    let rows: Vec<CommentRow> = trakkt_core::db_fetch_all!(
        db,
        CommentRow,
        "SELECT c.comment_id, c.issue_id, c.user_id, c.body, c.parent_id, \
                u.name AS author_name, NULL AS author_avatar, \
                c.action_source, c.action_source_label, \
                c.created_at, \
                c.updated_at \
         FROM comments c \
         JOIN users u ON u.user_id = c.user_id \
         WHERE c.issue_id = $1 \
         ORDER BY c.created_at ASC",
        issue_id
    )?;
    Ok(rows.into_iter().map(CommentRow::into_dto).collect())
}

/// List all comments for a workspace, ordered by creation date (oldest first).
///
/// Joins through `issues` to filter by workspace_id and includes author name.
/// Used by the WebSocket bootstrap to hydrate the client's SyncStore.
pub async fn list_comments_for_workspace(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Comment>> {
    let rows: Vec<CommentRow> = trakkt_core::db_fetch_all!(
        db,
        CommentRow,
        "SELECT c.comment_id, c.issue_id, c.user_id, c.body, c.parent_id, \
                u.name AS author_name, NULL AS author_avatar, \
                c.action_source, c.action_source_label, \
                c.created_at, \
                c.updated_at \
         FROM comments c \
         JOIN issues i ON i.issue_id = c.issue_id \
         JOIN users u ON u.user_id = c.user_id \
         WHERE i.workspace_id = $1 \
         ORDER BY c.created_at ASC",
        workspace_id
    )?;
    Ok(rows.into_iter().map(CommentRow::into_dto).collect())
}

/// Update a comment's body. Only the comment's author can edit it.
///
/// The UPDATE and its `sync_log` entry commit as one transaction; a rejected
/// edit rolls back and returns the reason.
pub async fn update_comment(
    db: &DbPool,
    comment_id: &str,
    user_id: &str,
    body: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Comment> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    let sql = format!(
        "UPDATE comments SET body = $1, updated_at = {now} \
         WHERE comment_id = $2 AND user_id = $3"
    );
    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(&mut tx, &sql, body, comment_id, user_id)?;

    if result.rows_affected() == 0 {
        // Distinguish between "not found" and "not owned by user".
        let exists: i64 = trakkt_core::tx_fetch_scalar!(
            &mut tx,
            i64,
            "SELECT COUNT(*) FROM comments WHERE comment_id = $1",
            comment_id
        )?;
        tx.rollback().await?;
        if exists == 0 {
            return Err(trakkt_core::Error::NotFound(format!(
                "comment {comment_id} not found"
            )));
        }
        return Err(trakkt_core::Error::Forbidden(
            "you can only edit your own comments".to_string(),
        ));
    }

    // Re-fetch with joined user data.
    let row = trakkt_core::tx_fetch_one!(&mut tx, CommentRow, COMMENT_BY_ID_SELECT, comment_id)?;
    let comment = row.into_dto();
    let payload = comment_payload_value(&comment);

    let workspace_id = get_workspace_for_issue_tx(&mut tx, &comment.issue_id).await?;

    let sync_id = sync_log_service::write_sync_entry_in_tx(
        &mut tx,
        entity_types::COMMENT,
        comment_id,
        &workspace_id,
        None,
        SyncActionType::Update,
        payload.clone(),
    )
    .await?;

    tx.commit().await?;

    // The broadcast reaches for the socket, so it has to follow the commit.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &workspace_id,
            entity_types::COMMENT,
            comment_id,
            SyncActionType::Update,
            payload,
            sync_id,
        )
        .await;
    }

    Ok(comment)
}

/// Delete a comment. Only the comment's author can delete it.
///
/// The ownership check, the DELETE and the `sync_log` entry all run in one
/// transaction, so the comment cannot change owner underneath the check and can
/// never be removed without the sync row that tells clients to drop it.
pub async fn delete_comment(
    db: &DbPool,
    comment_id: &str,
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let mut tx = db.begin().await?;

    // Fetch the comment first for ownership check and sync log.
    let row = trakkt_core::tx_fetch_optional!(
        &mut tx,
        CommentRow,
        COMMENT_BY_ID_SELECT,
        comment_id
    )?;

    let comment = row.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!("comment {comment_id} not found"))
    })?;

    if comment.user_id != user_id {
        return Err(trakkt_core::Error::Forbidden(
            "you can only delete your own comments".to_string(),
        ));
    }

    trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM comments WHERE comment_id = $1",
        comment_id
    )?;

    let workspace_id = get_workspace_for_issue_tx(&mut tx, &comment.issue_id).await?;

    let sync_id = sync_log_service::write_sync_entry_in_tx(
        &mut tx,
        entity_types::COMMENT,
        comment_id,
        &workspace_id,
        None,
        SyncActionType::Delete,
        None,
    )
    .await?;

    tx.commit().await?;

    // The broadcast reaches for the socket, so it has to follow the commit.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &workspace_id,
            entity_types::COMMENT,
            comment_id,
            SyncActionType::Delete,
            None,
            sync_id,
        )
        .await;
    }

    Ok(())
}
