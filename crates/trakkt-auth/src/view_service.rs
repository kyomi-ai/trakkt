// SPDX-License-Identifier: AGPL-3.0-or-later

//! View service — CRUD operations for the `views` table.
//!
//! Views are saved filter + display option presets that users can name, share,
//! and reorder in the sidebar. Each view belongs to a workspace and a creating
//! user. Shared views are visible to all workspace members.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::View;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising `views` query results.
///
/// `is_shared` is `bool` — sqlx handles the Postgres `boolean` and
/// SQLite `INTEGER` mapping transparently.
#[derive(sqlx::FromRow)]
struct ViewRow {
    view_id: String,
    workspace_id: String,
    team_id: Option<String>,
    created_by: String,
    name: String,
    icon: Option<String>,
    filters: String,
    display_options: String,
    sort_order: f64,
    position: i32,
    is_shared: bool,
    created_at: String,
    updated_at: String,
}

impl ViewRow {
    fn into_dto(self) -> View {
        View {
            view_id: self.view_id,
            workspace_id: self.workspace_id,
            team_id: self.team_id,
            created_by: self.created_by,
            name: self.name,
            icon: self.icon,
            filters: self.filters,
            display_options: self.display_options,
            sort_order: self.sort_order,
            position: self.position,
            is_shared: self.is_shared,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Base SELECT for view queries.
const VIEW_SELECT: &str = "\
    SELECT view_id, workspace_id, team_id, created_by, name, icon, \
           CAST(filters AS TEXT) AS filters, \
           CAST(display_options AS TEXT) AS display_options, \
           sort_order, position, is_shared, \
           CAST(created_at AS TEXT) AS created_at, \
           CAST(updated_at AS TEXT) AS updated_at \
    FROM views";

// ─── View CRUD ──────────────────────────────────────────────────────────────

/// List views visible to a user: their own views plus shared views in the workspace.
///
/// When `team_id` is `Some`, only views belonging to that team are returned.
/// When `None`, all views for the workspace are returned (no team filter).
///
/// Ordered by position, then sort_order, then creation date.
pub async fn list_views(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    team_id: Option<&str>,
) -> trakkt_core::Result<Vec<View>> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    let (sql, has_team) = if team_id.is_some() {
        (
            format!(
                "{VIEW_SELECT} WHERE workspace_id = $1 AND (created_by = $2 OR is_shared = {bt}) \
                 AND team_id = $3 \
                 ORDER BY position ASC, sort_order ASC, created_at ASC"
            ),
            true,
        )
    } else {
        (
            format!(
                "{VIEW_SELECT} WHERE workspace_id = $1 AND (created_by = $2 OR is_shared = {bt}) \
                 ORDER BY position ASC, sort_order ASC, created_at ASC"
            ),
            false,
        )
    };

    let rows: Vec<ViewRow> = if has_team {
        trakkt_core::db_fetch_all!(
            db,
            ViewRow,
            &sql,
            workspace_id,
            user_id,
            team_id.unwrap()
        )?
    } else {
        trakkt_core::db_fetch_all!(
            db,
            ViewRow,
            &sql,
            workspace_id,
            user_id
        )?
    };
    Ok(rows.into_iter().map(ViewRow::into_dto).collect())
}

/// Get a single view by ID.
pub async fn get_view(
    db: &DbPool,
    view_id: &str,
) -> trakkt_core::Result<Option<View>> {
    let sql = format!("{VIEW_SELECT} WHERE view_id = $1");
    let row = trakkt_core::db_fetch_optional!(
        db,
        ViewRow,
        &sql,
        view_id
    )?;
    Ok(row.map(ViewRow::into_dto))
}

/// Create a new saved view in a workspace.
///
/// `team_id` scopes the view to a specific team. `position` controls the
/// ordering of views in the sidebar.
pub async fn create_view(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    name: &str,
    icon: Option<&str>,
    filters: &str,
    display_options: &str,
    is_shared: bool,
    team_id: Option<&str>,
    position: i32,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<View> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let view_id = uuid::Uuid::new_v4().to_string();
    let shared_val = if is_shared {
        sql_compat::bool_true(is_pg)
    } else {
        sql_compat::bool_false(is_pg)
    };

    let filters_cast = sql_compat::cast_to_json(is_pg, "$6");
    let display_cast = sql_compat::cast_to_json(is_pg, "$7");
    let position_cast = if is_pg {
        "CAST($8 AS INTEGER)"
    } else {
        "$8"
    };

    let sql = format!(
        "INSERT INTO views \
            (view_id, workspace_id, created_by, name, icon, filters, display_options, \
             sort_order, is_shared, team_id, position, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, {filters_cast}, {display_cast}, 0, {shared_val}, $9, {position_cast}, {now}, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &view_id,
        workspace_id,
        user_id,
        name,
        icon,
        filters,
        display_options,
        position,
        team_id
    )?;

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::VIEW,
        &view_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, view_id = %view_id, "Failed to write sync log entry for view create");
    }

    // Re-fetch to get DB-assigned timestamps.
    let sql = format!("{VIEW_SELECT} WHERE view_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        ViewRow,
        &sql,
        &view_id
    )?;
    let view = row.into_dto();

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::VIEW,
            &view_id,
            SyncActionType::Insert,
            serde_json::to_value(&view).ok(),
        )
        .await;
    }

    Ok(view)
}

/// Update a view.
///
/// Only fields that are `Some` are changed. `updated_at` is always set.
///
/// `team_id` uses a double-Option: the outer `Option` controls whether the
/// field should be updated at all, while the inner `Option` allows setting
/// the column to `NULL` (making the view workspace-scoped rather than
/// team-scoped).
pub async fn update_view(
    db: &DbPool,
    view_id: &str,
    name: Option<&str>,
    icon: Option<&str>,
    filters: Option<&str>,
    display_options: Option<&str>,
    is_shared: Option<bool>,
    sort_order: Option<f64>,
    team_id: Option<Option<&str>>,
    position: Option<i32>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<View> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    // Dynamic SET clause.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx: usize = 1;

    if name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if icon.is_some() {
        set_parts.push(format!("icon = ${param_idx}"));
        param_idx += 1;
    }
    if filters.is_some() {
        let cast = sql_compat::cast_to_json(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("filters = {cast}"));
        param_idx += 1;
    }
    if display_options.is_some() {
        let cast = sql_compat::cast_to_json(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("display_options = {cast}"));
        param_idx += 1;
    }
    if let Some(shared) = is_shared {
        let shared_val = if shared {
            sql_compat::bool_true(is_pg)
        } else {
            sql_compat::bool_false(is_pg)
        };
        set_parts.push(format!("is_shared = {shared_val}"));
    }
    if sort_order.is_some() {
        set_parts.push(format!("sort_order = ${param_idx}"));
        param_idx += 1;
    }
    if team_id.is_some() {
        set_parts.push(format!("team_id = ${param_idx}"));
        param_idx += 1;
    }
    if position.is_some() {
        if is_pg {
            set_parts.push(format!("position = CAST(${param_idx} AS INTEGER)"));
        } else {
            set_parts.push(format!("position = ${param_idx}"));
        }
        param_idx += 1;
    }

    // Always update updated_at.
    set_parts.push(format!("updated_at = {now}"));

    let vid_idx = param_idx;
    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE views SET {set_clause} WHERE view_id = ${vid_idx}"
    );

    let affected: u64 = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql);

        if let Some(v) = name {
            query = query.bind(v);
        }
        if let Some(v) = icon {
            query = query.bind(v);
        }
        if let Some(v) = filters {
            query = query.bind(v);
        }
        if let Some(v) = display_options {
            query = query.bind(v);
        }
        if let Some(v) = sort_order {
            query = query.bind(v);
        }
        if let Some(v) = team_id {
            // v is Option<&str> — bind as nullable string.
            query = query.bind(v);
        }
        if let Some(v) = position {
            query = query.bind(v);
        }

        query = query.bind(view_id);

        query.execute(p).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "view {view_id} not found"
        )));
    }

    // Re-fetch the updated view.
    let sql = format!("{VIEW_SELECT} WHERE view_id = $1");
    let row = trakkt_core::db_fetch_one!(
        db,
        ViewRow,
        &sql,
        view_id
    )?;
    let view = row.into_dto();

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::VIEW,
        view_id,
        &view.workspace_id,
        SyncActionType::Update,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, view_id = %view_id, "Failed to write sync log entry for view update");
    }

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            &view.workspace_id,
            entity_types::VIEW,
            view_id,
            SyncActionType::Update,
            serde_json::to_value(&view).ok(),
        )
        .await;
    }

    Ok(view)
}

/// Delete a view.
pub async fn delete_view(
    db: &DbPool,
    view_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let result = trakkt_core::db_execute!(
        db,
        "DELETE FROM views WHERE view_id = $1",
        view_id
    )?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(format!(
            "view {view_id} not found"
        )));
    }

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::VIEW,
        view_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, view_id = %view_id, "Failed to write sync log entry for view delete");
    }

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::VIEW,
            view_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
}
