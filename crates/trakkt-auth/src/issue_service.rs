// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue service — CRUD operations for the `issues` table.
//!
//! Issues are the core entity in Trakkt. Each issue belongs to a team within
//! a workspace and has a workspace-scoped auto-incrementing number. The service
//! supports dynamic filtering, label assignment, and full CRUD with sync log
//! integration.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::{CreateIssueParams, Issue, IssueFilters, IssueUpdate, IssueWithDetails, Label};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising basic issue queries.
#[derive(sqlx::FromRow)]
struct IssueRow {
    issue_id: String,
    workspace_id: String,
    team_id: String,
    number: i32,
    title: String,
    description: Option<String>,
    status: String,
    priority: i32,
    assignee_id: Option<String>,
    creator_id: String,
    due_date: Option<String>,
    created_at: String,
    updated_at: String,
}

impl IssueRow {
    fn into_dto(self) -> Issue {
        Issue {
            issue_id: self.issue_id,
            workspace_id: self.workspace_id,
            team_id: self.team_id,
            number: self.number,
            title: self.title,
            description: self.description,
            status: self.status,
            priority: self.priority,
            assignee_id: self.assignee_id,
            creator_id: self.creator_id,
            due_date: self.due_date,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Internal row type for issue queries joined with team and user tables.
#[derive(sqlx::FromRow)]
struct IssueDetailRow {
    issue_id: String,
    workspace_id: String,
    team_id: String,
    team_key: String,
    number: i32,
    title: String,
    description: Option<String>,
    status: String,
    priority: i32,
    assignee_id: Option<String>,
    assignee_name: Option<String>,
    creator_id: String,
    creator_name: Option<String>,
    due_date: Option<String>,
    created_at: String,
    updated_at: String,
}

impl IssueDetailRow {
    fn into_dto(self, labels: Vec<Label>) -> IssueWithDetails {
        IssueWithDetails {
            issue_id: self.issue_id,
            workspace_id: self.workspace_id,
            team_id: self.team_id,
            team_key: self.team_key,
            number: self.number,
            title: self.title,
            description: self.description,
            status: self.status,
            priority: self.priority,
            assignee_id: self.assignee_id,
            assignee_name: self.assignee_name,
            creator_id: self.creator_id,
            creator_name: self.creator_name,
            due_date: self.due_date,
            created_at: self.created_at,
            updated_at: self.updated_at,
            labels,
        }
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Base SELECT for issue detail queries with JOINs to teams and users.
const ISSUE_DETAIL_SELECT: &str = "\
    SELECT i.issue_id, i.workspace_id, i.team_id, t.key AS team_key, \
           i.number, i.title, i.description, i.status, i.priority, \
           i.assignee_id, assignee.name AS assignee_name, \
           i.creator_id, creator.name AS creator_name, \
           CAST(i.due_date AS TEXT) AS due_date, \
           CAST(i.created_at AS TEXT) AS created_at, \
           CAST(i.updated_at AS TEXT) AS updated_at \
    FROM issues i \
    JOIN teams t ON t.team_id = i.team_id \
    LEFT JOIN users assignee ON assignee.user_id = i.assignee_id \
    JOIN users creator ON creator.user_id = i.creator_id";

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Fetch labels for a list of issue IDs in a single query (avoids N+1).
///
/// Returns a map from issue_id to its labels.
async fn fetch_labels_for_issues(
    db: &DbPool,
    issue_ids: &[String],
) -> trakkt_core::Result<std::collections::HashMap<String, Vec<Label>>> {
    if issue_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build IN clause with numbered placeholders.
    let (in_clause, _) = trakkt_core::db::in_clause_placeholders(issue_ids.len(), 1);

    /// Row type for the label join query. Includes the issue_id for grouping.
    #[derive(sqlx::FromRow)]
    struct IssueLabelRow {
        issue_id: String,
        label_id: String,
        workspace_id: String,
        name: String,
        color: String,
        created_at: String,
    }

    let sql = format!(
        "SELECT il.issue_id, l.label_id, l.workspace_id, l.name, l.color, \
                CAST(l.created_at AS TEXT) AS created_at \
         FROM labels l \
         JOIN issue_labels il ON l.label_id = il.label_id \
         WHERE il.issue_id IN {in_clause} \
         ORDER BY l.name ASC"
    );

    // We need to bind each issue_id individually. Since the macro expands binds
    // at compile time, we use db_with_pool! and build the query manually.
    let rows: Vec<IssueLabelRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, IssueLabelRow>(&sql);
        for id in issue_ids {
            query = query.bind(id);
        }
        query.fetch_all(p).await
    })?;

    let mut map: std::collections::HashMap<String, Vec<Label>> =
        std::collections::HashMap::new();
    for row in rows {
        let issue_id = row.issue_id.clone();
        map.entry(issue_id).or_default().push(Label {
            label_id: row.label_id,
            workspace_id: row.workspace_id,
            name: row.name,
            color: row.color,
            created_at: row.created_at,
        });
    }
    Ok(map)
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new issue in a team.
///
/// The issue `number` is auto-incremented per workspace. Labels are attached
/// via the `issue_labels` junction table.
pub async fn create_issue(
    db: &DbPool,
    params: &CreateIssueParams,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Issue> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let issue_id = uuid::Uuid::new_v4().to_string();

    // Atomic number generation: the subquery computes the next number inside the
    // INSERT statement so the MAX read and INSERT happen atomically, preventing
    // race conditions where concurrent creates read the same MAX.
    let sql = format!(
        "INSERT INTO issues \
            (issue_id, workspace_id, team_id, number, title, description, \
             status, priority, assignee_id, creator_id, due_date, created_at, updated_at) \
         VALUES ($1, $2, $3, \
                 (SELECT COALESCE(MAX(number), 0) + 1 FROM issues WHERE workspace_id = $2), \
                 $4, $5, 'backlog', $6, $7, $8, $9, {now}, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &issue_id,
        &params.workspace_id,
        &params.team_id,
        &params.title,
        params.description.as_deref(),
        params.priority,
        params.assignee_id.as_deref(),
        &params.creator_id,
        params.due_date.as_deref()
    )?;

    // Attach labels.
    for label_id in &params.label_ids {
        trakkt_core::db_execute!(
            db,
            "INSERT INTO issue_labels (issue_id, label_id) VALUES ($1, $2)",
            &issue_id,
            label_id
        )?;
    }

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE,
        &issue_id,
        &params.workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to write sync log entry for issue create");
    }

    // WebSocket broadcast — fetch full entity data and send as SyncResponse.
    if let Some(ws) = ws_manager {
        if let Ok(Some(full_issue)) = get_issue_by_id(db, &issue_id).await {
            sync_log_service::broadcast_sync_action(
                ws,
                &params.workspace_id,
                entity_types::ISSUE,
                &issue_id,
                SyncActionType::Insert,
                serde_json::to_value(&full_issue).ok(),
            )
            .await;
        }
    }

    // Re-fetch to get DB-assigned timestamps.
    let row = trakkt_core::db_fetch_one!(
        db,
        IssueRow,
        "SELECT issue_id, workspace_id, team_id, number, title, description, \
                status, priority, assignee_id, creator_id, \
                CAST(due_date AS TEXT) AS due_date, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM issues WHERE issue_id = $1",
        &issue_id
    )?;
    Ok(row.into_dto())
}

/// Get a single issue by its UUID, with full details.
pub async fn get_issue_by_id(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<Option<IssueWithDetails>> {
    let sql = format!(
        "{ISSUE_DETAIL_SELECT} WHERE i.issue_id = $1"
    );
    let row = trakkt_core::db_fetch_optional!(
        db,
        IssueDetailRow,
        &sql,
        issue_id
    )?;

    match row {
        Some(r) => {
            let labels = fetch_labels_for_issues(db, std::slice::from_ref(&r.issue_id)).await?;
            let issue_labels = labels
                .into_values()
                .next()
                .unwrap_or_default();
            Ok(Some(r.into_dto(issue_labels)))
        }
        None => Ok(None),
    }
}

/// Get a single issue by workspace-scoped number, with full details.
pub async fn get_issue(
    db: &DbPool,
    workspace_id: &str,
    number: i32,
) -> trakkt_core::Result<Option<IssueWithDetails>> {
    let sql = format!(
        "{ISSUE_DETAIL_SELECT} WHERE i.workspace_id = $1 AND i.number = $2"
    );
    let row = trakkt_core::db_fetch_optional!(
        db,
        IssueDetailRow,
        &sql,
        workspace_id,
        number
    )?;

    match row {
        Some(r) => {
            let labels = fetch_labels_for_issues(db, std::slice::from_ref(&r.issue_id)).await?;
            let issue_labels = labels
                .into_values()
                .next()
                .unwrap_or_default();
            Ok(Some(r.into_dto(issue_labels)))
        }
        None => Ok(None),
    }
}

/// List issues in a workspace with optional filters.
///
/// Supports filtering by status, priority, assignee, label, and text search.
/// Results are ordered by priority ASC (urgent first), then created_at DESC.
pub async fn list_issues(
    db: &DbPool,
    workspace_id: &str,
    filters: &IssueFilters,
) -> trakkt_core::Result<Vec<IssueWithDetails>> {
    let is_pg = db.is_postgres();

    // Dynamic WHERE clause construction.
    // $1 is always workspace_id. Additional params start at $2.
    let mut conditions = vec!["i.workspace_id = $1".to_string()];
    let mut param_idx: usize = 2;

    // We build the SQL string with numbered placeholders and bind values
    // dynamically via db_with_pool! below. Params are bound in the same
    // order as they appear in the SQL.

    if filters.status.is_some() {
        conditions.push(format!("i.status = ${param_idx}"));
        param_idx += 1;
    }

    if filters.priority.is_some() {
        // CAST for Postgres compatibility per CODING_STANDARDS.md.
        if is_pg {
            conditions.push(format!("i.priority = CAST(${param_idx} AS INTEGER)"));
        } else {
            conditions.push(format!("i.priority = ${param_idx}"));
        }
        param_idx += 1;
    }

    if filters.assignee_id.is_some() {
        conditions.push(format!("i.assignee_id = ${param_idx}"));
        param_idx += 1;
    }

    if filters.label_id.is_some() {
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM issue_labels WHERE issue_id = i.issue_id AND label_id = ${param_idx})"
        ));
        param_idx += 1;
    }

    // search must be the last filter — it does not increment param_idx.
    if filters.search.is_some() {
        conditions.push(format!("i.title LIKE ${param_idx} ESCAPE '\\'"));
    }

    let where_clause = conditions.join(" AND ");

    // Inline LIMIT/OFFSET per CODING_STANDARDS.md (sanitized i64 values).
    let limit = filters.limit.unwrap_or(100);
    let offset = filters.offset.unwrap_or(0);

    let sql = format!(
        "{ISSUE_DETAIL_SELECT} \
         WHERE {where_clause} \
         ORDER BY i.priority ASC, i.created_at DESC \
         LIMIT {limit} OFFSET {offset}"
    );

    // Prepare the search term with wildcards, escaping LIKE special chars.
    let search_pattern = filters
        .search
        .as_ref()
        .map(|s| {
            let escaped = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            format!("%{escaped}%")
        });

    // Bind dynamically using db_with_pool!.
    let rows: Vec<IssueDetailRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, IssueDetailRow>(&sql);

        // $1: workspace_id (always present)
        query = query.bind(workspace_id);

        // Bind remaining params in order of their slot assignment.
        if let Some(ref v) = filters.status {
            query = query.bind(v);
        }
        if let Some(v) = filters.priority {
            query = query.bind(v);
        }
        if let Some(ref v) = filters.assignee_id {
            query = query.bind(v);
        }
        if let Some(ref v) = filters.label_id {
            query = query.bind(v);
        }
        if let Some(ref v) = search_pattern {
            query = query.bind(v);
        }

        query.fetch_all(p).await
    })?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-fetch labels for all issues (avoids N+1).
    let issue_ids: Vec<String> = rows.iter().map(|r| r.issue_id.clone()).collect();
    let mut labels_map = fetch_labels_for_issues(db, &issue_ids).await?;

    let results = rows
        .into_iter()
        .map(|r| {
            let labels = labels_map.remove(&r.issue_id).unwrap_or_default();
            r.into_dto(labels)
        })
        .collect();

    Ok(results)
}

/// Update an issue by workspace-scoped number.
///
/// Only fields present in `updates` are changed. `updated_at` is always set.
pub async fn update_issue(
    db: &DbPool,
    workspace_id: &str,
    number: i32,
    updates: &IssueUpdate,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Issue> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    // Dynamic SET clause — params are numbered sequentially starting at $1.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx: usize = 1;

    if updates.title.is_some() {
        set_parts.push(format!("title = ${param_idx}"));
        param_idx += 1;
    }

    if updates.description.is_some() {
        set_parts.push(format!("description = ${param_idx}"));
        param_idx += 1;
    }

    if updates.status.is_some() {
        set_parts.push(format!("status = ${param_idx}"));
        param_idx += 1;
    }

    if updates.priority.is_some() {
        if is_pg {
            set_parts.push(format!("priority = CAST(${param_idx} AS INTEGER)"));
        } else {
            set_parts.push(format!("priority = ${param_idx}"));
        }
        param_idx += 1;
    }

    // Double-Option: Some(None) clears the field, Some(Some(v)) sets it.
    if updates.assignee_id.is_some() {
        set_parts.push(format!("assignee_id = ${param_idx}"));
        param_idx += 1;
    }

    if updates.due_date.is_some() {
        set_parts.push(format!("due_date = ${param_idx}"));
        param_idx += 1;
    }

    // Always update updated_at.
    set_parts.push(format!("updated_at = {now}"));

    let ws_idx = param_idx;
    param_idx += 1;
    let num_idx = param_idx;

    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE issues SET {set_clause} \
         WHERE workspace_id = ${ws_idx} AND number = ${num_idx}"
    );

    // Bind dynamically. Map to rows_affected() inside the closure so both
    // pool arms return the same type (u64).
    let affected: u64 = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql);

        if let Some(ref v) = updates.title {
            query = query.bind(v);
        }
        if let Some(ref v) = updates.description {
            // Double-Option: bind the inner Option (None = NULL, Some = value)
            query = query.bind(v.as_deref());
        }
        if let Some(ref v) = updates.status {
            query = query.bind(v);
        }
        if let Some(v) = updates.priority {
            query = query.bind(v);
        }
        if let Some(ref v) = updates.assignee_id {
            query = query.bind(v.as_deref());
        }
        if let Some(ref v) = updates.due_date {
            query = query.bind(v.as_deref());
        }

        query = query.bind(workspace_id);
        query = query.bind(number);

        query.execute(p).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "issue #{number} not found in workspace {workspace_id}"
        )));
    }

    // Re-fetch the updated issue.
    let row = trakkt_core::db_fetch_one!(
        db,
        IssueRow,
        "SELECT issue_id, workspace_id, team_id, number, title, description, \
                status, priority, assignee_id, creator_id, \
                CAST(due_date AS TEXT) AS due_date, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM issues WHERE workspace_id = $1 AND number = $2",
        workspace_id,
        number
    )?;
    let issue = row.into_dto();

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE,
        &issue.issue_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, issue_id = %issue.issue_id, "Failed to write sync log entry for issue update");
    }

    // WebSocket broadcast — fetch full entity data and send as SyncResponse.
    if let Some(ws) = ws_manager {
        if let Ok(Some(full_issue)) = get_issue_by_id(db, &issue.issue_id).await {
            sync_log_service::broadcast_sync_action(
                ws,
                workspace_id,
                entity_types::ISSUE,
                &issue.issue_id,
                SyncActionType::Update,
                serde_json::to_value(&full_issue).ok(),
            )
            .await;
        }
    }

    Ok(issue)
}

/// Delete an issue by workspace-scoped number.
///
/// Cascading deletes remove associated issue_labels, comments, and watchers.
pub async fn delete_issue(
    db: &DbPool,
    workspace_id: &str,
    number: i32,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch the issue_id first for the sync log entry.
    let issue_row = trakkt_core::db_fetch_optional!(
        db,
        IssueRow,
        "SELECT issue_id, workspace_id, team_id, number, title, description, \
                status, priority, assignee_id, creator_id, \
                CAST(due_date AS TEXT) AS due_date, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM issues WHERE workspace_id = $1 AND number = $2",
        workspace_id,
        number
    )?;

    let issue_row = issue_row.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!(
            "issue #{number} not found in workspace {workspace_id}"
        ))
    })?;

    let issue_id = issue_row.issue_id.clone();

    trakkt_core::db_execute!(
        db,
        "DELETE FROM issues WHERE issue_id = $1",
        &issue_id
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE,
        &issue_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to write sync log entry for issue delete");
    }

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ISSUE,
            &issue_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
}

/// Replace all labels on an issue.
///
/// Deletes existing label associations and inserts the new set.
/// TODO: wrap delete+insert in a transaction (Slice 9 — sync engine).
pub async fn set_issue_labels(
    db: &DbPool,
    issue_id: &str,
    label_ids: &[String],
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Remove existing labels.
    trakkt_core::db_execute!(
        db,
        "DELETE FROM issue_labels WHERE issue_id = $1",
        issue_id
    )?;

    // Insert new labels.
    for label_id in label_ids {
        trakkt_core::db_execute!(
            db,
            "INSERT INTO issue_labels (issue_id, label_id) VALUES ($1, $2)",
            issue_id,
            label_id
        )?;
    }

    // Determine workspace_id for the sync log.
    let ws_id: String = trakkt_core::db_fetch_scalar!(
        db,
        String,
        "SELECT workspace_id FROM issues WHERE issue_id = $1",
        issue_id
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE,
        issue_id,
        &ws_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to write sync log entry for label update");
    }

    // WebSocket broadcast — fetch full entity data with updated labels.
    if let Some(ws) = ws_manager {
        if let Ok(Some(full_issue)) = get_issue_by_id(db, issue_id).await {
            sync_log_service::broadcast_sync_action(
                ws,
                &ws_id,
                entity_types::ISSUE,
                issue_id,
                SyncActionType::Update,
                serde_json::to_value(&full_issue).ok(),
            )
            .await;
        }
    }

    Ok(())
}
