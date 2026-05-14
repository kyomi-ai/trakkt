// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue service — CRUD operations for the `issues` table.
//!
//! Issues are the core entity in Trakkt. Each issue belongs to a team within
//! a workspace and has a team-scoped auto-incrementing number. The service
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
    status_id: String,
    priority: i32,
    assignee_id: Option<String>,
    creator_id: String,
    due_date: Option<String>,
    project_id: Option<String>,
    milestone_id: Option<String>,
    sort_order: Option<f64>,
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
            status_id: self.status_id,
            priority: self.priority,
            assignee_id: self.assignee_id,
            creator_id: self.creator_id,
            due_date: self.due_date,
            project_id: self.project_id,
            milestone_id: self.milestone_id,
            sort_order: self.sort_order,
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
    status_id: String,
    status_name: String,
    status_category: String,
    priority: i32,
    assignee_id: Option<String>,
    assignee_name: Option<String>,
    creator_id: String,
    creator_name: Option<String>,
    due_date: Option<String>,
    project_id: Option<String>,
    project_name: Option<String>,
    milestone_id: Option<String>,
    parent_identifier: Option<String>,
    parent_title: Option<String>,
    sort_order: Option<f64>,
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
            status_id: self.status_id,
            status_name: self.status_name,
            status_category: self.status_category,
            priority: self.priority,
            assignee_id: self.assignee_id,
            assignee_name: self.assignee_name,
            creator_id: self.creator_id,
            creator_name: self.creator_name,
            due_date: self.due_date,
            project_id: self.project_id,
            project_name: self.project_name,
            milestone_id: self.milestone_id,
            parent_identifier: self.parent_identifier,
            parent_title: self.parent_title,
            sort_order: self.sort_order,
            created_at: self.created_at,
            updated_at: self.updated_at,
            labels,
        }
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Base SELECT for issue detail queries with JOINs to teams, users, statuses, and projects.
const ISSUE_DETAIL_SELECT: &str = "\
    SELECT i.issue_id, i.workspace_id, i.team_id, t.key AS team_key, \
           i.number, i.title, i.description, i.status_id, \
           s.name AS status_name, s.category AS status_category, \
           i.priority, \
           i.assignee_id, assignee.name AS assignee_name, \
           i.creator_id, creator.name AS creator_name, \
           CAST(i.due_date AS TEXT) AS due_date, \
           i.project_id, p.name AS project_name, i.milestone_id, \
           (SELECT t2.key || '-' || p2.number FROM issue_relations r2 \
            JOIN issues p2 ON p2.issue_id = r2.source_issue_id \
            JOIN teams t2 ON t2.team_id = p2.team_id \
            WHERE r2.target_issue_id = i.issue_id AND r2.relation_type = 'parent') AS parent_identifier, \
           (SELECT p2.title FROM issue_relations r2 \
            JOIN issues p2 ON p2.issue_id = r2.source_issue_id \
            WHERE r2.target_issue_id = i.issue_id AND r2.relation_type = 'parent') AS parent_title, \
           i.sort_order, \
           CAST(i.created_at AS TEXT) AS created_at, \
           CAST(i.updated_at AS TEXT) AS updated_at \
    FROM issues i \
    JOIN teams t ON t.team_id = i.team_id \
    JOIN statuses s ON s.status_id = i.status_id \
    LEFT JOIN users assignee ON assignee.user_id = i.assignee_id \
    JOIN users creator ON creator.user_id = i.creator_id \
    LEFT JOIN projects p ON p.project_id = i.project_id";

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
        team_id: Option<String>,
        name: String,
        color: String,
        created_at: String,
    }

    let sql = format!(
        "SELECT il.issue_id, l.label_id, l.workspace_id, l.team_id, l.name, l.color, \
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
            team_id: row.team_id,
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
/// The issue `number` is auto-incremented per team. Labels are attached
/// via the `issue_labels` junction table.
pub async fn create_issue(
    db: &DbPool,
    params: &CreateIssueParams,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Issue> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let issue_id = uuid::Uuid::new_v4().to_string();

    // Look up the default status for this workspace.
    let default_status = crate::status_service::get_default_status(db, &params.workspace_id).await?;

    // Atomic number generation: the subquery computes the next number inside the
    // INSERT statement so the MAX read and INSERT happen atomically, preventing
    // race conditions where concurrent creates read the same MAX.
    // Numbers are scoped per-team (e.g. ENG-1, ENG-2, DES-1, DES-2).
    let due_date_cast = if is_pg { "CAST($10 AS TIMESTAMPTZ)" } else { "$10" };
    let sql = format!(
        "INSERT INTO issues \
            (issue_id, workspace_id, team_id, number, title, description, \
             status_id, priority, assignee_id, creator_id, due_date, \
             project_id, milestone_id, created_at, updated_at) \
         VALUES ($1, $2, $3, \
                 (SELECT COALESCE(MAX(number), 0) + 1 FROM issues WHERE team_id = $3), \
                 $4, $5, $6, $7, $8, $9, {due_date_cast}, $11, $12, {now}, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &issue_id,
        &params.workspace_id,
        &params.team_id,
        &params.title,
        params.description.as_deref(),
        &default_status.status_id,
        params.priority,
        params.assignee_id.as_deref(),
        &params.creator_id,
        params.due_date.as_deref(),
        params.project_id.as_deref(),
        params.milestone_id.as_deref()
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
    if let Some(ws) = ws_manager
        && let Ok(Some(full_issue)) = get_issue_by_id(db, &issue_id).await
    {
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

    // Auto-watch: creator watches the issue they just created (best-effort).
    if let Err(e) = crate::watcher_service::watch_issue(db, &issue_id, &params.creator_id).await {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to auto-watch issue for creator");
    }

    // Re-fetch to get DB-assigned timestamps.
    let row = trakkt_core::db_fetch_one!(
        db,
        IssueRow,
        "SELECT issue_id, workspace_id, team_id, number, title, description, \
                status_id, priority, assignee_id, creator_id, \
                CAST(due_date AS TEXT) AS due_date, \
                project_id, milestone_id, sort_order, \
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

/// Get a single issue by team key + number (e.g. "ENG-42"), with full details.
pub async fn get_issue(
    db: &DbPool,
    workspace_id: &str,
    team_key: &str,
    number: i32,
) -> trakkt_core::Result<Option<IssueWithDetails>> {
    let sql = format!(
        "{ISSUE_DETAIL_SELECT} WHERE i.workspace_id = $1 AND t.key = $2 AND i.number = $3"
    );
    let row = trakkt_core::db_fetch_optional!(
        db,
        IssueDetailRow,
        &sql,
        workspace_id,
        team_key,
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
    team_id: Option<&str>,
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

    if team_id.is_some() {
        conditions.push(format!("i.team_id = ${param_idx}"));
        param_idx += 1;
    }

    if filters.status_id.is_some() {
        conditions.push(format!("i.status_id = ${param_idx}"));
        param_idx += 1;
    }

    if let Some(ref cats) = filters.status_categories
        && !cats.is_empty()
    {
        let (in_clause, next_idx) = trakkt_core::db::in_clause_placeholders(cats.len(), param_idx);
        conditions.push(format!("s.category IN {in_clause}"));
        param_idx = next_idx;
    }

    if let Some(ref cats) = filters.exclude_status_categories
        && !cats.is_empty()
    {
        let (in_clause, next_idx) = trakkt_core::db::in_clause_placeholders(cats.len(), param_idx);
        conditions.push(format!("s.category NOT IN {in_clause}"));
        param_idx = next_idx;
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
        if let Some(ref v) = team_id {
            query = query.bind(v);
        }
        if let Some(ref v) = filters.status_id {
            query = query.bind(v);
        }
        if let Some(ref cats) = filters.status_categories {
            for cat in cats {
                query = query.bind(cat);
            }
        }
        if let Some(ref cats) = filters.exclude_status_categories {
            for cat in cats {
                query = query.bind(cat);
            }
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

/// Update an issue by team key + number (e.g. "ENG-42").
///
/// Only fields present in `updates` are changed. `updated_at` is always set.
/// When `team_id` changes, the issue is renumbered in the target team.
pub async fn update_issue(
    db: &DbPool,
    workspace_id: &str,
    team_key: &str,
    number: i32,
    updates: &IssueUpdate,
    actor_user_id: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Issue> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    // Resolve the issue_id first — needed for parent validation, renumbering,
    // and re-fetch. This also validates the issue exists.
    let issue_id: String = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_scalar::<_, String>(
            "SELECT i.issue_id FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             WHERE i.workspace_id = $1 AND t.key = $2 AND i.number = $3"
        )
        .bind(workspace_id)
        .bind(team_key)
        .bind(number)
        .fetch_optional(p)
        .await
    })?
    .ok_or_else(|| {
        trakkt_core::Error::NotFound(format!(
            "issue {team_key}-{number} not found in workspace {workspace_id}"
        ))
    })?;

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

    if updates.status_id.is_some() {
        set_parts.push(format!("status_id = ${param_idx}"));
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
        if is_pg {
            set_parts.push(format!("due_date = CAST(${param_idx} AS TIMESTAMPTZ)"));
        } else {
            set_parts.push(format!("due_date = ${param_idx}"));
        }
        param_idx += 1;
    }

    if updates.project_id.is_some() {
        set_parts.push(format!("project_id = ${param_idx}"));
        param_idx += 1;
    }

    if updates.milestone_id.is_some() {
        set_parts.push(format!("milestone_id = ${param_idx}"));
        param_idx += 1;
    }

    if updates.sort_order.is_some() {
        set_parts.push(format!("sort_order = ${param_idx}"));
        param_idx += 1;
    }

    // When team_id changes, assign a new number in the target team atomically.
    if updates.team_id.is_some() {
        set_parts.push(format!("team_id = ${param_idx}"));
        param_idx += 1;
        set_parts.push(format!(
            "number = (SELECT COALESCE(MAX(number), 0) + 1 FROM issues WHERE team_id = ${}", param_idx - 1
        ));
        // Close the subquery paren.
        if let Some(last) = set_parts.last_mut() {
            last.push(')');
        }
    }

    // Always update updated_at.
    set_parts.push(format!("updated_at = {now}"));

    let id_idx = param_idx;

    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE issues SET {set_clause} WHERE issue_id = ${id_idx}"
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
        if let Some(ref v) = updates.status_id {
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
        if let Some(ref v) = updates.project_id {
            query = query.bind(v.as_deref());
        }
        if let Some(ref v) = updates.milestone_id {
            query = query.bind(v.as_deref());
        }
        if let Some(ref v) = updates.sort_order {
            query = query.bind(*v);
        }
        if let Some(ref v) = updates.team_id {
            query = query.bind(v.as_str());
        }

        query = query.bind(&issue_id);

        query.execute(p).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "issue {team_key}-{number} not found in workspace {workspace_id}"
        )));
    }

    // Re-fetch the updated issue by UUID (number may have changed on team reassignment).
    let row = trakkt_core::db_fetch_one!(
        db,
        IssueRow,
        "SELECT issue_id, workspace_id, team_id, number, title, description, \
                status_id, priority, assignee_id, creator_id, \
                CAST(due_date AS TEXT) AS due_date, \
                project_id, milestone_id, sort_order, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM issues WHERE issue_id = $1",
        &issue_id
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
    if let Some(ws) = ws_manager
        && let Ok(Some(full_issue)) = get_issue_by_id(db, &issue.issue_id).await
    {
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

    // ── Notification triggers (best-effort) ─────────────────────────────
    if let Some(actor_id) = actor_user_id {
        // Assignee auto-watch: when assigned, auto-add as watcher.
        if let Some(Some(ref assignee_id)) = updates.assignee_id
            && let Err(e) = crate::watcher_service::watch_issue(db, &issue_id, assignee_id).await
        {
            tracing::warn!(error = %e, issue_id = %issue_id, "Failed to auto-watch issue for assignee");
        }

        // Determine which notification types to send.
        let mut types_to_notify = Vec::new();
        if updates.status_id.is_some() {
            types_to_notify.push(crate::notification_service::TYPE_STATUS_CHANGED);
        }
        if matches!(updates.assignee_id, Some(Some(_))) {
            types_to_notify.push(crate::notification_service::TYPE_ASSIGNED);
        }
        if updates.priority.is_some() {
            types_to_notify.push(crate::notification_service::TYPE_PRIORITY_CHANGED);
        }

        if !types_to_notify.is_empty() {
            match crate::watcher_service::list_watchers_of_issue(db, &issue_id).await {
                Ok(watchers) => {
                    for notification_type in types_to_notify {
                        for watcher_id in &watchers {
                            if watcher_id == actor_id {
                                continue;
                            }
                            if let Err(e) = crate::notification_service::create_notification(
                                db, workspace_id, watcher_id, &issue_id, notification_type, Some(actor_id), ws_manager,
                            ).await {
                                tracing::warn!(error = %e, "Failed to create notification");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, issue_id = %issue_id, "Failed to list watchers for notifications");
                }
            }
        }
    }

    Ok(issue)
}

/// Delete an issue by team key + number (e.g. "ENG-42").
///
/// Cascading deletes remove associated issue_labels, comments, and watchers.
pub async fn delete_issue(
    db: &DbPool,
    workspace_id: &str,
    team_key: &str,
    number: i32,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch the issue_id first for the sync log entry.
    let issue_row: Option<IssueRow> = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_as::<_, IssueRow>(
            "SELECT i.issue_id, i.workspace_id, i.team_id, i.number, i.title, i.description, \
                    i.status_id, i.priority, i.assignee_id, i.creator_id, \
                    CAST(i.due_date AS TEXT) AS due_date, \
                    i.project_id, i.milestone_id, i.sort_order, \
                    CAST(i.created_at AS TEXT) AS created_at, \
                    CAST(i.updated_at AS TEXT) AS updated_at \
             FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             WHERE i.workspace_id = $1 AND t.key = $2 AND i.number = $3"
        )
        .bind(workspace_id)
        .bind(team_key)
        .bind(number)
        .fetch_optional(p)
        .await
    })?;

    let issue_row = issue_row.ok_or_else(|| {
        trakkt_core::Error::NotFound(format!(
            "issue {team_key}-{number} not found in workspace {workspace_id}"
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
    if let Some(ws) = ws_manager
        && let Ok(Some(full_issue)) = get_issue_by_id(db, issue_id).await
    {
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

    Ok(())
}

/// Set the sort order for an issue (used by board drag-to-reorder).
///
/// Updates only `sort_order` and `updated_at`. Logs to sync_log and broadcasts
/// the updated issue over WebSocket.
pub async fn set_sort_order(
    db: &DbPool,
    workspace_id: &str,
    team_key: &str,
    issue_number: i32,
    sort_order: f64,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let now = sql_compat::now(db.is_postgres());

    // Resolve issue_id first — needed for the UPDATE and sync log/broadcast.
    let issue_id: String = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_scalar::<_, String>(
            "SELECT i.issue_id FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             WHERE i.workspace_id = $1 AND t.key = $2 AND i.number = $3"
        )
        .bind(workspace_id)
        .bind(team_key)
        .bind(issue_number)
        .fetch_optional(p)
        .await
    })?
    .ok_or_else(|| {
        trakkt_core::Error::NotFound(format!(
            "issue {team_key}-{issue_number} not found in workspace {workspace_id}"
        ))
    })?;

    // UPDATE using the resolved issue_id directly — no subquery needed.
    let sql = format!(
        "UPDATE issues SET sort_order = $1, updated_at = {now} WHERE issue_id = $2"
    );

    trakkt_core::db_with_pool!(db, |p| {
        sqlx::query(&sql)
            .bind(sort_order)
            .bind(&issue_id)
            .execute(p)
            .await
            .map(|_| ())
    })?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE,
        &issue_id,
        workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, issue_id = %issue_id, "Failed to write sync log entry for sort_order update");
    }

    // WebSocket broadcast — fetch full entity data and send as SyncResponse.
    if let Some(ws) = ws_manager
        && let Ok(Some(full_issue)) = get_issue_by_id(db, &issue_id).await
    {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ISSUE,
            &issue_id,
            SyncActionType::Update,
            serde_json::to_value(&full_issue).ok(),
        )
        .await;
    }

    Ok(())
}

// ─── Sub-issues ────────────────────────────────────────────────────────────

/// Validate that setting `proposed_parent_id` as the parent of `issue_id`
/// would not create a circular reference.
///
/// Walks up the ancestor chain from the proposed parent via `issue_relations`
/// (relation_type = 'parent'). If `issue_id` is encountered, the assignment
/// would create a cycle. Limits traversal to 10 levels to prevent infinite
/// loops from corrupt data.
pub(crate) async fn validate_no_circular_reference(
    db: &DbPool,
    issue_id: &str,
    proposed_parent_id: &str,
) -> trakkt_core::Result<()> {
    if issue_id == proposed_parent_id {
        return Err(trakkt_core::Error::BadRequest(
            "An issue cannot be its own parent".to_string(),
        ));
    }

    let mut current_id = proposed_parent_id.to_owned();
    for _ in 0..10 {
        let parent: Option<String> = trakkt_core::db_with_pool!(db, |p| {
            sqlx::query_scalar::<_, String>(
                "SELECT source_issue_id FROM issue_relations \
                 WHERE target_issue_id = $1 AND relation_type = 'parent'",
            )
            .bind(&current_id)
            .fetch_optional(p)
            .await
        })?;

        match parent {
            Some(pid) => {
                if pid == issue_id {
                    return Err(trakkt_core::Error::BadRequest(
                        "Circular reference: setting this parent would create a cycle".to_string(),
                    ));
                }
                current_id = pid;
            }
            None => return Ok(()),
        }
    }

    Err(trakkt_core::Error::BadRequest(
        "Issue hierarchy too deep (max 10 levels)".to_string(),
    ))
}

/// List sub-issues (direct children) of a given parent issue.
///
/// Queries via the `issue_relations` table (relation_type = 'parent',
/// source = parent, target = child).
pub async fn list_sub_issues(
    db: &DbPool,
    parent_issue_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<IssueWithDetails>> {
    let sql = format!(
        "{ISSUE_DETAIL_SELECT} \
         JOIN issue_relations r ON r.target_issue_id = i.issue_id \
         WHERE r.source_issue_id = $1 AND r.relation_type = 'parent' AND i.workspace_id = $2 \
         ORDER BY i.priority ASC, i.created_at DESC"
    );

    let rows: Vec<IssueDetailRow> = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_as::<_, IssueDetailRow>(&sql)
            .bind(parent_issue_id)
            .bind(workspace_id)
            .fetch_all(p)
            .await
    })?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-fetch labels for all sub-issues.
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
