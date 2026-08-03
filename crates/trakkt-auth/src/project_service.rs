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
use crate::sync_log_service::CascadedIdRow;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising `projects` query results.
///
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
    lead_name: Option<String>,
    start_date: Option<String>,
    target_date: Option<String>,
    sort_order: f64,
    created_at: String,
    updated_at: String,
    archived_at: Option<String>,
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
            lead_name: self.lead_name,
            sort_order: self.sort_order,
            start_date: self.start_date,
            target_date: self.target_date,
            created_at: self.created_at,
            updated_at: self.updated_at,
            archived_at: self.archived_at,
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

/// One issue a project delete detaches, and the workspace its `sync_log` entry
/// belongs in.
///
/// [`CascadedIdRow`] cannot serve this read. These issues are the `ON DELETE SET
/// NULL` half of the cascade — they survive it — so what each needs is an ISSUE
/// *update* filed in its own workspace, and nothing constrains an issue's
/// `project_id` or `milestone_id` to the workspace its project is in. The query
/// lives in [`delete_project`] and nowhere else.
#[derive(sqlx::FromRow)]
struct DetachedIssueRow {
    issue_id: String,
    workspace_id: String,
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
    SELECT p.project_id, p.workspace_id, p.name, p.description, p.icon, p.color, p.status, \
           p.lead_id, lead.name AS lead_name, \
           CAST(p.start_date AS TEXT) AS start_date, \
           CAST(p.target_date AS TEXT) AS target_date, \
           p.sort_order, \
           CAST(p.created_at AS TEXT) AS created_at, \
           CAST(p.updated_at AS TEXT) AS updated_at, \
           CAST(p.archived_at AS TEXT) AS archived_at \
    FROM projects p \
    LEFT JOIN users lead ON lead.user_id = p.lead_id";

/// Base SELECT for project member queries.
const PROJECT_MEMBER_SELECT: &str = "\
    SELECT project_id, user_id, role, \
           CAST(created_at AS TEXT) AS created_at \
    FROM project_members";

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
        "{PROJECT_SELECT} WHERE p.workspace_id = $1 ORDER BY p.sort_order ASC, p.created_at ASC"
    );
    let rows: Vec<ProjectRow> = trakkt_core::db_fetch_all!(
        db,
        ProjectRow,
        &sql,
        workspace_id
    )?;
    Ok(rows.into_iter().map(ProjectRow::into_dto).collect())
}

/// Get a single project by ID, across every workspace.
///
/// This lookup is **unscoped**: it will return a project belonging to any
/// workspace. It answers "does this id exist", never "may this caller touch
/// it". Use it only where the id is already known to be in the caller's
/// workspace, or where the result's `workspace_id` is compared afterwards.
/// Anywhere the id came from the caller and the answer decides access, use
/// [`get_project_in_workspace`] instead.
pub async fn get_project(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<Option<Project>> {
    let sql = format!("{PROJECT_SELECT} WHERE p.project_id = $1");
    let row = trakkt_core::db_fetch_optional!(
        db,
        ProjectRow,
        &sql,
        project_id
    )?;
    Ok(row.map(ProjectRow::into_dto))
}

/// Get a project by ID, requiring it to belong to `workspace_id`.
///
/// Use this wherever the `project_id` came from the caller. [`get_project`]
/// looks up a project id across every workspace, so on its own it cannot tell a
/// project the caller owns from one they merely named — the caller has to
/// compare `workspace_id` afterwards, and forgetting to is silent.
///
/// A project in another workspace is reported as `NotFound`, not `Forbidden`:
/// the caller cannot see that workspace, and an error distinguishing "wrong
/// workspace" from "no such project" turns any project id into an existence
/// oracle.
pub async fn get_project_in_workspace(
    db: &DbPool,
    project_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Project> {
    let sql = format!("{PROJECT_SELECT} WHERE p.project_id = $1 AND p.workspace_id = $2");
    let row = trakkt_core::db_fetch_optional!(
        db,
        ProjectRow,
        &sql,
        project_id,
        workspace_id
    )?;
    row.map(ProjectRow::into_dto)
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("project {project_id} not found")))
}

/// The sync `entity_id` for a project membership.
///
/// `project_members` has a composite primary key and no surrogate id, so the
/// two columns are joined into one stable key. The add and the remove derive it
/// the same way, which is what lets the client's cache delete target exactly the
/// row the add upserted.
fn project_member_entity_id(project_id: &str, user_id: &str) -> String {
    format!("{project_id}:{user_id}")
}

/// Parameters for creating a new project.
pub struct CreateProjectParams<'a> {
    pub workspace_id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub color: Option<&'a str>,
    pub lead_id: Option<&'a str>,
    pub start_date: Option<&'a str>,
    pub target_date: Option<&'a str>,
}

/// Create a new project in a workspace.
///
/// The INSERT and its `sync_log` entry are one transaction: a project that
/// commits without its sync row is invisible to every future delta, so a failed
/// log write rolls the project back rather than leaving it stranded.
pub async fn create_project(
    db: &DbPool,
    params: &CreateProjectParams<'_>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Project> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let project_id = uuid::Uuid::new_v4().to_string();

    let sd = sql_compat::cast_to_date(is_pg, "$8");
    let td = sql_compat::cast_to_date(is_pg, "$9");
    let sql = format!(
        "INSERT INTO projects \
            (project_id, workspace_id, name, description, icon, color, \
             status, lead_id, start_date, target_date, sort_order, \
             created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 'planned', $7, {sd}, {td}, 0, {now}, {now})"
    );
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &project_id,
        params.workspace_id,
        params.name,
        params.description,
        params.icon,
        params.color,
        params.lead_id,
        params.start_date,
        params.target_date
    )?;

    // Re-fetch to get DB-assigned timestamps. This has to happen before the sync
    // log write: both the stored entry and the live frame carry the full
    // project, and the client cannot apply either without it. The row does not
    // exist outside the transaction yet, so the read runs on it.
    let sql = format!("{PROJECT_SELECT} WHERE p.project_id = $1");
    let row: ProjectRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        ProjectRow,
        &sql,
        &project_id
    )?;
    let project = row.into_dto();
    let payload =
        sync_log_service::sync_payload(&project, entity_types::PROJECT, &project.project_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT,
        &project_id,
        params.workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(project)
}

/// Parameters for updating a project.
///
/// Only fields that are `Some` are changed. `updated_at` is always set.
/// For clearable fields (`lead_id`, `start_date`, `target_date`), the outer
/// `Option` controls whether the field is updated; the inner `Option` allows
/// setting the column to `NULL`.
pub struct UpdateProjectParams<'a> {
    pub project_id: &'a str,
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub color: Option<&'a str>,
    pub status: Option<&'a str>,
    pub lead_id: Option<Option<&'a str>>,
    pub start_date: Option<Option<&'a str>>,
    pub target_date: Option<Option<&'a str>>,
    /// Archive/unarchive: `None` = no change, `Some(None)` = unarchive (clear),
    /// `Some(Some(timestamp))` = archive (set timestamp).
    pub archived_at: Option<Option<&'a str>>,
}

/// Update a project.
///
/// Only fields that are `Some` are changed. `updated_at` is always set.
///
/// The UPDATE and its `sync_log` entry are one transaction — see
/// [`create_project`].
pub async fn update_project(
    db: &DbPool,
    params: &UpdateProjectParams<'_>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Project> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    // Dynamic SET clause.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx: usize = 1;

    if params.name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if params.description.is_some() {
        set_parts.push(format!("description = ${param_idx}"));
        param_idx += 1;
    }
    if params.icon.is_some() {
        set_parts.push(format!("icon = ${param_idx}"));
        param_idx += 1;
    }
    if params.color.is_some() {
        set_parts.push(format!("color = ${param_idx}"));
        param_idx += 1;
    }
    if params.status.is_some() {
        set_parts.push(format!("status = ${param_idx}"));
        param_idx += 1;
    }
    if params.lead_id.is_some() {
        set_parts.push(format!("lead_id = ${param_idx}"));
        param_idx += 1;
    }
    if params.start_date.is_some() {
        let cast = sql_compat::cast_to_date(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("start_date = {cast}"));
        param_idx += 1;
    }
    if params.target_date.is_some() {
        let cast = sql_compat::cast_to_date(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("target_date = {cast}"));
        param_idx += 1;
    }
    if params.archived_at.is_some() {
        let cast = sql_compat::cast_to_timestamptz(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("archived_at = {cast}"));
        param_idx += 1;
    }

    // Always update updated_at.
    set_parts.push(format!("updated_at = {now}"));

    let pid_idx = param_idx;
    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE projects SET {set_clause} WHERE project_id = ${pid_idx}"
    );

    let mut tx = db.begin().await?;

    let affected: u64 = trakkt_core::tx_with!(&mut tx, |e| {
        let mut query = sqlx::query(&sql);

        if let Some(v) = params.name {
            query = query.bind(v);
        }
        if let Some(v) = params.description {
            query = query.bind(v);
        }
        if let Some(v) = params.icon {
            query = query.bind(v);
        }
        if let Some(v) = params.color {
            query = query.bind(v);
        }
        if let Some(v) = params.status {
            query = query.bind(v);
        }
        if let Some(v) = params.lead_id {
            query = query.bind(v);
        }
        if let Some(v) = params.start_date {
            query = query.bind(v);
        }
        if let Some(v) = params.target_date {
            query = query.bind(v);
        }
        if let Some(v) = params.archived_at {
            query = query.bind(v);
        }

        query = query.bind(params.project_id);

        query.execute(e).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "project {} not found", params.project_id
        )));
    }

    // Re-fetch the updated project. The new state is only visible on the
    // transaction, so the read runs on it.
    let sql = format!("{PROJECT_SELECT} WHERE p.project_id = $1");
    let row: ProjectRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        ProjectRow,
        &sql,
        params.project_id
    )?;
    let project = row.into_dto();
    let payload =
        sync_log_service::sync_payload(&project, entity_types::PROJECT, &project.project_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT,
        params.project_id,
        &project.workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Update,
        payload,
        ws_manager,
    )
    .await?;

    Ok(project)
}

/// Delete a project.
///
/// The single `DELETE FROM projects` destroys the project's members, milestones
/// and posted updates through the database's own `ON DELETE CASCADE` foreign
/// keys, and detaches its issues through `ON DELETE SET NULL`, so nothing after
/// it can report what went with it. That is why the cascaded ids are read on the
/// transaction *before* the DELETE: `sync_delta` replays entity-scoped actions,
/// so an entity that never receives a delete action is never evicted from a
/// client's IndexedDB cache — it survives every reconnect until the cache is
/// cleared by hand. That was TRA-9971, and TRA-9957 was the same defect in
/// [`crate::issue_service::delete_issue`].
///
/// The DELETE, the PROJECT entry, one entry per cascaded row and one ISSUE
/// update per detached issue are written on one transaction and delivered by
/// [`sync_log_service::SyncBatch`] strictly after it commits, so a client can
/// never be told about a removal that was rolled back, nor left without the row
/// that replays one that was not.
///
/// The issues are the asymmetric case. `ON DELETE SET NULL` leaves them in
/// place, so what they need is an ISSUE **update carrying their new state**, not
/// a delete: a client told to delete them would drop rows the server still has.
/// Their payloads are therefore read back *after* the DELETE, which is the only
/// point at which the cascade's effect on them is visible.
///
/// Only the cascaded types the client caches get an entry. The rest are
/// enumerated at the read site below, each with the reason it needs none.
pub async fn delete_project(
    db: &DbPool,
    project_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch workspace_id before delete for the sync log. This reads state that
    // predates the transaction, so it stays on the pool ahead of it — once the
    // transaction is open the pool is unreachable on SQLite (see `DbTx`).
    let sql = format!("{PROJECT_SELECT} WHERE p.project_id = $1");
    let row = trakkt_core::db_fetch_optional!(
        db,
        ProjectRow,
        &sql,
        project_id
    )?;

    let project = row.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!("project {project_id} not found"))
    })?;

    let mut tx = db.begin().await?;

    // ── Cascaded rows the client caches, read before the DELETE ─────────────
    //
    // Everything named below is gone the instant the DELETE runs, and the
    // foreign keys that remove it report nothing back, so these are the only
    // reads that can ever see it. They run on the transaction, not the pool: an
    // open transaction holds SQLite's only connection (see `DbTx`).
    //
    // All three cascaded types are cached. `cache_rows_written_by` in
    // `crates/trakkt-ui/src/cache/cached_types.rs` is the client's single
    // membership rule — the write path is gated on it, the per-entity delete
    // iterates it, and the reset wipe is its union — and none of
    // `project_member`, `project_milestone` or `project_update` is on its
    // `NOT_CACHED` list, so each owns exactly its own cache row. The rows really
    // are written, too: `add_project_member`, `create_milestone` and
    // `create_project_update` all send a payload, and a payload-carrying frame
    // is what `enqueue_cache_writes` persists. So an entry here evicts an
    // IndexedDB row rather than only bumping a counter.

    // `project_members` has a composite primary key and no surrogate id, so the
    // sync `entity_id` is the one `project_member_entity_id` builds — the same
    // key `add_project_member` upserted the cached row under, which is what lets
    // the client's delete target exactly that row. Only `user_id` is read back;
    // `project_id` is this function's argument.
    let member_user_ids: Vec<CascadedIdRow> = trakkt_core::tx_fetch_all!(
        &mut tx,
        CascadedIdRow,
        "SELECT user_id AS id FROM project_members WHERE project_id = $1",
        project_id
    )?;

    let milestone_ids: Vec<CascadedIdRow> = trakkt_core::tx_fetch_all!(
        &mut tx,
        CascadedIdRow,
        "SELECT milestone_id AS id FROM project_milestones WHERE project_id = $1",
        project_id
    )?;

    let update_ids: Vec<CascadedIdRow> = trakkt_core::tx_fetch_all!(
        &mut tx,
        CascadedIdRow,
        "SELECT update_id AS id FROM project_updates WHERE project_id = $1",
        project_id
    )?;

    // The issues the cascade detaches. Read here because after the DELETE
    // neither predicate can find them again — `project_id` is NULL and the
    // milestones the second one names no longer exist — and read back below,
    // once the cascade has actually run, for the state their payloads carry.
    //
    // Two predicates, because a project takes two kinds of issue with it:
    //
    // * `project_id = $1` — the project's own issues, cleared by
    //   `issues_project_id_fkey` / the SQLite column's `ON DELETE SET NULL`.
    // * `milestone_id IN (its milestones)` — the second-order cascade. Deleting
    //   the project deletes `project_milestones`, and on Postgres
    //   `issues_milestone_id_fkey` (ON DELETE SET NULL) clears every
    //   `issues.milestone_id` that pointed at one. This set is not a subset of
    //   the first: neither `create_issue` nor `update_issue` requires an issue's
    //   milestone to belong to the issue's own project, so an issue outside the
    //   project can hold one of its milestones.
    //
    //   On SQLite those pointers are *not* cleared — `issues.milestone_id` has
    //   no foreign key there at all, which
    //   `migrations-sqlite/20260803100000_dual_backend_fk_parity.sql` records in
    //   its closing note as the twelfth of twelve and deliberately leaves
    //   outstanding. Both backends still get an entry per issue, because the
    //   payload below is read back from the row as it actually stands: on
    //   Postgres it reports the cleared pointer, on SQLite it re-reports the
    //   unchanged issue. Neither claims a change that did not happen, and the
    //   number of entries does not vary by dialect.
    //
    // `workspace_id` comes off the issue rather than off the project. Nothing
    // constrains an issue's `project_id` or `milestone_id` to its own workspace
    // — the two writers above bind whichever id they are given, and the schema
    // has no such check — and a `sync_log` row filed under the wrong workspace
    // reaches no client that holds the issue.
    let detached_issues: Vec<DetachedIssueRow> = trakkt_core::tx_fetch_all!(
        &mut tx,
        DetachedIssueRow,
        "SELECT issue_id, workspace_id FROM issues \
         WHERE project_id = $1 \
            OR milestone_id IN \
               (SELECT milestone_id FROM project_milestones WHERE project_id = $1)",
        project_id
    )?;

    // Deliberately *not* given an entry:
    //
    // * `favorites` — a favourited project is stored as `(target_type,
    //   target_id)` with no foreign key to `projects` in either dialect, so this
    //   DELETE cascades nothing there and the row survives. `favorite` is cached
    //   (it is not on `NOT_CACHED`, and `sync_bootstrap` streams it), and the
    //   surviving row is now stale — but it is an orphan the schema leaves
    //   behind, not a removal this function performed, and a FAVORITE delete
    //   entry here would report a deletion that did not happen and would silently
    //   diverge the cache from the table. That is a defect in its own right and
    //   is reported separately rather than papered over here.
    // * `notifications` — `context_id` is plain TEXT with no foreign key
    //   (`migrations-sqlite/20260610400000_notification_context_id.sql`), so a
    //   project delete cascades nothing there either. Notifications are removed
    //   by their issue's delete, which is `delete_issue`'s business, not this
    //   one's; no issue is deleted here.
    // * `issues.milestone_id` — covered by the read above rather than treated as
    //   a separate entity type, because the entry it needs is an ISSUE update
    //   and the issue may already be in that list through `project_id`.
    // * Nothing else cascades, because nothing else points here. Grepping
    //   `REFERENCES … projects(` across both migration directories finds four
    //   referencing columns and no fifth — `project_members.project_id`,
    //   `project_milestones.project_id`, `project_updates.project_id` and
    //   `issues.project_id` — and `REFERENCES … project_milestones(` finds only
    //   `issues.milestone_id`, which is the second-order case handled above.
    //   `project_members` and `project_updates` are named by no foreign key at
    //   all, so neither cascades any further. (Grep counts lines, and the SQLite
    //   side redeclares columns each time a migration rebuilds `issues`, so the
    //   line count there exceeds the key count; the set of referencing columns
    //   is the same four either way.)

    trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM projects WHERE project_id = $1",
        project_id
    )?;

    let mut batch = sync_log_service::SyncBatch::new();

    // The sync entries follow the DELETE they describe — the same order as
    // `issue_service::delete_issue` and `team_service::delete_team`.
    batch
        .record(
            &mut tx,
            entity_types::PROJECT,
            project_id,
            &project.workspace_id,
            sync_log_service::SyncAudience::Workspace,
            SyncActionType::Delete,
            None,
        )
        .await?;

    // Every cascaded entry is `SyncAudience::Workspace`, matching the writer of
    // each type: `add_project_member`, `create_milestone` and
    // `create_project_update` all publish workspace-wide, so a delete scoped to
    // one user would leave the row cached for everyone else.
    //
    // A delete carries no payload: there is no row left to send, and the client
    // reads the entity id alone to drop the cached row and bump its counter.
    for member in &member_user_ids {
        let entity_id = project_member_entity_id(project_id, &member.id);
        batch
            .record(
                &mut tx,
                entity_types::PROJECT_MEMBER,
                &entity_id,
                &project.workspace_id,
                sync_log_service::SyncAudience::Workspace,
                SyncActionType::Delete,
                None,
            )
            .await?;
    }

    for milestone in &milestone_ids {
        batch
            .record(
                &mut tx,
                entity_types::PROJECT_MILESTONE,
                &milestone.id,
                &project.workspace_id,
                sync_log_service::SyncAudience::Workspace,
                SyncActionType::Delete,
                None,
            )
            .await?;
    }

    for update in &update_ids {
        batch
            .record(
                &mut tx,
                entity_types::PROJECT_UPDATE,
                &update.id,
                &project.workspace_id,
                sync_log_service::SyncAudience::Workspace,
                SyncActionType::Delete,
                None,
            )
            .await?;
    }

    // `Update` with a payload, not `Delete`: these issues outlive the project.
    // The payload is read here rather than beside the id capture above because
    // only now does it carry what the cascade left — before the DELETE it would
    // still name the project, and a client applying it would put the issue back
    // under a project it has just been told to remove.
    //
    // A vanished issue reaches this as `NotFound` — the one thing
    // `issue_sync_payload_tx` promises that variant means — and it is allowed to
    // abort the whole delete rather than be skipped. The only way to reach it is
    // a concurrent `delete_issue` landing in the gap between the id read above
    // and the DELETE that takes these rows' locks, which SQLite cannot express
    // at all, having one connection. Failing rolls the cascade back untouched
    // and the caller's retry succeeds, because the id is no longer there to
    // capture; skipping instead would commit a cascade leaving one issue stale
    // in every client's cache, with no later delta able to repair it.
    for issue in &detached_issues {
        let payload =
            crate::issue_service::issue_sync_payload_tx(&mut tx, &issue.issue_id).await?;
        batch
            .record(
                &mut tx,
                entity_types::ISSUE,
                &issue.issue_id,
                &issue.workspace_id,
                sync_log_service::SyncAudience::Workspace,
                SyncActionType::Update,
                payload,
            )
            .await?;
    }

    batch.commit_and_deliver(tx, ws_manager).await
}

// ─── Project Members ────────────────────────────────────────────────────────

/// List all members of a project.
pub async fn list_project_members(
    db: &DbPool,
    project_id: &str,
) -> trakkt_core::Result<Vec<ProjectMember>> {
    let sql =
        format!("{PROJECT_MEMBER_SELECT} WHERE project_id = $1 ORDER BY created_at ASC");
    let rows: Vec<ProjectMemberRow> = trakkt_core::db_fetch_all!(
        db,
        ProjectMemberRow,
        &sql,
        project_id
    )?;
    Ok(rows.into_iter().map(ProjectMemberRow::into_dto).collect())
}

/// Add a member to a project.
///
/// The INSERT and its `sync_log` entry are one transaction. `project_members` is
/// not an entity type a delta re-reads, so a membership that commits without its
/// sync row can never be repaired — it simply never reaches another client.
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

    // Resolved before the insert for two reasons: a missing project is a clean
    // NotFound rather than a foreign-key error out of the write, and the resolve
    // is workspace-scoped, so a project id from another workspace never reaches
    // the INSERT below. Every caller also checks ownership one layer up; this is
    // the second layer, held where the mutation is rather than in caller
    // discipline.
    // This is authorization, and it runs on the pool, so it stays ahead of the
    // transaction on both counts (see `DbTx`).
    get_project_in_workspace(db, project_id, workspace_id).await?;

    let sql = format!(
        "INSERT INTO project_members (project_id, user_id, role, created_at) \
         VALUES ($1, $2, $3, {now})"
    );

    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(&mut tx, &sql, project_id, user_id, role)?;

    // Re-read the row just written, for its DB-assigned timestamp. This has to
    // happen before the sync log write: both the stored entry and the live frame
    // carry the full membership, and the client skips either without it. The row
    // does not exist outside the transaction yet, so the read runs on it.
    let sql = format!("{PROJECT_MEMBER_SELECT} WHERE project_id = $1 AND user_id = $2");
    let row: ProjectMemberRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        ProjectMemberRow,
        &sql,
        project_id,
        user_id
    )?;
    let member = row.into_dto();
    let entity_id = project_member_entity_id(project_id, user_id);
    let payload = sync_log_service::sync_payload(&member, entity_types::PROJECT_MEMBER, &entity_id);

    // The membership is its own entity type rather than an update to the parent
    // project: the INSERT above is the only write this function makes, so the
    // `projects` row is byte-identical afterwards. Reporting it as a PROJECT
    // update would make every connected client re-upsert an unchanged project
    // and would still leave the membership itself invisible.
    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT_MEMBER,
        &entity_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await
}

/// Remove a member from a project.
///
/// The DELETE and its `sync_log` entry are one transaction — see
/// [`add_project_member`] for why a membership write cannot be repaired later.
pub async fn remove_project_member(
    db: &DbPool,
    project_id: &str,
    user_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Resolved before the DELETE so a project id from another workspace cannot
    // reach it. Without this the DELETE is the only filter, and it matches on
    // `project_id` alone — a foreign project's membership row would be a legal
    // target. Every caller also checks ownership one layer up; this is the
    // second layer, held where the mutation is rather than in caller discipline.
    // It is authorization, and it runs on the pool, so it stays ahead of the
    // transaction on both counts (see `DbTx`).
    get_project_in_workspace(db, project_id, workspace_id).await?;

    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM project_members WHERE project_id = $1 AND user_id = $2",
        project_id,
        user_id
    )?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "member {user_id} not found in project {project_id}"
        )));
    }

    let entity_id = project_member_entity_id(project_id, user_id);

    // A delete carries no payload: there is no row left to send, and the client
    // reads the entity id alone to drop the cached row and bump its counter.
    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT_MEMBER,
        &entity_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Delete,
        None,
        ws_manager,
    )
    .await
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

/// List all milestones across all projects in a workspace.
pub async fn list_milestones_for_workspace(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<ProjectMilestone>> {
    let sql = "\
        SELECT pm.milestone_id, pm.project_id, pm.name, pm.description, \
               CAST(pm.target_date AS TEXT) AS target_date, \
               pm.sort_order, \
               CAST(pm.created_at AS TEXT) AS created_at \
        FROM project_milestones pm \
        JOIN projects p ON pm.project_id = p.project_id \
        WHERE p.workspace_id = $1 \
        ORDER BY pm.sort_order ASC, pm.created_at ASC";
    let rows: Vec<MilestoneRow> = trakkt_core::db_fetch_all!(
        db,
        MilestoneRow,
        sql,
        workspace_id
    )?;
    Ok(rows.into_iter().map(MilestoneRow::into_dto).collect())
}

/// Create a new milestone in a project.
///
/// The INSERT and its `sync_log` entry are one transaction — see
/// [`create_project`].
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

    let td = sql_compat::cast_to_date(is_pg, "$5");
    let sql = format!(
        "INSERT INTO project_milestones \
            (milestone_id, project_id, name, description, target_date, sort_order, created_at) \
         VALUES ($1, $2, $3, $4, {td}, 0, {now})"
    );
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &milestone_id,
        project_id,
        name,
        description,
        target_date
    )?;

    // Re-fetch to get the DB-assigned timestamp. This has to happen before the
    // sync log write: both the stored entry and the live frame carry the full
    // milestone, and the client skips either without it. The row does not exist
    // outside the transaction yet, so the read runs on it.
    let sql = format!("{MILESTONE_SELECT} WHERE milestone_id = $1");
    let row: MilestoneRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        MilestoneRow,
        &sql,
        &milestone_id
    )?;
    let milestone = row.into_dto();
    let payload = sync_log_service::sync_payload(
        &milestone,
        entity_types::PROJECT_MILESTONE,
        &milestone.milestone_id,
    );

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT_MILESTONE,
        &milestone_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(milestone)
}

/// Update a milestone.
///
/// The UPDATE and its `sync_log` entry are one transaction — see
/// [`create_project`]. A call that changes nothing writes neither, and opens no
/// transaction.
pub async fn update_milestone(
    db: &DbPool,
    milestone_id: &str,
    name: Option<&str>,
    description: Option<&str>,
    target_date: Option<Option<&str>>,
    ws_manager: Option<&WebSocketManager>,
    workspace_id: &str,
) -> trakkt_core::Result<ProjectMilestone> {
    let is_pg = db.is_postgres();

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
        let cast = sql_compat::cast_to_date(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("target_date = {cast}"));
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

    let mut tx = db.begin().await?;

    let affected: u64 = trakkt_core::tx_with!(&mut tx, |e| {
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

        query.execute(e).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "milestone {milestone_id} not found"
        )));
    }

    // Re-fetch the updated milestone. This has to happen before the sync log
    // write: both the stored entry and the live frame carry the full milestone,
    // and the client skips either without it. The new state is only visible on
    // the transaction, so the read runs on it.
    let sql = format!("{MILESTONE_SELECT} WHERE milestone_id = $1");
    let row: MilestoneRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        MilestoneRow,
        &sql,
        milestone_id
    )?;
    let milestone = row.into_dto();
    let payload = sync_log_service::sync_payload(
        &milestone,
        entity_types::PROJECT_MILESTONE,
        &milestone.milestone_id,
    );

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT_MILESTONE,
        milestone_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Update,
        payload,
        ws_manager,
    )
    .await?;

    Ok(milestone)
}

/// Delete a milestone.
///
/// The DELETE and the `sync_log` entry that reports it are one transaction — see
/// [`delete_project`]. No row is destroyed alongside it: `project_milestones` is
/// named by exactly one foreign key, `issues.milestone_id`, and the issues
/// outlive the milestone.
///
/// # Two things this does not do, both tracked separately
///
/// Neither is introduced here; both were true before [`delete_project`] was
/// taught to report its cascade, and neither is that change's to fix.
///
/// * **The issues get no ISSUE update.** On Postgres
///   `issues_milestone_id_fkey` clears their `milestone_id`, and nothing tells
///   another client, so its cached issue rows keep naming a milestone that no
///   longer exists — the same defect TRA-9971 fixed for a project's issues.
///   [`delete_project`] does emit those updates for the milestones *it*
///   cascades away, so the two paths currently disagree.
/// * **On SQLite the pointer is not cleared at all.** `issues.milestone_id` has
///   no foreign key there; the closing note of
///   `migrations-sqlite/20260803100000_dual_backend_fk_parity.sql` records it as
///   the twelfth of twelve keys and explains why rebuilding eleven tables to add
///   it was not taken. So this DELETE leaves a dangling `milestone_id` on
///   SQLite and a NULL one on Postgres.
pub async fn delete_milestone(
    db: &DbPool,
    milestone_id: &str,
    ws_manager: Option<&WebSocketManager>,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM project_milestones WHERE milestone_id = $1",
        milestone_id
    )?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "milestone {milestone_id} not found"
        )));
    }

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT_MILESTONE,
        milestone_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Delete,
        None,
        ws_manager,
    )
    .await
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
///
/// The INSERT and its `sync_log` entry are one transaction. `project_updates` is
/// not an entity type a delta re-reads, so a posted update that commits without
/// its sync row never reaches another client and cannot be repaired later.
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
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &update_id,
        project_id,
        user_id,
        health,
        body
    )?;

    // Re-fetch to get the DB-assigned timestamp. This has to happen before the
    // sync log write: both the stored entry and the live frame carry the full
    // update, and the client skips either without it. The row does not exist
    // outside the transaction yet, so the read runs on it.
    let sql = format!("{PROJECT_UPDATE_SELECT} WHERE update_id = $1");
    let row: ProjectUpdateRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        ProjectUpdateRow,
        &sql,
        &update_id
    )?;
    let update = row.into_dto();
    let payload = sync_log_service::sync_payload(
        &update,
        entity_types::PROJECT_UPDATE,
        &update.update_id,
    );

    // The posted update is its own entity type rather than an update to the
    // parent project: the INSERT above is the only write this function makes, so
    // the `projects` row is byte-identical afterwards. Reporting it as a PROJECT
    // update would make every connected client re-upsert an unchanged project
    // and would still leave the posted update itself invisible.
    sync_log_service::commit_and_deliver(
        tx,
        entity_types::PROJECT_UPDATE,
        &update_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

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

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trakkt_core::db_execute;

    const WS_A: &str = "ws_alpha";
    const WS_B: &str = "ws_beta";
    const USER_A: &str = "usr_alpha";
    const USER_B: &str = "usr_beta";

    /// Two separate workspaces, one project each, seeded through the real
    /// service so the rows are exactly what production writes.
    ///
    /// `USER_A` belongs to workspace A only, and is the caller in the
    /// cross-workspace cases below: the workspace-B project is one they can name
    /// but must not be able to touch. `USER_B` is seeded as a member of it, so
    /// the remove case has a real membership row to try to disturb.
    ///
    /// Returns `(project_a_id, project_b_id)`.
    async fn two_workspaces() -> (DbPool, WebSocketManager, String, String) {
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

        // A real manager, so the broadcast arm of every mutation runs for real
        // rather than being skipped by a `None`.
        let ws_manager = WebSocketManager::new(None, db.clone());

        let mut project_ids = Vec::new();
        for (ws, name) in [(WS_A, "Alpha"), (WS_B, "Beta")] {
            let project = create_project(
                &db,
                &CreateProjectParams {
                    workspace_id: ws,
                    name,
                    description: None,
                    icon: None,
                    color: None,
                    lead_id: None,
                    start_date: None,
                    target_date: None,
                },
                Some(&ws_manager),
            )
            .await
            .expect("create project");
            project_ids.push(project.project_id);
        }
        let project_b = project_ids.pop().expect("project B");
        let project_a = project_ids.pop().expect("project A");

        add_project_member(&db, &project_b, USER_B, "member", WS_B, Some(&ws_manager))
            .await
            .expect("seed the existing membership in workspace B");

        (db, ws_manager, project_a, project_b)
    }

    /// The full `project_members` table, ordered, as `(project_id, user_id,
    /// role)`. Read straight from the table: the point is not to take the
    /// mutation's return value for it.
    async fn all_members(db: &DbPool) -> Vec<(String, String, Option<String>)> {
        #[derive(sqlx::FromRow)]
        struct MemberRow {
            project_id: String,
            user_id: String,
            role: Option<String>,
        }
        let rows: Vec<MemberRow> = trakkt_core::db_fetch_all!(
            db,
            MemberRow,
            "SELECT project_id, user_id, role FROM project_members \
             ORDER BY project_id ASC, user_id ASC"
        )
        .expect("read project_members");
        rows.into_iter()
            .map(|r| (r.project_id, r.user_id, r.role))
            .collect()
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

    async fn member_sync_rows_for_workspace(db: &DbPool, workspace_id: &str) -> i64 {
        trakkt_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM sync_log WHERE workspace_id = $1 AND entity_type = $2",
            workspace_id,
            entity_types::PROJECT_MEMBER
        )
        .expect("count project member sync log rows")
    }

    #[tokio::test]
    async fn add_project_member_refuses_a_project_in_another_workspace() {
        let (db, ws_manager, _project_a, project_b) = two_workspaces().await;
        let members_before = all_members(&db).await;
        let sync_before = sync_rows_for_workspace(&db, WS_B).await;
        let member_sync_before = member_sync_rows_for_workspace(&db, WS_B).await;

        // The project genuinely exists — an unscoped `is_none()` check passes
        // straight through it. Only a workspace-scoped resolve can refuse this.
        assert!(
            get_project(&db, &project_b)
                .await
                .expect("unscoped read")
                .is_some(),
            "the foreign project really does exist, so the refusal below has to \
             come from the workspace scoping and not from a missing row"
        );

        let result =
            add_project_member(&db, &project_b, USER_A, "admin", WS_A, Some(&ws_manager)).await;

        assert!(
            matches!(result, Err(trakkt_core::Error::NotFound(_))),
            "a project id from another workspace must be indistinguishable from a \
             project id that does not exist, got {result:?}"
        );
        assert_eq!(
            all_members(&db).await,
            members_before,
            "project_members must be unchanged — the caller must not have \
             inserted themselves into the foreign project"
        );
        assert_eq!(
            sync_rows_for_workspace(&db, WS_B).await,
            sync_before,
            "no sync_log row may be written into a workspace the caller cannot see"
        );
        assert_eq!(
            member_sync_rows_for_workspace(&db, WS_B).await,
            member_sync_before,
            "a refused mutation must not emit a PROJECT_MEMBER sync frame"
        );
    }

    #[tokio::test]
    async fn remove_project_member_refuses_a_project_in_another_workspace() {
        let (db, ws_manager, _project_a, project_b) = two_workspaces().await;
        let members_before = all_members(&db).await;
        let sync_before = sync_rows_for_workspace(&db, WS_B).await;
        let member_sync_before = member_sync_rows_for_workspace(&db, WS_B).await;

        assert!(
            get_project(&db, &project_b)
                .await
                .expect("unscoped read")
                .is_some(),
            "the foreign project really does exist, so the refusal below has to \
             come from the workspace scoping and not from a missing row"
        );

        let result =
            remove_project_member(&db, &project_b, USER_B, WS_A, Some(&ws_manager)).await;

        assert!(
            matches!(result, Err(trakkt_core::Error::NotFound(_))),
            "a project id from another workspace must be indistinguishable from a \
             project id that does not exist, got {result:?}"
        );
        assert_eq!(
            all_members(&db).await,
            members_before,
            "the seeded membership in the other workspace must survive intact"
        );
        assert_eq!(
            sync_rows_for_workspace(&db, WS_B).await,
            sync_before,
            "no sync_log row may be written into a workspace the caller cannot see"
        );
        assert_eq!(
            member_sync_rows_for_workspace(&db, WS_B).await,
            member_sync_before,
            "a refused mutation must not emit a PROJECT_MEMBER sync frame"
        );
    }

    #[tokio::test]
    async fn membership_mutations_still_work_within_the_workspace() {
        let (db, ws_manager, project_a, _project_b) = two_workspaces().await;
        let sync_before = member_sync_rows_for_workspace(&db, WS_A).await;

        add_project_member(&db, &project_a, USER_A, "admin", WS_A, Some(&ws_manager))
            .await
            .expect("adding a member to a project in the caller's own workspace");
        assert!(
            all_members(&db)
                .await
                .contains(&(project_a.clone(), USER_A.to_string(), Some("admin".to_string()))),
            "the membership must be written with the role it was given"
        );

        remove_project_member(&db, &project_a, USER_A, WS_A, Some(&ws_manager))
            .await
            .expect("removing a member of a project in the caller's own workspace");
        assert!(
            !all_members(&db)
                .await
                .iter()
                .any(|(p, u, _)| p == &project_a && u == USER_A),
            "the membership must be gone"
        );

        assert_eq!(
            member_sync_rows_for_workspace(&db, WS_A).await - sync_before,
            2,
            "the add and the remove each report themselves as a PROJECT_MEMBER action"
        );
    }

    #[tokio::test]
    async fn get_project_in_workspace_hides_a_project_from_another_workspace() {
        let (db, _ws_manager, project_a, project_b) = two_workspaces().await;

        assert!(
            get_project(&db, &project_b)
                .await
                .expect("unscoped read")
                .is_some(),
            "the project really does exist — the scoped read below has to be what hides it"
        );
        assert!(
            matches!(
                get_project_in_workspace(&db, &project_b, WS_A).await,
                Err(trakkt_core::Error::NotFound(_))
            ),
            "a project in another workspace must read as missing"
        );

        let project = get_project_in_workspace(&db, &project_a, WS_A)
            .await
            .expect("a project in the caller's own workspace resolves");
        assert_eq!(project.project_id, project_a);
        assert_eq!(project.workspace_id, WS_A);
    }
}
