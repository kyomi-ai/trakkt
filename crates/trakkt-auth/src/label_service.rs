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
use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `labels` query results.
#[derive(sqlx::FromRow)]
struct LabelRow {
    label_id: String,
    workspace_id: String,
    team_id: Option<String>,
    name: String,
    color: String,
    created_at: String,
}

impl LabelRow {
    fn into_dto(self) -> Label {
        Label {
            label_id: self.label_id,
            workspace_id: self.workspace_id,
            team_id: self.team_id,
            name: self.name,
            color: self.color,
            created_at: self.created_at,
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new label in a workspace, optionally scoped to a team.
///
/// If `team_id` is `None`, the label is workspace-scoped (available to all teams).
/// If `team_id` is `Some(...)`, the label is team-scoped (only available within that team).
pub async fn create_label(
    db: &DbPool,
    workspace_id: &str,
    name: &str,
    color: &str,
    team_id: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Label> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let label_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO labels (label_id, workspace_id, team_id, name, color, created_at) \
         VALUES ($1, $2, $3, $4, $5, {now})"
    );
    trakkt_core::db_execute!(db, &sql, &label_id, workspace_id, team_id, name, color)?;

    // Sync log — best-effort.
    let sync_id = sync_log_service::write_sync_entry(
        db,
        entity_types::LABEL,
        &label_id,
        workspace_id,
        None,
        SyncActionType::Insert,
        None,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, label_id = %label_id, "Failed to write sync log entry for label create");
        0
    });

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, team_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE label_id = $1",
        &label_id
    )?;
    let label = row.into_dto();

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::LABEL,
            &label_id,
            SyncActionType::Insert,
            serde_json::to_value(&label).ok(),
            sync_id,
        )
        .await;
    }

    Ok(label)
}

/// List all labels in a workspace, ordered by name.
pub async fn list_labels(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Label>> {
    let rows: Vec<LabelRow> = trakkt_core::db_fetch_all!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, team_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE workspace_id = $1 ORDER BY name ASC",
        workspace_id
    )?;
    Ok(rows.into_iter().map(LabelRow::into_dto).collect())
}

/// List labels available for a specific team.
///
/// Returns workspace-level labels (`team_id IS NULL`) plus team-specific labels
/// (`team_id = team_id`). This gives teams access to shared workspace labels
/// alongside their own team-scoped labels.
pub async fn list_labels_for_team(
    db: &DbPool,
    workspace_id: &str,
    team_id: &str,
) -> trakkt_core::Result<Vec<Label>> {
    let rows: Vec<LabelRow> = trakkt_core::db_fetch_all!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, team_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels \
         WHERE workspace_id = $1 AND (team_id IS NULL OR team_id = $2) \
         ORDER BY name ASC",
        workspace_id,
        team_id
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
        "SELECT label_id, workspace_id, team_id, name, color, \
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
    ws_manager: Option<&WebSocketManager>,
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
        "SELECT label_id, workspace_id, team_id, name, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM labels WHERE label_id = $1",
        label_id
    )?;
    let label = row.into_dto();

    // Sync log — best-effort.
    let sync_id = sync_log_service::write_sync_entry(
        db,
        entity_types::LABEL,
        label_id,
        &label.workspace_id,
        None,
        SyncActionType::Update,
        None,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, label_id = %label_id, "Failed to write sync log entry for label update");
        0
    });

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &label.workspace_id,
            entity_types::LABEL,
            label_id,
            SyncActionType::Update,
            serde_json::to_value(&label).ok(),
            sync_id,
        )
        .await;
    }

    Ok(label)
}

/// Delete a label.
///
/// Cascading deletes remove associated `issue_labels` rows.
pub async fn delete_label(
    db: &DbPool,
    label_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch workspace_id before delete for the sync log.
    let row = trakkt_core::db_fetch_optional!(
        db,
        LabelRow,
        "SELECT label_id, workspace_id, team_id, name, color, \
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
    let sync_id = sync_log_service::write_sync_entry(
        db,
        entity_types::LABEL,
        label_id,
        &label.workspace_id,
        None,
        SyncActionType::Delete,
        None,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, label_id = %label_id, "Failed to write sync log entry for label delete");
        0
    });

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &label.workspace_id,
            entity_types::LABEL,
            label_id,
            SyncActionType::Delete,
            None,
            sync_id,
        )
        .await;
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
        "SELECT l.label_id, l.workspace_id, l.team_id, l.name, l.color, \
                CAST(l.created_at AS TEXT) AS created_at \
         FROM labels l \
         JOIN issue_labels il ON l.label_id = il.label_id \
         WHERE il.issue_id = $1 \
         ORDER BY l.name ASC",
        issue_id
    )?;
    Ok(rows.into_iter().map(LabelRow::into_dto).collect())
}
