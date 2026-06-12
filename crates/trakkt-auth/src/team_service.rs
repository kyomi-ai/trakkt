// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team service — CRUD operations for the `teams` table.
//!
//! Teams are workspace-scoped groups (e.g. "Engineering", "Design") that own
//! issues. Each team has a short `key` used as a prefix in issue identifiers
//! (e.g. ENG-42).

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::{IssueTeamMember, Team, TeamSettings, WorkspaceSettings};
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
    icon_type: Option<String>,
    icon_name: Option<String>,
    icon_color: Option<String>,
    member_count: i64,
    settings: Option<String>,
    created_at: String,
}

impl TeamRow {
    fn into_dto(self) -> Team {
        let settings = self.settings.and_then(|s| match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, team_id = %self.team_id, "Failed to deserialize team settings");
                None
            }
        });
        Team {
            team_id: self.team_id,
            workspace_id: self.workspace_id,
            name: self.name,
            key: self.key,
            description: self.description,
            icon: self.icon,
            icon_type: self.icon_type,
            icon_name: self.icon_name,
            icon_color: self.icon_color,
            member_count: self.member_count,
            settings,
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
///
/// The team INSERT and member INSERT are wrapped in a transaction so a partial
/// failure cannot leave an orphaned team row with no creator membership.
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

    let insert_team_sql = format!(
        "INSERT INTO teams (team_id, workspace_id, name, key, description, icon, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {now})"
    );

    let insert_member_sql = if is_pg {
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

    // Run both inserts in a single transaction so we never get an orphaned team
    // without its creator membership.
    match db {
        DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await.map_err(|e| {
                trakkt_core::Error::Internal(format!("failed to begin transaction: {e}"))
            })?;
            sqlx::query(&insert_team_sql)
                .bind(&team_id)
                .bind(params.workspace_id)
                .bind(params.name)
                .bind(params.key)
                .bind(params.description)
                .bind(params.icon)
                .execute(&mut *tx)
                .await?;
            if let Some(uid) = params.creator_id {
                sqlx::query(&insert_member_sql)
                    .bind(&team_id)
                    .bind(uid)
                    .bind("lead")
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await.map_err(|e| {
                trakkt_core::Error::Internal(format!("failed to commit transaction: {e}"))
            })?;
        }
        DbPool::Sqlite(sq) => {
            let mut tx = sq.begin().await.map_err(|e| {
                trakkt_core::Error::Internal(format!("failed to begin transaction: {e}"))
            })?;
            sqlx::query(&insert_team_sql)
                .bind(&team_id)
                .bind(params.workspace_id)
                .bind(params.name)
                .bind(params.key)
                .bind(params.description)
                .bind(params.icon)
                .execute(&mut *tx)
                .await?;
            if let Some(uid) = params.creator_id {
                sqlx::query(&insert_member_sql)
                    .bind(&team_id)
                    .bind(uid)
                    .bind("lead")
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await.map_err(|e| {
                trakkt_core::Error::Internal(format!("failed to commit transaction: {e}"))
            })?;
        }
    }

    // Sync log + broadcast happen after commit — these are best-effort and
    // should not roll back an otherwise-successful team creation.
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

    if params.creator_id.is_some() {
        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            entity_types::TEAM,
            &team_id,
            params.workspace_id,
            SyncActionType::Update,
            None,
        )
        .await
        {
            tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log for team member add");
        }
    }

    // Re-fetch to get the DB-assigned created_at.
    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, description, icon, \
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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

/// List teams in a workspace, ordered alphabetically by name.
///
/// When `user_id` is `Some`, only teams the user belongs to are returned
/// (INNER JOIN on `team_members`). When `None`, all teams are returned
/// (for admin/internal use).
pub async fn list_teams(
    db: &DbPool,
    workspace_id: &str,
    user_id: Option<&str>,
) -> trakkt_core::Result<Vec<Team>> {
    match user_id {
        Some(uid) => {
            let rows: Vec<TeamRow> = trakkt_core::db_fetch_all!(
                db,
                TeamRow,
                "SELECT t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                        t.icon_type, t.icon_name, t.icon_color, \
                        COUNT(tm2.user_id) AS member_count, \
                        CAST(t.settings AS TEXT) AS settings, \
                        CAST(t.created_at AS TEXT) AS created_at \
                 FROM teams t \
                 INNER JOIN team_members tm ON tm.team_id = t.team_id AND tm.user_id = $2 \
                 LEFT JOIN team_members tm2 ON tm2.team_id = t.team_id \
                 WHERE t.workspace_id = $1 \
                 GROUP BY t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                          t.icon_type, t.icon_name, t.icon_color, t.settings, t.created_at \
                 ORDER BY t.name ASC",
                workspace_id,
                uid
            )?;
            Ok(rows.into_iter().map(TeamRow::into_dto).collect())
        }
        None => {
            let rows: Vec<TeamRow> = trakkt_core::db_fetch_all!(
                db,
                TeamRow,
                "SELECT t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                        t.icon_type, t.icon_name, t.icon_color, \
                        COUNT(tm.user_id) AS member_count, \
                        CAST(t.settings AS TEXT) AS settings, \
                        CAST(t.created_at AS TEXT) AS created_at \
                 FROM teams t \
                 LEFT JOIN team_members tm ON tm.team_id = t.team_id \
                 WHERE t.workspace_id = $1 \
                 GROUP BY t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                          t.icon_type, t.icon_name, t.icon_color, t.settings, t.created_at \
                 ORDER BY t.name ASC",
                workspace_id
            )?;
            Ok(rows.into_iter().map(TeamRow::into_dto).collect())
        }
    }
}

/// List teams in a workspace that the user is NOT a member of.
///
/// Used for the "join team" flow — shows teams available to join.
pub async fn list_joinable_teams(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> trakkt_core::Result<Vec<Team>> {
    let rows: Vec<TeamRow> = trakkt_core::db_fetch_all!(
        db,
        TeamRow,
        "SELECT t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                t.icon_type, t.icon_name, t.icon_color, \
                COUNT(tm.user_id) AS member_count, \
                CAST(t.settings AS TEXT) AS settings, \
                CAST(t.created_at AS TEXT) AS created_at \
         FROM teams t \
         LEFT JOIN team_members tm ON tm.team_id = t.team_id \
         WHERE t.workspace_id = $1 \
           AND t.team_id NOT IN (SELECT tm2.team_id FROM team_members tm2 WHERE tm2.user_id = $2) \
         GROUP BY t.team_id, t.workspace_id, t.name, t.key, t.description, t.icon, \
                  t.icon_type, t.icon_name, t.icon_color, t.settings, t.created_at \
         ORDER BY t.name ASC",
        workspace_id,
        user_id
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
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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

/// Get the default team for a user in a workspace, with three-tier resolution:
///
/// 1. User's personal `default_team_id` — if set and the team exists in this workspace
/// 2. Workspace-level `default_team_id` — if set and the team exists
/// 3. First-created team in the workspace (existing fallback)
pub async fn get_user_default_team(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Team> {
    // Tier 1: user's personal default
    if let Some(user) = crate::user_service::get_user_by_id(db, user_id).await?
        && let Some(ref tid) = user.default_team_id
        && let Some(team) = get_team(db, tid).await?
        && team.workspace_id == workspace_id
    {
        return Ok(team);
    }

    // Tier 2: workspace default
    if let Some(ref tid) =
        crate::workspace_service::get_workspace_default_team_id(db, workspace_id).await?
        && let Some(team) = get_team(db, tid).await?
        && team.workspace_id == workspace_id
    {
        return Ok(team);
    }

    // Tier 3: first-created team (existing fallback)
    get_default_team(db, workspace_id).await
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
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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

// ─── Icon management ────────────────────────────────────────────────────────

/// Update a team's icon to a preset (icon_type = "preset") or clear it.
///
/// Pass `None` for all three fields to clear the icon entirely.
pub async fn update_team_icon(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
    icon_type: Option<&str>,
    icon_name: Option<&str>,
    icon_color: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Team> {
    // When setting a preset, clear any custom upload data.
    let result = trakkt_core::db_execute!(
        db,
        "UPDATE teams SET icon_type = $1, icon_name = $2, icon_color = $3, \
         icon_data = NULL, icon_mime = NULL \
         WHERE team_id = $4 AND workspace_id = $5",
        icon_type,
        icon_name,
        icon_color,
        team_id,
        workspace_id
    )?;

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
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log for team icon update");
    }

    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, description, icon, \
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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

/// Upload a custom image as a team icon.
///
/// Sets `icon_type = "custom"` and stores the binary data + MIME type.
/// Clears `icon_name` and `icon_color` since those are preset-only fields.
pub async fn upload_team_icon(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
    data: &[u8],
    mime: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Team> {
    let result = trakkt_core::db_execute!(
        db,
        "UPDATE teams SET icon_type = 'custom', icon_name = NULL, icon_color = NULL, \
         icon_data = $1, icon_mime = $2 \
         WHERE team_id = $3 AND workspace_id = $4",
        data,
        mime,
        team_id,
        workspace_id
    )?;

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
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log for team icon upload");
    }

    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, description, icon, \
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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

/// Fetch a team's custom icon binary data and MIME type.
///
/// Returns `None` if no custom icon is uploaded (`icon_data` is NULL).
pub async fn get_team_icon_data(
    db: &DbPool,
    team_id: &str,
) -> trakkt_core::Result<Option<(Vec<u8>, String)>> {
    #[derive(sqlx::FromRow)]
    struct IconDataRow {
        icon_data: Option<Vec<u8>>,
        icon_mime: Option<String>,
    }

    let row = trakkt_core::db_fetch_optional!(
        db,
        IconDataRow,
        "SELECT icon_data, icon_mime FROM teams WHERE team_id = $1",
        team_id
    )?;

    match row {
        Some(r) => match (r.icon_data, r.icon_mime) {
            (Some(data), Some(mime)) => Ok(Some((data, mime))),
            _ => Ok(None),
        },
        None => Err(trakkt_core::Error::NotFound(format!(
            "team {team_id} not found"
        ))),
    }
}

/// Remove a team's icon entirely (clears all icon fields).
pub async fn delete_team_icon(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Team> {
    let result = trakkt_core::db_execute!(
        db,
        "UPDATE teams SET icon_type = NULL, icon_name = NULL, icon_color = NULL, \
         icon_data = NULL, icon_mime = NULL \
         WHERE team_id = $1 AND workspace_id = $2",
        team_id,
        workspace_id
    )?;

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
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log for team icon delete");
    }

    let row = trakkt_core::db_fetch_one!(
        db,
        TeamRow,
        "SELECT team_id, workspace_id, name, key, description, icon, \
                icon_type, icon_name, icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(settings AS TEXT) AS settings, \
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

/// Delete a team and optionally reassign its issues to another team.
///
/// Steps:
/// 1. Verify the team exists and belongs to this workspace
/// 2. Prevent deletion of the last team in a workspace
/// 3. Reassign issues to target team (with new team-scoped numbers) if requested
/// 4. Delete favorites referencing this team
/// 5. Clear `default_team_id` on users who had this team as default
/// 6. Optionally set a new workspace default team
/// 7. Write sync log + delete the team (cascades team_members, labels, statuses)
/// 8. Broadcast delete via WebSocket
pub async fn delete_team(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
    reassign_to_team_id: Option<&str>,
    new_workspace_default_id: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // 1. Verify team exists and belongs to workspace
    let team = get_team(db, team_id).await?.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!("team {team_id} not found"))
    })?;
    if team.workspace_id != workspace_id {
        return Err(trakkt_core::Error::BadRequest(
            "Team does not belong to this workspace".into(),
        ));
    }

    // 2. Count teams — refuse to delete the last one
    let team_count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM teams WHERE workspace_id = $1",
        workspace_id
    )?;
    if team_count <= 1 {
        return Err(trakkt_core::Error::BadRequest(
            "Cannot delete the only team in a workspace".into(),
        ));
    }

    // 3. Check for issues and reassign if needed
    let issue_count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM issues WHERE team_id = $1",
        team_id
    )?;

    if issue_count > 0 && reassign_to_team_id.is_none() {
        return Err(trakkt_core::Error::BadRequest(format!(
            "Team has {issue_count} issues. Provide a target team to reassign them to."
        )));
    }

    if let Some(target_team_id) = reassign_to_team_id {
        // Verify target team exists in this workspace
        let target = get_team(db, target_team_id).await?.ok_or_else(|| {
            trakkt_core::Error::NotFound(format!(
                "reassign target team {target_team_id} not found"
            ))
        })?;
        if target.workspace_id != workspace_id {
            return Err(trakkt_core::Error::BadRequest(
                "Target team does not belong to this workspace".into(),
            ));
        }

        // Fetch issue IDs belonging to the team being deleted.
        #[derive(sqlx::FromRow)]
        struct IssueIdRow {
            issue_id: String,
        }
        let issue_rows: Vec<IssueIdRow> = trakkt_core::db_fetch_all!(
            db,
            IssueIdRow,
            "SELECT issue_id FROM issues WHERE team_id = $1 ORDER BY number ASC",
            team_id
        )?;

        // Find the default (backlog) status in the workspace for reassigned issues.
        // Team-scoped statuses will be cascaded when the team is deleted, so we
        // must move issues to a workspace-scoped status to avoid FK violations.
        #[derive(sqlx::FromRow)]
        struct StatusIdRow {
            status_id: String,
        }
        let default_status = trakkt_core::db_fetch_optional!(
            db,
            StatusIdRow,
            "SELECT status_id FROM statuses \
             WHERE workspace_id = $1 AND team_id IS NULL AND category = 'backlog' \
             ORDER BY position ASC LIMIT 1",
            workspace_id
        )?
        .ok_or_else(|| {
            trakkt_core::Error::Internal("no workspace-scoped backlog status found".into())
        })?;

        // Reassign each issue one at a time so team-scoped numbers are sequential.
        let is_pg = db.is_postgres();
        let now = sql_compat::now(is_pg);
        for row in &issue_rows {
            let sql = format!(
                "UPDATE issues SET team_id = $1, \
                 number = (SELECT COALESCE(MAX(number), 0) + 1 FROM issues WHERE team_id = $1), \
                 status_id = $3, \
                 updated_at = {now} \
                 WHERE issue_id = $2"
            );
            trakkt_core::db_execute!(db, &sql, target_team_id, &row.issue_id, &default_status.status_id)?;

            // Sync log for each moved issue — best-effort.
            if let Err(e) = sync_log_service::write_sync_entry(
                db,
                entity_types::ISSUE,
                &row.issue_id,
                workspace_id,
                SyncActionType::Update,
                None,
            )
            .await
            {
                tracing::warn!(error = %e, issue_id = %row.issue_id, "Failed to write sync log for issue reassignment");
            }

            // Broadcast issue update
            if let Some(ws) = ws_manager {
                sync_log_service::broadcast_sync_action(
                    ws,
                    workspace_id,
                    entity_types::ISSUE,
                    &row.issue_id,
                    SyncActionType::Update,
                    None,
                )
                .await;
            }
        }
    }

    // 4. Delete favorites referencing this team
    trakkt_core::db_execute!(
        db,
        "DELETE FROM favorites WHERE target_type = 'team' AND target_id = $1",
        team_id
    )?;

    // 5. Clear default_team_id on any users who had this team as default
    trakkt_core::db_execute!(
        db,
        "UPDATE users SET default_team_id = NULL WHERE default_team_id = $1",
        team_id
    )?;

    // 6. Optionally set a new workspace default team
    if let Some(new_default_id) = new_workspace_default_id {
        crate::workspace_service::set_workspace_default_team(db, workspace_id, new_default_id)
            .await?;
    }

    // 7. Sync log for team delete — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::TEAM,
        team_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log entry for team delete");
    }

    // Delete the team (cascades team_members, team-scoped labels, team-scoped statuses)
    trakkt_core::db_execute!(
        db,
        "DELETE FROM teams WHERE team_id = $1",
        team_id
    )?;

    // 8. Broadcast delete via WebSocket
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::TEAM,
            team_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
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
                t.icon_type, t.icon_name, t.icon_color, \
                CAST(0 AS BIGINT) AS member_count, \
                CAST(t.settings AS TEXT) AS settings, \
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

// ─── Team settings ─────────────────────────────────────────────────────────

/// Update a team's settings JSON (full replace).
///
/// The `teams.settings` column is Postgres JSONB. We serialize to text and
/// cast in SQL (same pattern as `workspace_service::update_workspace_settings`).
pub async fn update_team_settings(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
    settings: &TeamSettings,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<bool> {
    let is_pg = db.is_postgres();
    let json_cast = sql_compat::cast_to_json(is_pg, "$1");
    let settings_str = serde_json::to_string(settings)
        .map_err(|e| trakkt_core::Error::Internal(format!("JSON serialization failed: {e}")))?;
    let sql = format!(
        "UPDATE teams SET settings = {json_cast} WHERE team_id = $2 AND workspace_id = $3"
    );
    let result = trakkt_core::db_execute!(db, &sql, &settings_str, team_id, workspace_id)?;

    if result.rows_affected() > 0 {
        let team_data = match get_team(db, team_id).await {
            Ok(Some(t)) => serde_json::to_value(&t).ok(),
            Ok(None) => {
                tracing::warn!(team_id, "update_team_settings: team not found after write");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, team_id, "update_team_settings: re-fetch failed");
                None
            }
        };

        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            entity_types::TEAM,
            team_id,
            workspace_id,
            SyncActionType::Update,
            team_data.clone(),
        )
        .await
        {
            tracing::warn!(error = %e, team_id = %team_id, "Failed to write sync log entry for team settings update");
        }

        if let Some(ws) = ws_manager {
            sync_log_service::broadcast_sync_action(
                ws,
                workspace_id,
                entity_types::TEAM,
                team_id,
                SyncActionType::Update,
                team_data,
            )
            .await;
        }
    }

    Ok(result.rows_affected() > 0)
}

/// Resolve the effective auto-archive-days for a team.
///
/// Resolution order:
/// 1. Team's own `auto_archive_days` setting (if > 0)
/// 2. Workspace-level `default_auto_archive_days` (if > 0)
/// 3. `None` — archiving is disabled
pub async fn get_team_archive_days(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Option<u32>> {
    #[derive(sqlx::FromRow)]
    struct SettingsRow {
        settings: Option<String>,
    }

    // 1. Try team-level setting.
    let team_row = trakkt_core::db_fetch_optional!(
        db,
        SettingsRow,
        "SELECT CAST(settings AS TEXT) AS settings FROM teams WHERE team_id = $1 AND workspace_id = $2",
        team_id,
        workspace_id
    )?;

    if let Some(row) = team_row
        && let Some(ref json_str) = row.settings
    {
        match serde_json::from_str::<TeamSettings>(json_str) {
            Ok(ts) => {
                if let Some(days) = ts.auto_archive_days
                    && days > 0
                {
                    return Ok(Some(days));
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    team_id = %team_id,
                    "Failed to parse team settings for archive days"
                );
            }
        }
    }

    // 2. Fall back to workspace default.
    let ws_row = trakkt_core::db_fetch_optional!(
        db,
        SettingsRow,
        "SELECT CAST(settings AS TEXT) AS settings FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;

    if let Some(row) = ws_row
        && let Some(ref json_str) = row.settings
    {
        match serde_json::from_str::<WorkspaceSettings>(json_str) {
            Ok(ws) => {
                if let Some(days) = ws.default_auto_archive_days
                    && days > 0
                {
                    return Ok(Some(days));
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    workspace_id = %workspace_id,
                    "Failed to parse workspace settings for archive days"
                );
            }
        }
    }

    Ok(None)
}
