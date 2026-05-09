// SPDX-License-Identifier: AGPL-3.0-or-later

//! Status service — CRUD operations for the `statuses` table.
//!
//! Statuses are workspace-scoped (optionally team-scoped) workflow states that
//! issues progress through. Each status belongs to a category (backlog,
//! unstarted, started, completed, cancelled) and has a position within that
//! category for ordering.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::Status;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `statuses` query results.
#[derive(sqlx::FromRow)]
struct StatusRow {
    status_id: String,
    workspace_id: String,
    team_id: Option<String>,
    name: String,
    category: String,
    position: i32,
    color: Option<String>,
    created_at: String,
}

impl StatusRow {
    fn into_dto(self) -> Status {
        Status {
            status_id: self.status_id,
            workspace_id: self.workspace_id,
            team_id: self.team_id,
            name: self.name,
            category: self.category,
            position: self.position,
            color: self.color,
            created_at: self.created_at,
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// List statuses for a workspace, optionally filtered by team.
///
/// Returns global statuses (team_id IS NULL) and, if `team_id` is provided,
/// also includes team-specific statuses. Results are ordered by category then
/// position.
pub async fn list_statuses(
    db: &DbPool,
    workspace_id: &str,
    team_id: Option<&str>,
) -> trakkt_core::Result<Vec<Status>> {
    let rows: Vec<StatusRow> = match team_id {
        Some(tid) => {
            trakkt_core::db_fetch_all!(
                db,
                StatusRow,
                "SELECT status_id, workspace_id, team_id, name, category, position, color, \
                        CAST(created_at AS TEXT) AS created_at \
                 FROM statuses \
                 WHERE workspace_id = $1 AND (team_id IS NULL OR team_id = $2) \
                 ORDER BY CASE category \
                     WHEN 'backlog' THEN 0 WHEN 'unstarted' THEN 1 \
                     WHEN 'started' THEN 2 WHEN 'completed' THEN 3 \
                     WHEN 'cancelled' THEN 4 ELSE 5 END, position",
                workspace_id,
                tid
            )?
        }
        None => {
            trakkt_core::db_fetch_all!(
                db,
                StatusRow,
                "SELECT status_id, workspace_id, team_id, name, category, position, color, \
                        CAST(created_at AS TEXT) AS created_at \
                 FROM statuses \
                 WHERE workspace_id = $1 AND team_id IS NULL \
                 ORDER BY CASE category \
                     WHEN 'backlog' THEN 0 WHEN 'unstarted' THEN 1 \
                     WHEN 'started' THEN 2 WHEN 'completed' THEN 3 \
                     WHEN 'cancelled' THEN 4 ELSE 5 END, position",
                workspace_id
            )?
        }
    };
    Ok(rows.into_iter().map(StatusRow::into_dto).collect())
}

/// Get the default status for a workspace (first backlog status, global only).
///
/// Returns `Error::NotFound` if no backlog status exists.
pub async fn get_default_status(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Status> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        StatusRow,
        "SELECT status_id, workspace_id, team_id, name, category, position, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM statuses \
         WHERE workspace_id = $1 AND team_id IS NULL AND category = 'backlog' \
         ORDER BY position ASC LIMIT 1",
        workspace_id
    )?;
    match row {
        Some(r) => Ok(r.into_dto()),
        None => Err(trakkt_core::Error::NotFound(format!(
            "no default status found in workspace {workspace_id}"
        ))),
    }
}

/// Create a new status in a workspace.
pub async fn create_status(
    db: &DbPool,
    workspace_id: &str,
    team_id: Option<&str>,
    name: &str,
    category: &str,
    position: i32,
    color: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Status> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let status_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO statuses (status_id, workspace_id, team_id, name, category, position, color, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &status_id,
        workspace_id,
        team_id,
        name,
        category,
        position,
        color
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::STATUS,
        &status_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, status_id = %status_id, "Failed to write sync log entry for status create");
    }

    // WebSocket broadcast — best-effort.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_notify(ws, entity_types::STATUS, workspace_id).await;
    }

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        StatusRow,
        "SELECT status_id, workspace_id, team_id, name, category, position, color, \
                CAST(created_at AS TEXT) AS created_at \
         FROM statuses WHERE status_id = $1",
        &status_id
    )?;
    Ok(row.into_dto())
}

/// Seed the 6 default global statuses for a workspace.
///
/// Uses deterministic IDs (`{workspace_id}::backlog`, etc.) and conflict-safe
/// INSERT so the function is idempotent.
pub async fn seed_default_statuses(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    let statuses: &[(&str, &str, &str, i32)] = &[
        ("backlog",      "Backlog",     "backlog",    0),
        ("triage",       "Triage",      "backlog",    1),
        ("todo",         "Todo",        "unstarted",  0),
        ("in_progress",  "In Progress", "started",    0),
        ("done",         "Done",        "completed",  0),
        ("cancelled",    "Cancelled",   "cancelled",  0),
    ];

    for (suffix, name, category, position) in statuses {
        let status_id = format!("{workspace_id}::{suffix}");

        let sql = if is_pg {
            format!(
                "INSERT INTO statuses (status_id, workspace_id, team_id, name, category, position, color, created_at) \
                 VALUES ($1, $2, NULL, $3, $4, $5, NULL, {now}) \
                 ON CONFLICT DO NOTHING"
            )
        } else {
            format!(
                "INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, color, created_at) \
                 VALUES ($1, $2, NULL, $3, $4, $5, NULL, {now})"
            )
        };

        trakkt_core::db_execute!(
            db,
            &sql,
            &status_id,
            workspace_id,
            name,
            category,
            position
        )?;
    }

    Ok(())
}
