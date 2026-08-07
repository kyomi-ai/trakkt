// SPDX-License-Identifier: AGPL-3.0-or-later

//! Attachment service — CRUD operations for the `attachments` table.
//!
//! Attachments are workspace-scoped file records. The actual file data is stored
//! via `AttachmentStorage` (local filesystem or S3), while this service manages
//! the database records and validation logic.
//!
//! Nothing in this module touches object storage. The blob writes live in
//! `trakkt-api`'s attachment handlers, outside any transaction, ordered so a
//! failure wastes storage rather than leaving a row pointing at a blob that is
//! not there — see `crates/trakkt-api/src/attachments.rs`.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;

use crate::sync_log_service;
use crate::websocket::WebSocketManager;
use trakkt_types::models::IssueAttachment;
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

/// Internal row type for deserialising `issue_attachments` junction rows.
#[derive(sqlx::FromRow)]
struct IssueAttachmentRow {
    issue_id: String,
    attachment_id: String,
    created_at: String,
}

impl IssueAttachmentRow {
    fn into_dto(self) -> IssueAttachment {
        IssueAttachment {
            issue_id: self.issue_id,
            attachment_id: self.attachment_id,
            created_at: self.created_at,
        }
    }
}

/// Base SELECT for the `issue_attachments` junction table.
///
/// `created_at` is `TIMESTAMPTZ` on Postgres and TEXT on SQLite while the row
/// type declares `String` for both, so the cast is not cosmetic — see the JSONB
/// note in `docs/CODING_STANDARDS.md`, which is the same failure in a different
/// column type.
const ISSUE_ATTACHMENT_SELECT: &str = "\
    SELECT issue_id, attachment_id, \
           CAST(created_at AS TEXT) AS created_at \
    FROM issue_attachments";

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
///
/// The INSERT and its `sync_log` entry are one transaction: an attachment that
/// commits without its sync row is invisible to every future delta, so a failed
/// log write rolls the record back rather than leaving it stranded.
///
/// The stored blob is written by the caller *before* this function runs, so a
/// rollback here leaves an orphaned blob and no row — wasted storage, which is
/// the failure this ordering is chosen for. The reverse order would leave a row
/// pointing at a blob that was never stored.
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
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &attachment_id,
        workspace_id,
        filename,
        content_type,
        size_bytes,
        storage_path,
        uploaded_by
    )?;

    // Re-fetch to get the DB-assigned created_at. The row does not exist outside
    // the transaction yet, so the read runs on it.
    let row: AttachmentRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        AttachmentRow,
        "SELECT attachment_id, workspace_id, filename, content_type, size_bytes, storage_path, uploaded_by, \
                CAST(created_at AS TEXT) AS created_at \
         FROM attachments WHERE attachment_id = $1",
        &attachment_id
    )?;
    let attachment = row.into_dto();
    let payload =
        sync_log_service::sync_payload(&attachment, entity_types::ATTACHMENT, &attachment_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::ATTACHMENT,
        &attachment_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

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
///
/// The DELETE, the `issue_attachments` links the schema cascades with it, and
/// the `sync_log` entry that reports it are one transaction. A delete that
/// commits without its sync row leaves the attachment on every other client
/// forever, and no later delta can repair it — the row it would have to re-read
/// is gone.
///
/// The `storage_path` is returned only on success, so an `Err` here stops the
/// caller's blob delete at its `?`: the row and its blob survive together rather
/// than the blob being destroyed under a row that was rolled back.
pub async fn delete_attachment(
    db: &DbPool,
    attachment_id: &str,
    workspace_id: &str,
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<String> {
    // Fetch the attachment first to check ownership. This reads state that
    // predates the transaction, and the check below is authorization, so both
    // stay on the pool ahead of `begin` — once the transaction is open the pool
    // is unreachable on SQLite (see `DbTx`).
    let attachment = get_attachment(db, attachment_id, workspace_id).await?;

    // Only the uploader can delete (workspace admin check happens at the API layer)
    if attachment.uploaded_by != user_id {
        return Err(trakkt_core::Error::Forbidden(
            "Only the uploader can delete this attachment".into(),
        ));
    }

    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM attachments WHERE attachment_id = $1 AND workspace_id = $2",
        attachment_id,
        workspace_id
    )?;

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::ATTACHMENT,
        attachment_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Delete,
        None,
        ws_manager,
    )
    .await?;

    Ok(attachment.storage_path)
}

// ─── Issue attachment linking ──────────────────────────────────────────────

/// Attach an existing attachment to an issue.
///
/// Verifies that the attachment belongs to the given workspace, then inserts
/// into the `issue_attachments` junction table. Uses `ON CONFLICT DO NOTHING`
/// so re-attaching is idempotent.
///
/// The INSERT and its `sync_log` entry are one transaction. `issue_attachments`
/// is not an entity type a delta re-reads, so a link that commits without its
/// sync row can never be repaired — it simply never reaches another client.
///
/// The entry carries the junction row itself, and has to. An insert entry with
/// no payload is dropped by `apply_action_to_memory`
/// (`crates/trakkt-ui/src/cache/apply.rs`) at its data-less guard, which runs
/// before the entity-type match — so the ISSUE_ATTACHMENT arm there depends on
/// this call sending one. That is the whole of TRA-9979.
///
/// The audience is [`SyncAudience::Workspace`](sync_log_service::SyncAudience)
/// because the read path is: [`list_issue_attachments`] filters on
/// `workspace_id` and nothing else, and its only caller resolves that id from
/// the session's workspace membership. Every workspace member may already read
/// this link, so nothing is disclosed by broadcasting it — and both sibling
/// writes (`detach_from_issue`, `create_attachment`) are `Workspace` too.
pub async fn attach_to_issue(
    db: &DbPool,
    workspace_id: &str,
    issue_id: &str,
    attachment_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Verify attachment belongs to workspace. This is authorization over state
    // that predates the transaction, so it stays on the pool ahead of `begin`
    // (see `DbTx`) — as does the issue check below.
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
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(&mut tx, &sql, issue_id, attachment_id)?;

    // Re-read the junction row just written, for its DB-assigned timestamp, and
    // send it as the payload — exactly as `project_service::add_project_member`
    // does for the membership row it writes. Both the stored entry and the live
    // frame carry it, and the client skips either without it:
    // `apply_action_to_memory` (`crates/trakkt-ui/src/cache/apply.rs`) returns at
    // its data-less guard *before* the entity-type match, so a `None` here made
    // the ISSUE_ATTACHMENT insert arm unreachable and a link to an existing file
    // reached no other client at all, live or on reconnect.
    //
    // The read runs on the transaction: the row does not exist on the pool until
    // the commit, and on SQLite the pool is not reachable at all while the
    // transaction is open (`max_connections(1)` — see `DbTx`).
    //
    // `ON CONFLICT DO NOTHING` above means the INSERT may have been a no-op, in
    // which case this reads the row the earlier link left. That is the right
    // answer for an idempotent call: the frame describes the link that now
    // exists, and re-announcing it costs a client one refetch of a list it
    // already agrees with.
    let sql = format!("{ISSUE_ATTACHMENT_SELECT} WHERE issue_id = $1 AND attachment_id = $2");
    let row: IssueAttachmentRow =
        trakkt_core::tx_fetch_one!(&mut tx, IssueAttachmentRow, &sql, issue_id, attachment_id)?;
    let link = row.into_dto();

    let entity_id = format!("{issue_id}:{attachment_id}");
    let payload = sync_log_service::sync_payload(&link, entity_types::ISSUE_ATTACHMENT, &entity_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::ISSUE_ATTACHMENT,
        &entity_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(())
}

/// Detach an attachment from an issue.
///
/// Removes the row from the `issue_attachments` junction table. Does NOT
/// delete the attachment record itself.
///
/// The DELETE and its `sync_log` entry are one transaction — an unlink that
/// commits without its sync row leaves the attachment hanging off the issue on
/// every other client forever, and no later delta can repair it.
pub async fn detach_from_issue(
    db: &DbPool,
    workspace_id: &str,
    issue_id: &str,
    attachment_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Authorization over state that predates the transaction, so it stays on the
    // pool ahead of `begin` (see `DbTx`).
    let _attachment = get_attachment(db, attachment_id, workspace_id).await?;

    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM issue_attachments WHERE issue_id = $1 AND attachment_id = $2",
        issue_id,
        attachment_id
    )?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "Attachment '{attachment_id}' is not linked to issue '{issue_id}'"
        )));
    }

    let entity_id = format!("{issue_id}:{attachment_id}");

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::ISSUE_ATTACHMENT,
        &entity_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Delete,
        None,
        ws_manager,
    )
    .await?;

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
