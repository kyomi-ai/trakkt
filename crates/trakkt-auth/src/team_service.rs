// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team service — CRUD operations for the `teams` table.
//!
//! Teams are workspace-scoped groups (e.g. "Engineering", "Design") that own
//! issues. Each team has a short `key` used as a prefix in issue identifiers
//! (e.g. ENG-42).

use trakkt_core::db::DbTx;
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

/// Base SELECT for single-team reads.
///
/// `member_count` is not computed here — only the list queries join
/// `team_members` to count it. Every single-team read has always reported 0,
/// and the sync payloads built from these reads carry that same 0.
const TEAM_SELECT: &str = "\
    SELECT team_id, workspace_id, name, key, description, icon, \
           icon_type, icon_name, icon_color, \
           CAST(0 AS BIGINT) AS member_count, \
           CAST(settings AS TEXT) AS settings, \
           CAST(created_at AS TEXT) AS created_at \
    FROM teams";

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

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Read a team by id on an open transaction.
///
/// Transaction-scoped [`get_team`], narrowed to the case every mutation below
/// needs: the team was just written, so a missing row is an error rather than
/// `None`. The read has to run on the transaction — the new state is not
/// visible on the pool until the commit, and on SQLite the pool is not
/// reachable at all while the transaction is open (see [`DbTx`]).
async fn get_team_tx(tx: &mut DbTx, team_id: &str) -> trakkt_core::Result<Team> {
    let sql = format!("{TEAM_SELECT} WHERE team_id = $1");
    let row: TeamRow = trakkt_core::tx_fetch_one!(&mut *tx, TeamRow, &sql, team_id)?;
    Ok(row.into_dto())
}

/// Finish a team mutation that has already run its UPDATE on `tx`: read the
/// team back, log the change, commit, then broadcast.
///
/// Every single-statement team update ends this way, and the ordering is the
/// part that has to be right every time — the read and the `sync_log` entry
/// inside the transaction so the change and the row that replays it commit
/// together, the broadcast strictly after the commit so it carries a `sync_id`
/// that exists and so it never runs while the transaction holds the SQLite
/// connection (see [`DbTx`]).
///
/// Takes the transaction by value: committing it is part of the job, and no
/// caller has anything left to do on it.
async fn commit_team_update(
    mut tx: DbTx,
    team_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Team> {
    // Read the updated row before the sync log write: both the stored entry and
    // the live frame carry the full team, and the client skips either without it.
    let team = get_team_tx(&mut tx, team_id).await?;
    let payload = team_payload_value(&team);

    let sync_id = sync_log_service::write_sync_entry_in_tx(
        &mut tx,
        entity_types::TEAM,
        team_id,
        workspace_id,
        None,
        SyncActionType::Update,
        payload.clone(),
    )
    .await?;

    tx.commit().await?;

    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::TEAM,
            team_id,
            SyncActionType::Update,
            payload,
            sync_id,
        )
        .await;
    }

    Ok(team)
}

/// Serialise a team into its sync payload.
///
/// A payload that cannot be serialised is logged and dropped: the sync entry is
/// still written, so the change keeps its place in the sequence. (The client
/// skips a TEAM entry with no payload, so the change itself is lost either way
/// — but the sequence stays intact and the next full read repairs it.)
fn team_payload_value(team: &Team) -> Option<serde_json::Value> {
    match serde_json::to_value(team) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(error = %e, team_id = %team.team_id,
                "Failed to serialize team for sync payload");
            None
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
/// The team INSERT, the member INSERT and both `sync_log` entries are one
/// transaction: a partial failure can neither leave an orphaned team row with
/// no creator membership, nor a team that exists with no sync row to carry it
/// to any other client.
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

    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &insert_team_sql,
        &team_id,
        params.workspace_id,
        params.name,
        params.key,
        params.description,
        params.icon
    )?;

    if let Some(uid) = params.creator_id {
        trakkt_core::tx_execute!(&mut tx, &insert_member_sql, &team_id, uid, "lead")?;
    }

    // Read the team back for the DB-assigned created_at. This has to happen
    // before the sync log writes: both stored entries and the live frame carry
    // the full team, and the client cannot apply any of them without it. The
    // row does not exist outside the transaction yet, so the read runs on it.
    let team = get_team_tx(&mut tx, &team_id).await?;
    let payload = team_payload_value(&team);

    // The broadcast below carries the Insert entry's sync_id: it is the Insert
    // frame, and a client that spots the gap re-fetches from there, which also
    // picks up the member-add Update entry written just after it.
    let sync_id = sync_log_service::write_sync_entry_in_tx(
        &mut tx,
        entity_types::TEAM,
        &team_id,
        params.workspace_id,
        None,
        SyncActionType::Insert,
        payload.clone(),
    )
    .await?;

    if params.creator_id.is_some() {
        sync_log_service::write_sync_entry_in_tx(
            &mut tx,
            entity_types::TEAM,
            &team_id,
            params.workspace_id,
            None,
            SyncActionType::Update,
            payload.clone(),
        )
        .await?;
    }

    tx.commit().await?;

    // The broadcast reaches for the socket, so it follows the commit and
    // carries the sync_id that was actually committed.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            params.workspace_id,
            entity_types::TEAM,
            &team_id,
            SyncActionType::Insert,
            payload,
            sync_id,
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
    let sql = format!("{TEAM_SELECT} WHERE team_id = $1");
    let row = trakkt_core::db_fetch_optional!(db, TeamRow, &sql, team_id)?;
    Ok(row.map(TeamRow::into_dto))
}

/// Get a team by ID, requiring it to belong to `workspace_id`.
///
/// Use this wherever the `team_id` came from the caller. [`get_team`] looks up
/// a team id across every workspace, so on its own it cannot tell a team the
/// caller owns from one they merely named — the caller has to compare
/// `workspace_id` afterwards, and forgetting to is silent.
///
/// A team in another workspace is reported as `NotFound`, not `Forbidden`: the
/// caller cannot see that workspace, and an error distinguishing "wrong
/// workspace" from "no such team" turns any team id into an existence oracle.
pub async fn get_team_in_workspace(
    db: &DbPool,
    team_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Team> {
    let sql = format!("{TEAM_SELECT} WHERE team_id = $1 AND workspace_id = $2");
    let row = trakkt_core::db_fetch_optional!(db, TeamRow, &sql, team_id, workspace_id)?;
    row.map(TeamRow::into_dto)
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("team {team_id} not found")))
}

/// Get a team by its unique workspace + key combination.
pub async fn get_team_by_key(
    db: &DbPool,
    workspace_id: &str,
    key: &str,
) -> trakkt_core::Result<Option<Team>> {
    let sql = format!("{TEAM_SELECT} WHERE workspace_id = $1 AND key = $2");
    let row = trakkt_core::db_fetch_optional!(db, TeamRow, &sql, workspace_id, key)?;
    Ok(row.map(TeamRow::into_dto))
}

/// Get the default (first-created) team in a workspace.
///
/// Returns `Error::NotFound` if the workspace has no teams.
pub async fn get_default_team(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Team> {
    let sql = format!("{TEAM_SELECT} WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1");
    let row = trakkt_core::db_fetch_optional!(db, TeamRow, &sql, workspace_id)?;
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

    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
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
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "team {team_id} not found"
        )));
    }

    commit_team_update(tx, team_id, workspace_id, ws_manager).await
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
    let mut tx = db.begin().await?;

    // When setting a preset, clear any custom upload data.
    let result = trakkt_core::tx_execute!(
        &mut tx,
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
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "team {team_id} not found"
        )));
    }

    commit_team_update(tx, team_id, workspace_id, ws_manager).await
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
    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE teams SET icon_type = 'custom', icon_name = NULL, icon_color = NULL, \
         icon_data = $1, icon_mime = $2 \
         WHERE team_id = $3 AND workspace_id = $4",
        data,
        mime,
        team_id,
        workspace_id
    )?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "team {team_id} not found"
        )));
    }

    commit_team_update(tx, team_id, workspace_id, ws_manager).await
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
    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE teams SET icon_type = NULL, icon_name = NULL, icon_color = NULL, \
         icon_data = NULL, icon_mime = NULL \
         WHERE team_id = $1 AND workspace_id = $2",
        team_id,
        workspace_id
    )?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "team {team_id} not found"
        )));
    }

    commit_team_update(tx, team_id, workspace_id, ws_manager).await
}

/// Delete a team and optionally reassign its issues to another team.
///
/// Steps, in order. Everything up to and including 5 runs on the pool; 6 is one
/// transaction; 7 follows its commit.
/// 1. Verify the team exists and belongs to this workspace
/// 2. Prevent deletion of the last team in a workspace
/// 3. Refuse to strand issues: a team with issues needs a reassign target
/// 4. Read what the reassignment needs (target team, issue ids, backlog status)
/// 5. Optionally set a new workspace default team
/// 6. In one transaction: reassign the issues to the target team with new
///    team-scoped numbers and a workspace-scoped status, one ISSUE sync entry
///    each; delete this team's `favorites` rows; clear `users.default_team_id`;
///    delete the team, which cascades `team_members` and its team-scoped
///    `statuses` and `labels`; write the TEAM delete sync entry
/// 7. Broadcast the reassignments and the delete via WebSocket
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

    // 4. Resolve everything the reassignment needs *before* the transaction
    //    opens. These are pool reads, and once a transaction is open the pool is
    //    unreachable on SQLite (see `DbTx`).
    let reassignment: Option<(&str, Vec<String>, String)> = match reassign_to_team_id {
        Some(target_team_id) => {
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

            Some((
                target_team_id,
                issue_rows.into_iter().map(|r| r.issue_id).collect(),
                default_status.status_id,
            ))
        }
        None => None,
    };

    // 5. Optionally set a new workspace default team.
    //
    // This runs on the pool, so it cannot move inside the transaction below, and
    // it does not need to: it writes no `sync_log` row of its own, so it is not
    // part of the atomicity this function owes the sync stream. It stays ahead
    // of the team delete exactly as it was, and touches only `workspaces` —
    // disjoint from every table the transaction writes.
    if let Some(new_default_id) = new_workspace_default_id {
        crate::workspace_service::set_workspace_default_team(db, workspace_id, new_default_id)
            .await?;
    }

    // 6. Everything that writes a `sync_log` row is one transaction: the issue
    //    reassignments, the cascade, and the two kinds of sync entry that report
    //    them. A cascade that half-commits leaves issues on a team that no
    //    longer exists, or a deleted team no client is ever told about.
    let mut tx = db.begin().await?;

    // Reassigned issues, held until the commit — the broadcasts below cannot run
    // while the transaction is open.
    let mut reassigned: Vec<(String, Option<serde_json::Value>, i64)> = Vec::new();

    if let Some((target_team_id, issue_ids, status_id)) = &reassignment {
        // Reassign each issue one at a time so team-scoped numbers are sequential.
        // Dialect comes from the transaction, not the pool: nothing inside this
        // span should have to reach for `db` at all (see `DbTx`).
        let now = sql_compat::now(tx.is_postgres());
        let sql = format!(
            "UPDATE issues SET team_id = $1, \
             number = (SELECT COALESCE(MAX(number), 0) + 1 FROM issues WHERE team_id = $1), \
             status_id = $3, \
             updated_at = {now} \
             WHERE issue_id = $2"
        );
        for issue_id in issue_ids {
            trakkt_core::tx_execute!(&mut tx, &sql, *target_team_id, issue_id, status_id)?;

            // Read the issue back before the sync log write. The reassignment
            // changed its team, number and status, and none of that reaches a
            // client without the payload — on the live frame or on delta. The
            // read runs on the transaction: the new state is not visible
            // anywhere else yet.
            let payload = crate::issue_service::issue_sync_payload_tx(&mut tx, issue_id).await?;

            let sync_id = sync_log_service::write_sync_entry_in_tx(
                &mut tx,
                entity_types::ISSUE,
                issue_id,
                workspace_id,
                None,
                SyncActionType::Update,
                payload.clone(),
            )
            .await?;

            reassigned.push((issue_id.clone(), payload, sync_id));
        }
    }

    // Delete favorites referencing this team. `favorites.target_id` is
    // polymorphic and carries no foreign key, so nothing cascades it.
    trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM favorites WHERE target_type = 'team' AND target_id = $1",
        team_id
    )?;

    // Clear default_team_id on any users who had this team as default.
    // `users.default_team_id` is a plain column with no foreign key either.
    trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE users SET default_team_id = NULL WHERE default_team_id = $1",
        team_id
    )?;

    // Delete the team. The schema cascades from here: `team_members`,
    // team-scoped `statuses` and team-scoped `labels` all declare
    // `ON DELETE CASCADE` on `teams(team_id)`. `issues` does not — which is why
    // the reassignment above is mandatory rather than a convenience.
    trakkt_core::tx_execute!(&mut tx, "DELETE FROM teams WHERE team_id = $1", team_id)?;

    // Sync log for the team delete, after the DELETE it describes — the same
    // order as `issue_service::delete_issue`.
    let sync_id = sync_log_service::write_sync_entry_in_tx(
        &mut tx,
        entity_types::TEAM,
        team_id,
        workspace_id,
        None,
        SyncActionType::Delete,
        None,
    )
    .await?;

    tx.commit().await?;

    // 7. Broadcast, now that every id above addresses a committed row.
    if let Some(ws) = ws_manager {
        for (issue_id, payload, issue_sync_id) in reassigned {
            sync_log_service::broadcast_sync_action(
                ws,
                workspace_id,
                entity_types::ISSUE,
                &issue_id,
                SyncActionType::Update,
                payload,
                issue_sync_id,
            )
            .await;
        }

        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::TEAM,
            team_id,
            SyncActionType::Delete,
            None,
            sync_id,
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

/// Record a team membership change on the sync log.
///
/// `team_members` is not a synced entity type of its own, so a membership change
/// is reported as an update to the parent team and has to carry the team row —
/// the shape the client's TEAM arm deserializes. An entry with no payload is
/// skipped outright by the client on both the live and the delta path.
///
/// The team is resolved by the caller before it mutates anything, so it is
/// passed in rather than read again here.
///
/// Written on the caller's transaction, alongside the `team_members` statement
/// it reports: a membership change with no sync row never reaches another
/// client, and no later delta can repair it because `team_members` is not a
/// synced entity type that a delta could re-read. Each of the three callers
/// therefore owns a transaction and hands it in — failing here rolls their
/// statement back.
///
/// Note the payload cannot express *what* changed. `Team` carries no member
/// list, and its `member_count` is reported as 0 by every single-team read. A
/// client applying this entry learns that the team changed, not how — the same
/// gap TRA-9940 records for project members.
async fn write_membership_sync_entry(
    tx: &mut DbTx,
    team: &Team,
    user_id: &str,
    operation: &str,
) -> trakkt_core::Result<()> {
    sync_log_service::write_sync_entry_in_tx(
        tx,
        entity_types::TEAM,
        &team.team_id,
        &team.workspace_id,
        None,
        SyncActionType::Update,
        team_payload_value(team),
    )
    .await
    // The underlying error names neither the team nor which membership change
    // was being reported, and the caller propagates rather than logs.
    .map_err(|e| {
        trakkt_core::Error::Internal(format!(
            "failed to write sync log for {operation} of user {user_id} on team {}: {e}",
            team.team_id
        ))
    })?;

    Ok(())
}

// `team_members` has no `workspace_id` column of its own — the only thing tying
// a membership row to a workspace is the `teams` row it points at. So each of
// the three mutations below resolves the team within `workspace_id` first and
// mutates nothing if that lookup fails. Filtering the statement alone would not
// do: a caller-supplied `team_id` names a row in *some* workspace, and without
// the resolve there is nothing in the statement to compare it against.

/// Add a user to a team. No-op if the user is already a member.
///
/// The team must belong to `workspace_id`; if it does not, this is `NotFound`
/// and nothing is written.
pub async fn add_team_member(
    db: &DbPool,
    team_id: &str,
    user_id: &str,
    role: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let team = get_team_in_workspace(db, team_id, workspace_id).await?;

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
    // The team resolve above ran on the pool; from here the insert and the sync
    // entry that reports it commit together or not at all.
    let mut tx = db.begin().await?;
    trakkt_core::tx_execute!(&mut tx, &sql, team_id, user_id, role)?;
    write_membership_sync_entry(&mut tx, &team, user_id, "member add").await?;
    tx.commit().await?;

    Ok(())
}

/// Remove a user from a team.
///
/// The team must belong to `workspace_id`; if it does not, this is `NotFound`
/// and nothing is deleted. Removing a user who is not a member remains a no-op.
pub async fn remove_team_member(
    db: &DbPool,
    team_id: &str,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let team = get_team_in_workspace(db, team_id, workspace_id).await?;

    let mut tx = db.begin().await?;
    trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM team_members WHERE team_id = $1 AND user_id = $2",
        team_id,
        user_id
    )?;
    write_membership_sync_entry(&mut tx, &team, user_id, "member remove").await?;
    tx.commit().await?;

    Ok(())
}

/// Update a team member's role.
///
/// The team must belong to `workspace_id`; if it does not, this is `NotFound`
/// and no role is changed.
pub async fn update_team_member_role(
    db: &DbPool,
    team_id: &str,
    user_id: &str,
    role: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let team = get_team_in_workspace(db, team_id, workspace_id).await?;

    let mut tx = db.begin().await?;
    trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE team_members SET role = $1 WHERE team_id = $2 AND user_id = $3",
        role,
        team_id,
        user_id
    )?;
    write_membership_sync_entry(&mut tx, &team, user_id, "member role update").await?;
    tx.commit().await?;

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
    let mut tx = db.begin().await?;
    let result = trakkt_core::tx_execute!(&mut tx, &sql, &settings_str, team_id, workspace_id)?;

    // No row matched — the team is not in this workspace. Nothing was written,
    // so there is nothing to report and nothing to commit.
    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }

    // The re-fetch is a plain read of a row this transaction just updated, so a
    // miss is no longer a case to warn past: it can only mean the read itself
    // failed, and continuing would commit a settings change with a sync entry
    // the client skips for having no payload.
    commit_team_update(tx, team_id, workspace_id, ws_manager).await?;

    Ok(true)
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

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trakkt_core::db_execute;

    const WS_A: &str = "ws_alpha";
    const WS_B: &str = "ws_beta";
    const USER_A: &str = "usr_alpha";
    const USER_B: &str = "usr_beta";
    const TEAM_A: &str = "team_alpha";
    const TEAM_B: &str = "team_beta";

    /// Two separate workspaces, one team and one member each.
    ///
    /// `USER_A` belongs to workspace A only, and is the attacker in the
    /// cross-workspace cases below: `TEAM_B` is a team they can name but must
    /// not be able to touch. `USER_B` is seeded into `TEAM_B` so the remove and
    /// role-update cases have a real membership row to try to disturb.
    async fn two_workspaces() -> DbPool {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");

        for user_id in [USER_A, USER_B] {
            db_execute!(
                &db,
                "INSERT INTO users (user_id, email, name) VALUES ($1, $2, $3)",
                user_id,
                format!("{user_id}@example.test"),
                user_id
            )
            .expect("insert user");
        }

        for (ws, owner) in [(WS_A, USER_A), (WS_B, USER_B)] {
            db_execute!(
                &db,
                "INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ($1, $2)",
                ws,
                owner
            )
            .expect("insert workspace");
            db_execute!(
                &db,
                "INSERT INTO workspace_users (workspace_id, user_id) VALUES ($1, $2)",
                ws,
                owner
            )
            .expect("insert workspace membership");
        }

        db_execute!(
            &db,
            "INSERT INTO teams (team_id, workspace_id, name, key) VALUES ($1, $2, $3, $4)",
            TEAM_A,
            WS_A,
            "Alpha",
            "ALP"
        )
        .expect("insert team A");
        db_execute!(
            &db,
            "INSERT INTO teams (team_id, workspace_id, name, key) VALUES ($1, $2, $3, $4)",
            TEAM_B,
            WS_B,
            "Beta",
            "BET"
        )
        .expect("insert team B");

        db_execute!(
            &db,
            "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3)",
            TEAM_B,
            USER_B,
            "member"
        )
        .expect("seed the existing membership in workspace B");

        db
    }

    /// Number of `team_members` rows for a team. Read straight from the table:
    /// the whole point is not to take the mutation's return value for it.
    async fn member_count(db: &DbPool, team_id: &str) -> i64 {
        trakkt_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM team_members WHERE team_id = $1",
            team_id
        )
        .expect("count team members")
    }

    async fn member_role(db: &DbPool, team_id: &str, user_id: &str) -> Option<String> {
        #[derive(sqlx::FromRow)]
        struct RoleRow {
            role: Option<String>,
        }
        let row = trakkt_core::db_fetch_optional!(
            db,
            RoleRow,
            "SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2",
            team_id,
            user_id
        )
        .expect("read member role");
        row.and_then(|r| r.role)
    }

    async fn sync_rows_for_workspace(db: &DbPool, workspace_id: &str) -> i64 {
        trakkt_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM sync_log WHERE workspace_id = $1",
            workspace_id
        )
        .expect("count sync log rows")
    }

    #[tokio::test]
    async fn add_team_member_refuses_a_team_in_another_workspace() {
        let db = two_workspaces().await;
        let before = member_count(&db, TEAM_B).await;

        let result = add_team_member(&db, TEAM_B, USER_A, "member", WS_A).await;

        assert!(
            matches!(result, Err(trakkt_core::Error::NotFound(_))),
            "a team id from another workspace must be indistinguishable from a \
             team id that does not exist, got {result:?}"
        );
        assert_eq!(
            member_count(&db, TEAM_B).await,
            before,
            "the foreign team's membership must be untouched"
        );
        assert_eq!(
            member_role(&db, TEAM_B, USER_A).await,
            None,
            "the caller must not have inserted themselves into the foreign team"
        );
        assert_eq!(
            sync_rows_for_workspace(&db, WS_B).await,
            0,
            "no sync_log row may be written into a workspace the caller cannot see"
        );
    }

    #[tokio::test]
    async fn remove_team_member_refuses_a_team_in_another_workspace() {
        let db = two_workspaces().await;

        let result = remove_team_member(&db, TEAM_B, USER_B, WS_A).await;

        assert!(
            matches!(result, Err(trakkt_core::Error::NotFound(_))),
            "expected NotFound, got {result:?}"
        );
        assert_eq!(
            member_count(&db, TEAM_B).await,
            1,
            "the seeded membership in the other workspace must survive"
        );
        assert_eq!(
            member_role(&db, TEAM_B, USER_B).await.as_deref(),
            Some("member"),
            "the seeded membership must survive intact"
        );
        assert_eq!(sync_rows_for_workspace(&db, WS_B).await, 0);
    }

    #[tokio::test]
    async fn update_team_member_role_refuses_a_team_in_another_workspace() {
        let db = two_workspaces().await;

        let result = update_team_member_role(&db, TEAM_B, USER_B, "lead", WS_A).await;

        assert!(
            matches!(result, Err(trakkt_core::Error::NotFound(_))),
            "expected NotFound, got {result:?}"
        );
        assert_eq!(
            member_role(&db, TEAM_B, USER_B).await.as_deref(),
            Some("member"),
            "the role in the other workspace must be unchanged"
        );
        assert_eq!(sync_rows_for_workspace(&db, WS_B).await, 0);
    }

    #[tokio::test]
    async fn membership_mutations_still_work_within_the_workspace() {
        let db = two_workspaces().await;
        assert_eq!(member_count(&db, TEAM_A).await, 0);

        add_team_member(&db, TEAM_A, USER_A, "member", WS_A)
            .await
            .expect("adding a member to a team in the caller's own workspace");
        assert_eq!(member_count(&db, TEAM_A).await, 1);
        assert_eq!(member_role(&db, TEAM_A, USER_A).await.as_deref(), Some("member"));

        update_team_member_role(&db, TEAM_A, USER_A, "lead", WS_A)
            .await
            .expect("promoting a member of a team in the caller's own workspace");
        assert_eq!(member_role(&db, TEAM_A, USER_A).await.as_deref(), Some("lead"));

        remove_team_member(&db, TEAM_A, USER_A, WS_A)
            .await
            .expect("removing a member of a team in the caller's own workspace");
        assert_eq!(member_count(&db, TEAM_A).await, 0);

        assert_eq!(
            sync_rows_for_workspace(&db, WS_A).await,
            3,
            "each of the three membership changes reports itself as a team update"
        );
    }

    #[tokio::test]
    async fn add_team_member_is_idempotent_within_the_workspace() {
        let db = two_workspaces().await;

        for _ in 0..2 {
            add_team_member(&db, TEAM_A, USER_A, "member", WS_A)
                .await
                .expect("re-adding an existing member stays a no-op, not an error");
        }
        assert_eq!(member_count(&db, TEAM_A).await, 1);
    }

    #[tokio::test]
    async fn get_team_in_workspace_hides_a_team_from_another_workspace() {
        let db = two_workspaces().await;

        assert!(
            get_team(&db, TEAM_B).await.expect("unscoped read").is_some(),
            "the team really does exist — the scoped read below has to be what hides it"
        );
        assert!(
            matches!(
                get_team_in_workspace(&db, TEAM_B, WS_A).await,
                Err(trakkt_core::Error::NotFound(_))
            ),
            "a team in another workspace must read as missing"
        );

        let team = get_team_in_workspace(&db, TEAM_A, WS_A)
            .await
            .expect("a team in the caller's own workspace resolves");
        assert_eq!(team.team_id, TEAM_A);
        assert_eq!(team.workspace_id, WS_A);
    }
}
