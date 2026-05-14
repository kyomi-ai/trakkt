// SPDX-License-Identifier: AGPL-3.0-or-later

//! Comment service — CRUD operations for the `comments` table.
//!
//! Comments belong to issues and support threading via an optional `parent_id`.
//! Write operations verify ownership before allowing edits/deletes.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
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
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Fetch the workspace_id for a given issue. Used for sync log entries.
async fn get_workspace_for_issue(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<String> {
    let ws_id: String = trakkt_core::db_fetch_scalar!(
        db,
        String,
        "SELECT workspace_id FROM issues WHERE issue_id = $1",
        issue_id
    )?;
    Ok(ws_id)
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new comment on an issue.
pub async fn create_comment(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
    body: &str,
    parent_id: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Comment> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let comment_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO comments (comment_id, issue_id, user_id, body, parent_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, {now}, {now})"
    );
    trakkt_core::db_execute!(db, &sql, &comment_id, issue_id, user_id, body, parent_id)?;

    // Re-fetch with joined user data (needed for sync broadcast and return value).
    let row = trakkt_core::db_fetch_one!(
        db,
        CommentRow,
        "SELECT c.comment_id, c.issue_id, c.user_id, c.body, c.parent_id, \
                u.name AS author_name, NULL AS author_avatar, \
                c.created_at, \
                c.updated_at \
         FROM comments c \
         JOIN users u ON u.user_id = c.user_id \
         WHERE c.comment_id = $1",
        &comment_id
    )?;
    let comment = row.into_dto();

    // Resolve workspace once — needed for sync log, broadcast, and notifications.
    let resolved_workspace_id = get_workspace_for_issue(db, issue_id).await;

    // Sync log + broadcast — best-effort.
    match &resolved_workspace_id {
        Ok(workspace_id) => {
            if let Err(e) = sync_log_service::write_sync_entry(
                db,
                entity_types::COMMENT,
                &comment_id,
                workspace_id,
                SyncActionType::Insert,
                serde_json::to_value(&comment).ok(),
            )
            .await
            {
                tracing::warn!(error = %e, comment_id = %comment_id, "Failed to write sync log entry for comment create");
            }
            if let Some(ws) = ws_manager {
                sync_log_service::broadcast_sync_action(
                    ws,
                    workspace_id,
                    entity_types::COMMENT,
                    &comment_id,
                    SyncActionType::Insert,
                    serde_json::to_value(&comment).ok(),
                )
                .await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, comment_id = %comment_id, "Failed to resolve workspace for sync log");
        }
    }

    // Auto-watch: commenter watches the issue they commented on (best-effort).
    if let Err(e) = crate::watcher_service::watch_issue(db, issue_id, user_id).await {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to auto-watch issue for commenter");
    }

    // Notify watchers about the new comment (best-effort).
    if let Ok(ws_id) = &resolved_workspace_id {
        match crate::watcher_service::list_watchers_of_issue(db, issue_id).await {
            Ok(watchers) => {
                for watcher_id in &watchers {
                    if *watcher_id == user_id {
                        continue;
                    }
                    if let Err(e) = crate::notification_service::create_notification(
                        db, ws_id, watcher_id, issue_id,
                        crate::notification_service::TYPE_COMMENTED, Some(user_id), ws_manager,
                    ).await {
                        tracing::warn!(error = %e, "Failed to create comment notification");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, issue_id = %issue_id, "Failed to list watchers for comment notification");
            }
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
    let result = trakkt_core::db_execute!(db, &sql, body, comment_id, user_id)?;

    if result.rows_affected() == 0 {
        // Distinguish between "not found" and "not owned by user".
        let exists: i64 = trakkt_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM comments WHERE comment_id = $1",
            comment_id
        )?;
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
    let row = trakkt_core::db_fetch_one!(
        db,
        CommentRow,
        "SELECT c.comment_id, c.issue_id, c.user_id, c.body, c.parent_id, \
                u.name AS author_name, NULL AS author_avatar, \
                c.created_at, \
                c.updated_at \
         FROM comments c \
         JOIN users u ON u.user_id = c.user_id \
         WHERE c.comment_id = $1",
        comment_id
    )?;
    let comment = row.into_dto();

    // Sync log + broadcast — best-effort.
    match get_workspace_for_issue(db, &comment.issue_id).await {
        Ok(workspace_id) => {
            if let Err(e) = sync_log_service::write_sync_entry(
                db,
                entity_types::COMMENT,
                comment_id,
                &workspace_id,
                SyncActionType::Update,
                serde_json::to_value(&comment).ok(),
            )
            .await
            {
                tracing::warn!(error = %e, comment_id = %comment_id, "Failed to write sync log entry for comment update");
            }
            if let Some(ws) = ws_manager {
                sync_log_service::broadcast_sync_action(
                    ws,
                    &workspace_id,
                    entity_types::COMMENT,
                    comment_id,
                    SyncActionType::Update,
                    serde_json::to_value(&comment).ok(),
                )
                .await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, comment_id = %comment_id, "Failed to resolve workspace for sync log");
        }
    }

    Ok(comment)
}

/// Delete a comment. Only the comment's author can delete it.
pub async fn delete_comment(
    db: &DbPool,
    comment_id: &str,
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch the comment first for ownership check and sync log.
    let row = trakkt_core::db_fetch_optional!(
        db,
        CommentRow,
        "SELECT c.comment_id, c.issue_id, c.user_id, c.body, c.parent_id, \
                u.name AS author_name, NULL AS author_avatar, \
                c.created_at, \
                c.updated_at \
         FROM comments c \
         JOIN users u ON u.user_id = c.user_id \
         WHERE c.comment_id = $1",
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

    trakkt_core::db_execute!(
        db,
        "DELETE FROM comments WHERE comment_id = $1",
        comment_id
    )?;

    // Sync log + broadcast — best-effort.
    match get_workspace_for_issue(db, &comment.issue_id).await {
        Ok(workspace_id) => {
            if let Err(e) = sync_log_service::write_sync_entry(
                db,
                entity_types::COMMENT,
                comment_id,
                &workspace_id,
                SyncActionType::Delete,
                None,
            )
            .await
            {
                tracing::warn!(error = %e, comment_id = %comment_id, "Failed to write sync log entry for comment delete");
            }
            if let Some(ws) = ws_manager {
                sync_log_service::broadcast_sync_action(
                    ws,
                    &workspace_id,
                    entity_types::COMMENT,
                    comment_id,
                    SyncActionType::Delete,
                    None,
                )
                .await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, comment_id = %comment_id, "Failed to resolve workspace for sync log");
        }
    }

    Ok(())
}
