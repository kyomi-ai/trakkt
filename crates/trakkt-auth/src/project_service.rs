// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project service — CRUD operations for the `projects`, `project_members`,
//! `project_milestones`, and `project_updates` tables.
//!
//! Projects are workspace-scoped containers that group issues towards a common
//! goal. Each project can have members, milestones, and periodic health updates.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::{Project, ProjectMember, ProjectMilestone, ProjectProgress, ProjectUpdate};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising `projects` query results.
///
/// `sort_order` is stored as REAL in the database. sqlx reads
/// it as `String` via CAST and parse to f64 in `into_dto()` to avoid sqlx
/// type mismatch between backends.
#[derive(sqlx::FromRow)]
struct ProjectRow {
    project_id: String,
    workspace_id: String,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    status: String,
    lead_id: Option<String>,
    start_date: Option<String>,
    target_date: Option<String>,
    sort_order: f64,
    created_at: String,
    updated_at: String,
}

impl ProjectRow {
    fn into_dto(self) -> Project {
        Project {
            project_id: self.project_id,
            workspace_id: self.workspace_id,
            name: self.name,
            description: self.description,
            icon: self.icon,
            color: self.color,
            status: self.status,
            lead_id: self.lead_id,
            sort_order: self.sort_order,
            start_date: self.start_date,
            target_date: self.target_date,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Internal row type for deserialising `project_members` query results.
#[derive(sqlx::FromRow)]
struct ProjectMemberRow {
    project_id: String,
    user_id: String,
    role: String,
    created_at: String,
}

impl ProjectMemberRow {
    fn into_dto(self) -> ProjectMember {
        ProjectMember {
            project_id: self.project_id,
            user_id: self.user_id,
            role: self.role,
            created_at: self.created_at,
        }
    }
}

/// Internal row type for deserialising `project_milestones` query results.
#[derive(sqlx::FromRow)]
struct MilestoneRow {
    milestone_id: String,
    project_id: String,
    name: String,
    description: Option<String>,
    target_date: Option<String>,
    sort_order: i32,
    created_at: String,
}

impl MilestoneRow {
    fn into_dto(self) -> ProjectMilestone {
        ProjectMilestone {
            milestone_id: self.milestone_id,
            project_id: self.project_id,
            name: self.name,
            description: self.description,
            target_date: self.target_date,
            sort_order: self.sort_order,
            created_at: self.created_at,
        }
    }
}

/// Internal row type for deserialising `project_updates` query results.
#[derive(sqlx::FromRow)]
struct ProjectUpdateRow {
    update_id: String,
    project_id: String,
    user_id: String,
    health: String,
    body: Option<String>,
    created_at: String,
}

impl ProjectUpdateRow {
    fn into_dto(self) -> ProjectUpdate {
        ProjectUpdate {
            update_id: self.update_id,
            project_id: self.project_id,
            user_id: self.user_id,
            health: self.health,
            body: self.body,
            created_at: self.created_at,
        }
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Base SELECT for project queries.
const PROJECT_SELECT: &str = "\
    SELECT project_id, workspace_id, name, description, icon, color, status, \
           lead_id, CAST(start_date AS TEXT) AS start_date, \
           CAST(target_date AS TEXT) AS target_date, \
           sort_order, \
           CAST(created_at AS TEXT) AS created_at, \
           CAST(updated_at AS TEXT) AS updated_at \
    FROM projects";

/// Base SELECT for milestone queries.
const MILESTONE_SELECT: &str = "\
    SELECT milestone_id, project_id, name, description, \
           CAST(target_date AS TEXT) AS target_date, \
           sort_order, \
           CAST(created_at AS TEXT) AS created_at \
    FROM project_milestones";

/// Base SELECT for project update queries.
const PROJECT_UPDATE_SELECT: &str = "\
    SELECT update_id, project_id, user_id, health, body, \
           CAST(created_at AS TEXT) AS created_at \
    FROM project_updates";

// ─── Project CRUD ───────────────────────────────────────────────────────────

/// List all projects in a workspace, ordered by sort_order then creation date.
pub async fn list_projects(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Project>> {
    let sql = format!(
        "{PROJECT_SELECT} WHERE workspace_id = $1 ORDER BY sort_order ASC, created_at ASC"
    );
    let rows: Vec<ProjectRow> = trakkt_core::db_fetch_all!(
        db,
        ProjectRow,
        &sql,
        workspace_id
    )?;
    Ok(rows.into_iter().map(ProjectRow::into_dto).collect())
}

/// Get a single project by ID.
pub async fn get_project(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<Option<Project>> {
    let sql = format!("{PROJECT_SELECT} WHERE project_id = $1");
    let row = trakkt_core::db_fetch_optional!(
        db,
        ProjectRow,
        &sql,
        project_id
    )?;
    Ok(row.map(ProjectRow::into_dto))
}

/// Create a new project in a workspace.
pub async fn create_project(
    db: &DbPool,
    workspace_id: &str,
    name: &str,
    description: Option<&str>,
    icon: Option<&str>,
    color: Option<&str>,
    lead_id: Option<&str>,
    start_date: Option<&str>,
    target_date: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Project> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let project_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO projects \
            (project_id, workspace_id, name, description, icon, color, \
             status, lead_id, start_date, target_date, sort_order, \
             created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 'planned', $7, $8, $9, 0, {now}, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &project_id,
        workspace_id,
        name,
        description,
        icon,
        color,
        lead_id,
        start_date,
        target_date
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT,
        &project_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, project_id = %project_id, "Failed to write sync log entry for project create");
    }

    // Re-fetch to get DB-assigned timestamps.
    let sql = format!("{PROJECT_SELECT} WHERE project_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        ProjectRow,
        &sql,
        &project_id
    )?;
    let project = row.into_dto();

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::PROJECT,
            &project_id,
            SyncActionType::Insert,
            serde_json::to_value(&project).ok(),
        )
        .await;
    }

    Ok(project)
}

/// Update a project.
///
/// Only fields that are `Some` are changed. `updated_at` is always set.
pub async fn update_project(
    db: &DbPool,
    project_id: &str,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    color: Option<&str>,
    status: Option<&str>,
    lead_id: Option<Option<&str>>,
    start_date: Option<Option<&str>>,
    target_date: Option<Option<&str>>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Project> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    // Dynamic SET clause.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx: usize = 1;

    if name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if description.is_some() {
        set_parts.push(format!("description = ${param_idx}"));
        param_idx += 1;
    }
    if icon.is_some() {
        set_parts.push(format!("icon = ${param_idx}"));
        param_idx += 1;
    }
    if color.is_some() {
        set_parts.push(format!("color = ${param_idx}"));
        param_idx += 1;
    }
    if status.is_some() {
        set_parts.push(format!("status = ${param_idx}"));
        param_idx += 1;
    }
    if lead_id.is_some() {
        set_parts.push(format!("lead_id = ${param_idx}"));
        param_idx += 1;
    }
    if start_date.is_some() {
        set_parts.push(format!("start_date = ${param_idx}"));
        param_idx += 1;
    }
    if target_date.is_some() {
        set_parts.push(format!("target_date = ${param_idx}"));
        param_idx += 1;
    }

    // Always update updated_at.
    set_parts.push(format!("updated_at = {now}"));

    let pid_idx = param_idx;
    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE projects SET {set_clause} WHERE project_id = ${pid_idx}"
    );

    let affected: u64 = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql);

        if let Some(v) = name {
            query = query.bind(v);
        }
        if let Some(v) = description {
            query = query.bind(v);
        }
        if let Some(v) = icon {
            query = query.bind(v);
        }
        if let Some(v) = color {
            query = query.bind(v);
        }
        if let Some(v) = status {
            query = query.bind(v);
        }
        if let Some(v) = lead_id {
            query = query.bind(v);
        }
        if let Some(v) = start_date {
            query = query.bind(v);
        }
        if let Some(v) = target_date {
            query = query.bind(v);
        }

        query = query.bind(project_id);

        query.execute(p).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "project {project_id} not found"
        )));
    }

    // Re-fetch the updated project.
    let sql = format!("{PROJECT_SELECT} WHERE project_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        ProjectRow,
        &sql,
        project_id
    )?;
    let project = row.into_dto();

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT,
        project_id,
        &project.workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, project_id = %project_id, "Failed to write sync log entry for project update");
    }

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &project.workspace_id,
            entity_types::PROJECT,
            project_id,
            SyncActionType::Update,
            serde_json::to_value(&project).ok(),
        )
        .await;
    }

    Ok(project)
}

/// Delete a project.
///
/// Cascading deletes remove associated members, milestones, and updates.
/// Issues linked to this project will have `project_id` set to NULL (ON DELETE SET NULL).
pub async fn delete_project(
    db: &DbPool,
    project_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch workspace_id before delete for the sync log.
    let sql = format!("{PROJECT_SELECT} WHERE project_id = $1");
    let row = trakkt_core::db_fetch_optional!(
        db,
        ProjectRow,
        &sql,
        project_id
    )?;

    let project = row.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!("project {project_id} not found"))
    })?;

    trakkt_core::db_execute!(
        db,
        "DELETE FROM projects WHERE project_id = $1",
        project_id
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT,
        project_id,
        &project.workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, project_id = %project_id, "Failed to write sync log entry for project delete");
    }

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &project.workspace_id,
            entity_types::PROJECT,
            project_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
}

// ─── Project Members ────────────────────────────────────────────────────────

/// List all members of a project.
pub async fn list_project_members(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<Vec<ProjectMember>> {
    let rows: Vec<ProjectMemberRow> = trakkt_core::db_fetch_all!(
        db,
        ProjectMemberRow,
        "SELECT project_id, user_id, role, \
                CAST(created_at AS TEXT) AS created_at \
         FROM project_members WHERE project_id = $1 \
         ORDER BY created_at ASC",
        project_id
    )?;
    Ok(rows.into_iter().map(ProjectMemberRow::into_dto).collect())
}

/// Add a member to a project.
pub async fn add_project_member(
    db: &DbPool,
    project_id: &str,
    user_id: &str,
    role: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    let sql = format!(
        "INSERT INTO project_members (project_id, user_id, role, created_at) \
         VALUES ($1, $2, $3, {now})"
    );
    trakkt_core::db_execute!(db, &sql, project_id, user_id, role)?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT,
        project_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, project_id = %project_id, "Failed to write sync log entry for member add");
    }

    // WebSocket broadcast — notify that the project changed.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_notify(ws, entity_types::PROJECT, workspace_id).await;
    }

    Ok(())
}

/// Remove a member from a project.
pub async fn remove_project_member(
    db: &DbPool,
    project_id: &str,
    user_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let result = trakkt_core::db_execute!(
        db,
        "DELETE FROM project_members WHERE project_id = $1 AND user_id = $2",
        project_id,
        user_id
    )?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "member {user_id} not found in project {project_id}"
        )));
    }

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT,
        project_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, project_id = %project_id, "Failed to write sync log entry for member remove");
    }

    // WebSocket broadcast — notify that the project changed.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_notify(ws, entity_types::PROJECT, workspace_id).await;
    }

    Ok(())
}

// ─── Milestones ─────────────────────────────────────────────────────────────

/// List all milestones in a project, ordered by sort_order then creation date.
pub async fn list_milestones(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<Vec<ProjectMilestone>> {
    let sql = format!(
        "{MILESTONE_SELECT} WHERE project_id = $1 ORDER BY sort_order ASC, created_at ASC"
    );
    let rows: Vec<MilestoneRow> = trakkt_core::db_fetch_all!(
        db,
        MilestoneRow,
        &sql,
        project_id
    )?;
    Ok(rows.into_iter().map(MilestoneRow::into_dto).collect())
}

/// Create a new milestone in a project.
pub async fn create_milestone(
    db: &DbPool,
    project_id: &str,
    name: &str,
    description: Option<&str>,
    target_date: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
    workspace_id: &str,
) -> trakkt_core::Result<ProjectMilestone> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let milestone_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO project_milestones \
            (milestone_id, project_id, name, description, target_date, sort_order, created_at) \
         VALUES ($1, $2, $3, $4, $5, 0, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &milestone_id,
        project_id,
        name,
        description,
        target_date
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT_MILESTONE,
        &milestone_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, milestone_id = %milestone_id, "Failed to write sync log entry for milestone create");
    }

    // Re-fetch to get DB-assigned timestamp.
    let sql = format!("{MILESTONE_SELECT} WHERE milestone_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        MilestoneRow,
        &sql,
        &milestone_id
    )?;
    let milestone = row.into_dto();

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::PROJECT_MILESTONE,
            &milestone_id,
            SyncActionType::Insert,
            serde_json::to_value(&milestone).ok(),
        )
        .await;
    }

    Ok(milestone)
}

/// Update a milestone.
pub async fn update_milestone(
    db: &DbPool,
    milestone_id: &str,
    name: Option<&str>,
    description: Option<&str>,
    target_date: Option<Option<&str>>,
    ws_manager: Option<&WebSocketManager>,
    workspace_id: &str,
) -> trakkt_core::Result<ProjectMilestone> {
    // Dynamic SET clause.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx: usize = 1;

    if name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if description.is_some() {
        set_parts.push(format!("description = ${param_idx}"));
        param_idx += 1;
    }
    if target_date.is_some() {
        set_parts.push(format!("target_date = ${param_idx}"));
        param_idx += 1;
    }

    if set_parts.is_empty() {
        // Nothing to update — just return the current milestone.
        let sql = format!("{MILESTONE_SELECT} WHERE milestone_id = $1");
        let row = trakkt_core::db_fetch_one!(
            db,
            MilestoneRow,
            &sql,
            milestone_id
        )?;
        return Ok(row.into_dto());
    }

    let mid_idx = param_idx;
    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE project_milestones SET {set_clause} WHERE milestone_id = ${mid_idx}"
    );

    let affected: u64 = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql);

        if let Some(v) = name {
            query = query.bind(v);
        }
        if let Some(v) = description {
            query = query.bind(v);
        }
        if let Some(v) = target_date {
            query = query.bind(v);
        }

        query = query.bind(milestone_id);

        query.execute(p).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "milestone {milestone_id} not found"
        )));
    }

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT_MILESTONE,
        milestone_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, milestone_id = %milestone_id, "Failed to write sync log entry for milestone update");
    }

    // Re-fetch the updated milestone.
    let sql = format!("{MILESTONE_SELECT} WHERE milestone_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        MilestoneRow,
        &sql,
        milestone_id
    )?;
    let milestone = row.into_dto();

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::PROJECT_MILESTONE,
            milestone_id,
            SyncActionType::Update,
            serde_json::to_value(&milestone).ok(),
        )
        .await;
    }

    Ok(milestone)
}

/// Delete a milestone.
///
/// Issues linked to this milestone will have `milestone_id` set to NULL (ON DELETE SET NULL).
pub async fn delete_milestone(
    db: &DbPool,
    milestone_id: &str,
    ws_manager: Option<&WebSocketManager>,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let result = trakkt_core::db_execute!(
        db,
        "DELETE FROM project_milestones WHERE milestone_id = $1",
        milestone_id
    )?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "milestone {milestone_id} not found"
        )));
    }

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT_MILESTONE,
        milestone_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, milestone_id = %milestone_id, "Failed to write sync log entry for milestone delete");
    }

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::PROJECT_MILESTONE,
            milestone_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
}

// ─── Project Updates ────────────────────────────────────────────────────────

/// List all updates for a project, newest first.
pub async fn list_project_updates(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<Vec<ProjectUpdate>> {
    let sql = format!(
        "{PROJECT_UPDATE_SELECT} WHERE project_id = $1 ORDER BY created_at DESC"
    );
    let rows: Vec<ProjectUpdateRow> = trakkt_core::db_fetch_all!(
        db,
        ProjectUpdateRow,
        &sql,
        project_id
    )?;
    Ok(rows.into_iter().map(ProjectUpdateRow::into_dto).collect())
}

/// Create a new status update on a project.
pub async fn create_project_update(
    db: &DbPool,
    project_id: &str,
    user_id: &str,
    health: &str,
    body: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
    workspace_id: &str,
) -> trakkt_core::Result<ProjectUpdate> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let update_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO project_updates \
            (update_id, project_id, user_id, health, body, created_at) \
         VALUES ($1, $2, $3, $4, $5, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &update_id,
        project_id,
        user_id,
        health,
        body
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::PROJECT,
        project_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, project_id = %project_id, "Failed to write sync log entry for project update");
    }

    // Re-fetch to get DB-assigned timestamp.
    let sql = format!("{PROJECT_UPDATE_SELECT} WHERE update_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        ProjectUpdateRow,
        &sql,
        &update_id
    )?;
    let update = row.into_dto();

    // WebSocket broadcast — notify that the project changed.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_notify(ws, entity_types::PROJECT, workspace_id).await;
    }

    Ok(update)
}

/// Compute project progress from issue status categories.
pub async fn get_project_progress(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<ProjectProgress> {
    #[derive(sqlx::FromRow)]
    struct ProgressRow {
        total: i64,
        completed: i64,
        cancelled: i64,
    }

    let row = trakkt_core::db_fetch_one!(
        db,
        ProgressRow,
        "SELECT \
             COUNT(*) AS total, \
             COUNT(CASE WHEN s.category = 'completed' THEN 1 END) AS completed, \
             COUNT(CASE WHEN s.category = 'cancelled' THEN 1 END) AS cancelled \
         FROM issues i \
         JOIN statuses s ON s.status_id = i.status_id \
         WHERE i.project_id = $1",
        project_id
    )?;

    let percent_done = if row.total > 0 {
        ((row.completed + row.cancelled) as f64 / row.total as f64) * 100.0
    } else {
        0.0
    };

    Ok(ProjectProgress {
        total: row.total,
        completed: row.completed,
        cancelled: row.cancelled,
        percent_done,
    })
}
