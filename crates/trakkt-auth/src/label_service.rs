// SPDX-License-Identifier: AGPL-3.0-or-later

//! Label service — CRUD operations for the `labels` table.
//!
//! Labels are workspace-scoped and can be attached to issues via the
//! `issue_labels` junction table. Each label has a name and color.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::Label;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `labels` query results.
#[derive(sqlx::FromRow)]
struct LabelRow {
    label_id: String,
    workspace_id: String,
    name: String,
    color: String,
    created_at: String,
}

impl LabelRow {
    fn into_dto(self) -> Label {
        Label {
            label_id: self.label_id,
            workspace_id: self.workspace_id,
            name: self.name,
            color: self.color,
            created_at: self.created_at,
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new label in a workspace.
pub async fn create_label(
    db: &DbPool,
    workspace_id: &str,
    name: &str,
    color: &str,
) -> trakkt_core::Result<Label> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let label_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO labels (label_id, workspace_id, name, color, created_at) \
         VALUES ($1, $2, $3, $4, {now})"
    );
    trakkt_core::db_execute!(db, &sql, &label_id, workspace_id, name, color)?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::LABEL,
        &label_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, label_id = %label_id, "Failed to write sync log entry for label create");
    }

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE label_id = $1",
        &label_id
    )?;
    Ok(row.into_dto())
}

/// List all labels in a workspace, ordered by name.
pub async fn list_labels(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Label>> {
    let rows: Vec<LabelRow> = trakkt_core::db_fetch_all!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE workspace_id = $1 ORDER BY name ASC",
        workspace_id
    )?;
    Ok(rows.into_iter().map(LabelRow::into_dto).collect())
}

/// Get a single label by its ID.
pub async fn get_label_by_id(
    db: &DbPool,
    label_id: &str,
) -> trakkt_core::Result<Option<Label>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE label_id = $1",
        label_id
    )?;
    Ok(row.map(LabelRow::into_dto))
}

/// Update a label's name and color.
pub async fn update_label(
    db: &DbPool,
    label_id: &str,
    name: &str,
    color: &str,
) -> trakkt_core::Result<Label> {
    let result = trakkt_core::db_execute!(
        db,
        "UPDATE labels SET name = $1, color = $2 WHERE label_id = $3",
        name,
        color,
        label_id
    )?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "label {label_id} not found"
        )));
    }

    // Fetch the updated row (need workspace_id for sync log).
    let row = trakkt_core::db_fetch_one!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE label_id = $1",
        label_id
    )?;
    let label = row.into_dto();

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::LABEL,
        label_id,
        &label.workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, label_id = %label_id, "Failed to write sync log entry for label update");
    }

    Ok(label)
}

/// Delete a label.
///
/// Cascading deletes remove associated `issue_labels` rows.
pub async fn delete_label(
    db: &DbPool,
    label_id: &str,
) -> trakkt_core::Result<()> {
    // Fetch workspace_id before delete for the sync log.
    let row = trakkt_core::db_fetch_optional!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE label_id = $1",
        label_id
    )?;

    let label = row.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!("label {label_id} not found"))
    })?;

    trakkt_core::db_execute!(
        db,
        "DELETE FROM labels WHERE label_id = $1",
        label_id
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::LABEL,
        label_id,
        &label.workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, label_id = %label_id, "Failed to write sync log entry for label delete");
    }

    Ok(())
}

/// Get all labels attached to a specific issue.
pub async fn get_issue_labels(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<Vec<Label>> {
    let rows: Vec<LabelRow> = trakkt_core::db_fetch_all!(
        db,
        LabelRow,
        "SELECT l.label_id, l.workspace_id, l.name, l.color, \
                CAST(l.created_at AS TEXT) AS created_at \
         FROM labels l \
         JOIN issue_labels il ON l.label_id = il.label_id \
         WHERE il.issue_id = $1 \
         ORDER BY l.name ASC",
        issue_id
    )?;
    Ok(rows.into_iter().map(LabelRow::into_dto).collect())
}
