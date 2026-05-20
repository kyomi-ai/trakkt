// SPDX-License-Identifier: AGPL-3.0-or-later

//! Attachment service — CRUD operations for the `attachments` table.
//!
//! Attachments are workspace-scoped file records. The actual file data is stored
//! via `AttachmentStorage` (local filesystem or S3), while this service manages
//! the database records and validation logic.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;

use crate::sync_log_service;
use crate::websocket::WebSocketManager;
use trakkt_types::sync::{SyncActionType, entity_types};

const MAX_FILE_SIZE: usize = 10 * 1024 * 1024; // 10MB

const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
    "application/pdf",
    "text/csv",
    "text/plain",
    "application/json",
];

const ALLOWED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "pdf", "csv", "txt", "json", "log",
];

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `attachments` query results.
#[derive(sqlx::FromRow)]
struct AttachmentRow {
    attachment_id: String,
    workspace_id: String,
    filename: String,
    content_type: String,
    size_bytes: i64,
    storage_path: String,
    uploaded_by: String,
    created_at: String,
}

/// Public DTO for attachment data.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Attachment {
    pub attachment_id: String,
    pub workspace_id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub storage_path: String,
    pub uploaded_by: String,
    pub created_at: String,
}

impl AttachmentRow {
    fn into_dto(self) -> Attachment {
        Attachment {
            attachment_id: self.attachment_id,
            workspace_id: self.workspace_id,
            filename: self.filename,
            content_type: self.content_type,
            size_bytes: self.size_bytes,
            storage_path: self.storage_path,
            uploaded_by: self.uploaded_by,
            created_at: self.created_at,
        }
    }
}

// ─── Validation ──────────────────────────────────────────────────────────────

/// Validate file type against allowlist.
/// Returns Ok(()) if both the content_type and file extension are allowed.
pub fn validate_file_type(filename: &str, content_type: &str) -> trakkt_core::Result<()> {
    if !ALLOWED_CONTENT_TYPES.contains(&content_type) {
        return Err(trakkt_core::Error::BadRequest(format!(
            "Content type '{content_type}' is not allowed. Allowed types: images (png, jpg, gif, webp, svg), documents (pdf), data (csv, txt, json)"
        )));
    }

    let extension = filename.rsplit('.').next().unwrap_or("").to_lowercase();

    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(trakkt_core::Error::BadRequest(format!(
            "File extension '.{extension}' is not allowed. Allowed extensions: {}",
            ALLOWED_EXTENSIONS.join(", ")
        )));
    }

    Ok(())
}

/// Validate file size.
pub fn validate_file_size(size: usize) -> trakkt_core::Result<()> {
    if size > MAX_FILE_SIZE {
        return Err(trakkt_core::Error::BadRequest(format!(
            "File size {} bytes exceeds maximum allowed size of {} bytes (10MB)",
            size, MAX_FILE_SIZE
        )));
    }
    Ok(())
}

// ─── Service functions ───────────────────────────────────────────────────────

/// Insert a new attachment record.
pub async fn create_attachment(
    db: &DbPool,
    workspace_id: &str,
    filename: &str,
    content_type: &str,
    size_bytes: i64,
    storage_path: &str,
    uploaded_by: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Attachment> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let attachment_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO attachments (attachment_id, workspace_id, filename, content_type, size_bytes, storage_path, uploaded_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &attachment_id,
        workspace_id,
        filename,
        content_type,
        size_bytes,
        storage_path,
        uploaded_by
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ATTACHMENT,
        &attachment_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, attachment_id = %attachment_id, "Failed to write sync log entry for attachment create");
    }

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        AttachmentRow,
        "SELECT attachment_id, workspace_id, filename, content_type, size_bytes, storage_path, uploaded_by, \
                CAST(created_at AS TEXT) AS created_at \
         FROM attachments WHERE attachment_id = $1",
        &attachment_id
    )?;
    let attachment = row.into_dto();

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ATTACHMENT,
            &attachment_id,
            SyncActionType::Insert,
            serde_json::to_value(&attachment).ok(),
        )
        .await;
    }

    Ok(attachment)
}

/// Get a single attachment by ID, scoped to workspace.
pub async fn get_attachment(
    db: &DbPool,
    attachment_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Attachment> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        AttachmentRow,
        "SELECT attachment_id, workspace_id, filename, content_type, size_bytes, storage_path, uploaded_by, \
                CAST(created_at AS TEXT) AS created_at \
         FROM attachments WHERE attachment_id = $1 AND workspace_id = $2",
        attachment_id,
        workspace_id
    )?;

    row.map(AttachmentRow::into_dto).ok_or_else(|| {
        trakkt_core::Error::NotFound(format!("Attachment '{attachment_id}' not found"))
    })
}

/// List all attachments in a workspace.
pub async fn list_attachments(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Attachment>> {
    let rows: Vec<AttachmentRow> = trakkt_core::db_fetch_all!(
        db,
        AttachmentRow,
        "SELECT attachment_id, workspace_id, filename, content_type, size_bytes, storage_path, uploaded_by, \
                CAST(created_at AS TEXT) AS created_at \
         FROM attachments WHERE workspace_id = $1 ORDER BY created_at DESC",
        workspace_id
    )?;

    Ok(rows.into_iter().map(AttachmentRow::into_dto).collect())
}

/// Delete an attachment record.
/// Returns the storage_path so the caller can delete the stored file.
pub async fn delete_attachment(
    db: &DbPool,
    attachment_id: &str,
    workspace_id: &str,
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<String> {
    // Fetch the attachment first to check ownership
    let attachment = get_attachment(db, attachment_id, workspace_id).await?;

    // Only the uploader can delete (workspace admin check happens at the API layer)
    if attachment.uploaded_by != user_id {
        return Err(trakkt_core::Error::Forbidden(
            "Only the uploader can delete this attachment".into(),
        ));
    }

    trakkt_core::db_execute!(
        db,
        "DELETE FROM attachments WHERE attachment_id = $1 AND workspace_id = $2",
        attachment_id,
        workspace_id
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ATTACHMENT,
        attachment_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, attachment_id = %attachment_id, "Failed to write sync log entry for attachment delete");
    }

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ATTACHMENT,
            attachment_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(attachment.storage_path)
}

// ─── Issue attachment linking ──────────────────────────────────────────────

/// Attach an existing attachment to an issue.
///
/// Verifies that the attachment belongs to the given workspace, then inserts
/// into the `issue_attachments` junction table. Uses `ON CONFLICT DO NOTHING`
/// so re-attaching is idempotent.
pub async fn attach_to_issue(
    db: &DbPool,
    workspace_id: &str,
    issue_id: &str,
    attachment_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Verify attachment belongs to workspace.
    let _attachment = get_attachment(db, attachment_id, workspace_id).await?;

    // Verify issue belongs to workspace.
    let count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM issues WHERE issue_id = $1 AND workspace_id = $2",
        issue_id,
        workspace_id
    )?;
    if count == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "Issue '{issue_id}' not found in workspace"
        )));
    }

    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    let sql = format!(
        "INSERT INTO issue_attachments (issue_id, attachment_id, created_at) \
         VALUES ($1, $2, {now}) \
         ON CONFLICT (issue_id, attachment_id) DO NOTHING"
    );
    trakkt_core::db_execute!(db, &sql, issue_id, attachment_id)?;

    let entity_id = format!("{issue_id}:{attachment_id}");

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE_ATTACHMENT,
        &entity_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, %entity_id, "Failed to write sync log entry for issue attachment link");
    }

    // WebSocket broadcast.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ISSUE_ATTACHMENT,
            &entity_id,
            SyncActionType::Insert,
            None,
        )
        .await;
    }

    Ok(())
}

/// Detach an attachment from an issue.
///
/// Removes the row from the `issue_attachments` junction table. Does NOT
/// delete the attachment record itself.
pub async fn detach_from_issue(
    db: &DbPool,
    workspace_id: &str,
    issue_id: &str,
    attachment_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let _attachment = get_attachment(db, attachment_id, workspace_id).await?;

    let result = trakkt_core::db_execute!(
        db,
        "DELETE FROM issue_attachments WHERE issue_id = $1 AND attachment_id = $2",
        issue_id,
        attachment_id
    )?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "Attachment '{attachment_id}' is not linked to issue '{issue_id}'"
        )));
    }

    let entity_id = format!("{issue_id}:{attachment_id}");

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE_ATTACHMENT,
        &entity_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, %entity_id, "Failed to write sync log entry for issue attachment unlink");
    }

    // WebSocket broadcast.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ISSUE_ATTACHMENT,
            &entity_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
}

/// List all attachments linked to an issue.
///
/// Joins `issue_attachments` with `attachments` and returns full attachment
/// metadata, ordered by link creation time (newest first).
pub async fn list_issue_attachments(
    db: &DbPool,
    workspace_id: &str,
    issue_id: &str,
) -> trakkt_core::Result<Vec<Attachment>> {
    let rows: Vec<AttachmentRow> = trakkt_core::db_fetch_all!(
        db,
        AttachmentRow,
        "SELECT a.attachment_id, a.workspace_id, a.filename, a.content_type, \
                a.size_bytes, a.storage_path, a.uploaded_by, \
                CAST(a.created_at AS TEXT) AS created_at \
         FROM attachments a \
         JOIN issue_attachments ia ON a.attachment_id = ia.attachment_id \
         WHERE ia.issue_id = $1 AND a.workspace_id = $2 \
         ORDER BY ia.created_at DESC",
        issue_id,
        workspace_id
    )?;

    Ok(rows.into_iter().map(AttachmentRow::into_dto).collect())
}
