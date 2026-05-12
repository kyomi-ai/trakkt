// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team service — CRUD operations for the `teams` table.
//!
//! Teams are workspace-scoped groups (e.g. "Engineering", "Design") that own
//! issues. Each team has a short `key` used as a prefix in issue identifiers
//! (e.g. ENG-42).

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::{IssueTeamMember, Team};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `teams` query results.
#[derive(sqlx::FromRow)]
struct TeamRow {
    team_id: String,
    workspace_id: String,
    name: String,
    key: String,
    description: Option<String>,
    icon: Option<String>,
    member_count: i64,
    created_at: String,
}

impl TeamRow {
    fn into_dto(self) -> Team {
        Team {
            team_id: self.team_id,
            workspace_id: self.workspace_id,
            name: self.name,
            key: self.key,
            description: self.description,
            icon: self.icon,
            member_count: self.member_count,
            created_at: self.created_at,
        }
    }
}

/// Internal row type for deserialising `team_members` JOIN query results.
#[derive(sqlx::FromRow)]
struct TeamMemberRow {
    team_id: String,
    user_id: String,
    user_name: Option<String>,
    user_email: String,
    role: String,
    created_at: String,
}

impl TeamMemberRow {
    fn into_dto(self) -> IssueTeamMember {
        IssueTeamMember {
            team_id: self.team_id,
            user_id: self.user_id,
            user_name: self.user_name,
            user_email: self.user_email,
            role: self.role,
            created_at: self.created_at,
        }
    }
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Parameters for creating a new team.
pub struct CreateTeamParams<'a> {
    pub workspace_id: &'a str,
    pub name: &'a str,
    pub key: &'a str,
    pub description: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub creator_id: Option<&'a str>,
}

/// Create a new team in a workspace.
///
/// If `params.creator_id` is provided, the creator is automatically added as a `lead` member.
pub async fn create_team(
    db: &DbPool,
    params: &CreateTeamParams<'_>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Team> {
    // Validate key format: 2-5 uppercase alphanumeric characters (no hyphens).
    if params.key.len() < 2
        || params.key.len() > 5
        || !params.key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(trakkt_core::Error::BadRequest(
            "Team key must be 2-5 uppercase alphanumeric characters".into(),
        ));
    }

    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let team_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO teams (team_id, workspace_id, name, key, description, icon, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {now})"
    );
    trakkt_core::db_execute!(db, &sql, &team_id, params.workspace_id, params.name, params.key, params.description, params.icon)?;

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        &team_id,
        params.workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log entry for team create");
    }

    // Auto-add creator as lead member if provided.
    if let Some(uid) = params.creator_id {
        add_team_member(db, &team_id, uid, "lead", params.workspace_id).await?;
    }

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, description, icon, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE team_id = $1",
        &team_id
    )?;
    let team = row.into_dto();

    // WebSocket broadcast — send full entity data so clients update immediately.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            params.workspace_id,
            entity_types::TEAM,
            &team_id,
            SyncActionType::Insert,
            serde_json::to_value(&team).ok(),
        )
        .await;
    }

    Ok(team)
}

/// List all teams in a workspace, ordered alphabetically by name.
pub async fn list_teams(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Team>> {
    let rows: Vec<TeamRow> = trakkt_core::db_fetch_all!(
        db,
        TeamRow,
        "SELECT t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                COUNT(tm.user_id) AS member_count, \
                CAST(t.created_at AS TEXT) AS created_at \
         FROM teams t \
         LEFT JOIN team_members tm ON tm.team_id = t.team_id \
         WHERE t.workspace_id = $1 \
         GROUP BY t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, t.created_at \
         ORDER BY t.name ASC",
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
        "SELECT team_id, workspace_id, name, key, description, icon, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(created_at AS TEXT) AS created_at \
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
        "SELECT team_id, workspace_id, name, key, description, icon, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(created_at AS TEXT) AS created_at \
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
        "SELECT team_id, workspace_id, name, key, description, icon, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(created_at AS TEXT) AS created_at \
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

/// Update a team's name and/or key.
///
/// Only provided fields are changed (COALESCE pattern). The DB UNIQUE
/// constraint on `(workspace_id, key)` rejects duplicate keys automatically.
pub async fn update_team(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
    name: Option<String>,
    key: Option<String>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Team> {
    if let Some(ref k) = key
        && (k.len() < 2
            || k.len() > 5
            || !k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
    {
        return Err(trakkt_core::Error::BadRequest(
            "Team key must be 2-5 uppercase alphanumeric characters".into(),
        ));
    }

    let result = trakkt_core::db_execute!(
        db,
        "UPDATE teams SET name = COALESCE($1, name), key = COALESCE($2, key) \
         WHERE team_id = $3 AND workspace_id = $4",
        name,
        key,
        team_id,
        workspace_id
    )
    .map_err(|e| {
        // Catch UNIQUE constraint violations and return a user-friendly error.
        // Postgres: code "23505", SQLite: code "2067" or message contains "UNIQUE constraint".
        if let sqlx::Error::Database(ref db_err) = e {
            let is_unique = db_err
                .code()
                .map(|c| c == "23505" || c == "2067")
                .unwrap_or(false)
                || db_err.message().contains("UNIQUE constraint");
            if is_unique {
                return trakkt_core::Error::Conflict(
                    "A team with that key already exists".into(),
                );
            }
        }
        trakkt_core::Error::from(e)
    })?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "team {team_id} not found"
        )));
    }

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        team_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log entry for team update");
    }

    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, description, icon, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(created_at AS TEXT) AS created_at \
         FROM teams WHERE team_id = $1",
        team_id
    )?;
    let team = row.into_dto();

    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::TEAM,
            team_id,
            SyncActionType::Update,
            serde_json::to_value(&team).ok(),
        )
        .await;
    }

    Ok(team)
}

// ─── Team membership ────────────────────────────────────────────────────────

/// List all members of a team, with joined user details.
pub async fn list_team_members(
    db: &DbPool,
    team_id: &str,
) -> trakkt_core::Result<Vec<IssueTeamMember>> {
    let rows: Vec<TeamMemberRow> = trakkt_core::db_fetch_all!(
        db,
        TeamMemberRow,
        "SELECT tm.team_id, tm.user_id, u.name AS user_name, u.email AS user_email, \
                tm.role, CAST(tm.created_at AS TEXT) AS created_at \
         FROM team_members tm \
         JOIN users u ON u.user_id = tm.user_id \
         WHERE tm.team_id = $1 \
         ORDER BY tm.created_at ASC",
        team_id
    )?;
    Ok(rows.into_iter().map(TeamMemberRow::into_dto).collect())
}

/// Add a user to a team. No-op if the user is already a member.
pub async fn add_team_member(
    db: &DbPool,
    team_id: &str,
    user_id: &str,
    role: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    let sql = if is_pg {
        format!(
            "INSERT INTO team_members (team_id, user_id, role, created_at) \
             VALUES ($1, $2, $3, {now}) \
             ON CONFLICT DO NOTHING"
        )
    } else {
        format!(
            "INSERT OR IGNORE INTO team_members (team_id, user_id, role, created_at) \
             VALUES ($1, $2, $3, {now})"
        )
    };
    trakkt_core::db_execute!(db, &sql, team_id, user_id, role)?;

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        team_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, user_id = %user_id, "Failed to write sync log for team member add");
    }

    Ok(())
}

/// Remove a user from a team.
pub async fn remove_team_member(
    db: &DbPool,
    team_id: &str,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "DELETE FROM team_members WHERE team_id = $1 AND user_id = $2",
        team_id,
        user_id
    )?;

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        team_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, user_id = %user_id, "Failed to write sync log for team member remove");
    }

    Ok(())
}

/// Update a team member's role.
pub async fn update_team_member_role(
    db: &DbPool,
    team_id: &str,
    user_id: &str,
    role: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "UPDATE team_members SET role = $1 WHERE team_id = $2 AND user_id = $3",
        role,
        team_id,
        user_id
    )?;

    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        team_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, user_id = %user_id, "Failed to write sync log for team member role update");
    }

    Ok(())
}

/// Get all teams a user belongs to within a workspace.
pub async fn get_user_teams(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> trakkt_core::Result<Vec<Team>> {
    let rows: Vec<TeamRow> = trakkt_core::db_fetch_all!(
        db,
        TeamRow,
        "SELECT t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(t.created_at AS TEXT) AS created_at \
         FROM teams t \
         JOIN team_members tm ON tm.team_id = t.team_id \
         WHERE t.workspace_id = $1 AND tm.user_id = $2 \
         ORDER BY t.created_at ASC",
        workspace_id,
        user_id
    )?;
    Ok(rows.into_iter().map(TeamRow::into_dto).collect())
}
