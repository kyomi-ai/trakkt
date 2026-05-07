// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team service — CRUD operations for the `teams` table.
//!
//! Teams are workspace-scoped groups (e.g. "Engineering", "Design") that own
//! issues. Each team has a short `key` used as a prefix in issue identifiers
//! (e.g. ENG-42).

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::Team;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `teams` query results.
#[derive(sqlx::FromRow)]
struct TeamRow {
    team_id: String,
    workspace_id: String,
    name: String,
    key: String,
    created_at: String,
}

impl TeamRow {
    fn into_dto(self) -> Team {
        Team {
            team_id: self.team_id,
            workspace_id: self.workspace_id,
            name: self.name,
            key: self.key,
            created_at: self.created_at,
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new team in a workspace.
pub async fn create_team(
    db: &DbPool,
    workspace_id: &str,
    name: &str,
    key: &str,
) -> trakkt_core::Result<Team> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let team_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO teams (team_id, workspace_id, name, key, created_at) \
         VALUES ($1, $2, $3, $4, {now})"
    );
    trakkt_core::db_execute!(db, &sql, &team_id, workspace_id, name, key)?;

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        &team_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log entry for team create");
    }

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE team_id = $1",
        &team_id
    )?;
    Ok(row.into_dto())
}

/// List all teams in a workspace, ordered by creation date.
pub async fn list_teams(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Team>> {
    let rows: Vec<TeamRow> = trakkt_core::db_fetch_all!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE workspace_id = $1 ORDER BY created_at ASC",
        workspace_id
    )?;
    Ok(rows.into_iter().map(TeamRow::into_dto).collect())
}

/// Get a single team by ID.
pub async fn get_team(
    db: &DbPool,
    team_id: &str,
) -> trakkt_core::Result<Option<Team>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE team_id = $1",
        team_id
    )?;
    Ok(row.map(TeamRow::into_dto))
}

/// Get a team by its unique workspace + key combination.
pub async fn get_team_by_key(
    db: &DbPool,
    workspace_id: &str,
    key: &str,
) -> trakkt_core::Result<Option<Team>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE workspace_id = $1 AND key = $2",
        workspace_id,
        key
    )?;
    Ok(row.map(TeamRow::into_dto))
}

/// Get the default (first-created) team in a workspace.
///
/// Returns `Error::NotFound` if the workspace has no teams.
pub async fn get_default_team(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Team> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1",
        workspace_id
    )?;
    match row {
        Some(r) => Ok(r.into_dto()),
        None => Err(trakkt_core::Error::NotFound(format!(
            "no teams found in workspace {workspace_id}"
        ))),
    }
}
