// SPDX-License-Identifier: AGPL-3.0-or-later

//! View service — CRUD operations for the `views` table.
//!
//! Views are saved filter + display option presets that users can name, share,
//! and reorder in the sidebar. Each view belongs to a workspace and a creating
//! user. Shared views are visible to all workspace members.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::enums::FavoriteTarget;
use trakkt_types::models::View;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service::{self, SyncAudience};
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

// ─── Sync visibility ────────────────────────────────────────────────────────

/// Who a view's sync rows are addressed to.
///
/// This service is the one place where the audience genuinely varies per call:
/// a shared view goes to the workspace and an unshared one only to its creator.
/// Derived from the WHERE clause of [`list_views`], which is also the
/// `sync_bootstrap` query:
/// `workspace_id = $1 AND (created_by = $2 OR is_shared = TRUE)`. The sync log
/// has to scope rows the same way — otherwise the entity set a client ends up
/// with depends on whether it bootstrapped or delta-synced.
///
/// Returning a [`SyncAudience`] rather than an `Option<&str>` is what keeps the
/// persisted `visibility_user_id` and the live frame in step: the single value
/// returned here drives both, so an unshared view cannot be logged as private
/// and then broadcast to everyone.
///
/// Note this reads the view's *current* `is_shared`: un-sharing a view makes
/// subsequent rows owner-only, which is the safe direction. Members who already
/// cached it while it was shared keep their stale copy until their next
/// bootstrap — un-sharing does not retroactively evict it.
fn view_audience(view: &View) -> SyncAudience<'_> {
    if view.is_shared {
        SyncAudience::Workspace
    } else {
        SyncAudience::User(view.created_by.as_str())
    }
}

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
            team_id.unwrap_or_default()
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

/// Parameters for creating a new saved view.
pub struct CreateViewParams<'a> {
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub name: &'a str,
    pub icon: Option<&'a str>,
    pub filters: &'a str,
    pub display_options: &'a str,
    pub is_shared: bool,
    pub team_id: Option<&'a str>,
    pub position: i32,
}

/// Create a new saved view in a workspace.
///
/// `params.team_id` scopes the view to a specific team. `params.position`
/// controls the ordering of views in the sidebar.
///
/// The INSERT and its `sync_log` entry are one transaction: a view that commits
/// without its sync row is invisible to every future delta, so a failed log
/// write rolls the view back rather than leaving it stranded.
pub async fn create_view(
    db: &DbPool,
    params: &CreateViewParams<'_>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<View> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let view_id = uuid::Uuid::new_v4().to_string();
    let shared_val = if params.is_shared {
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
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &view_id,
        params.workspace_id,
        params.user_id,
        params.name,
        params.icon,
        params.filters,
        params.display_options,
        params.position,
        params.team_id
    )?;

    // Re-fetch to get DB-assigned timestamps, and so the entry can be scoped
    // from the view's persisted `is_shared`. The row does not exist outside the
    // transaction yet, so the read runs on it.
    let row: ViewRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        ViewRow,
        &format!("{VIEW_SELECT} WHERE view_id = $1"),
        &view_id
    )?;
    let view = row.into_dto();
    let payload = sync_log_service::sync_payload(&view, entity_types::VIEW, &view_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::VIEW,
        &view_id,
        params.workspace_id,
        view_audience(&view),
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(view)
}

/// Parameters for updating a view.
///
/// Only fields that are `Some` are changed. `updated_at` is always set.
///
/// `team_id` uses a double-Option: the outer `Option` controls whether the
/// field should be updated at all, while the inner `Option` allows setting
/// the column to `NULL` (making the view workspace-scoped rather than
/// team-scoped).
pub struct UpdateViewParams<'a> {
    pub view_id: &'a str,
    pub name: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub filters: Option<&'a str>,
    pub display_options: Option<&'a str>,
    pub is_shared: Option<bool>,
    pub sort_order: Option<f64>,
    pub team_id: Option<Option<&'a str>>,
    pub position: Option<i32>,
}

/// Update a view.
///
/// Only fields that are `Some` are changed. `updated_at` is always set.
///
/// The UPDATE and its `sync_log` entry are one transaction — an edit that
/// commits without its sync row leaves every other client showing the old name,
/// filters or share state until it next bootstraps.
pub async fn update_view(
    db: &DbPool,
    params: &UpdateViewParams<'_>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<View> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    // Dynamic SET clause.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx: usize = 1;

    if params.name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if params.icon.is_some() {
        set_parts.push(format!("icon = ${param_idx}"));
        param_idx += 1;
    }
    if params.filters.is_some() {
        let cast = sql_compat::cast_to_json(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("filters = {cast}"));
        param_idx += 1;
    }
    if params.display_options.is_some() {
        let cast = sql_compat::cast_to_json(is_pg, &format!("${param_idx}"));
        set_parts.push(format!("display_options = {cast}"));
        param_idx += 1;
    }
    if let Some(shared) = params.is_shared {
        let shared_val = if shared {
            sql_compat::bool_true(is_pg)
        } else {
            sql_compat::bool_false(is_pg)
        };
        set_parts.push(format!("is_shared = {shared_val}"));
    }
    if params.sort_order.is_some() {
        set_parts.push(format!("sort_order = ${param_idx}"));
        param_idx += 1;
    }
    if params.team_id.is_some() {
        set_parts.push(format!("team_id = ${param_idx}"));
        param_idx += 1;
    }
    if params.position.is_some() {
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

    let mut tx = db.begin().await?;

    // The binds are built at runtime, so this goes through `tx_with!` — the
    // transaction-scoped form of `db_with_pool!`. Running it on the pool instead
    // would put the UPDATE outside the transaction that logs it.
    let affected: u64 = trakkt_core::tx_with!(&mut tx, |e| {
        let mut query = sqlx::query(&sql);

        if let Some(v) = params.name {
            query = query.bind(v);
        }
        if let Some(v) = params.icon {
            query = query.bind(v);
        }
        if let Some(v) = params.filters {
            query = query.bind(v);
        }
        if let Some(v) = params.display_options {
            query = query.bind(v);
        }
        if let Some(v) = params.sort_order {
            query = query.bind(v);
        }
        if let Some(v) = params.team_id {
            // v is Option<&str> — bind as nullable string.
            query = query.bind(v);
        }
        if let Some(v) = params.position {
            query = query.bind(v);
        }

        query = query.bind(params.view_id);

        query.execute(e).await.map(|r| r.rows_affected())
    })?;

    if affected == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "view {} not found", params.view_id
        )));
    }

    // Re-fetch the updated view on the transaction: the new `is_shared` that
    // scopes the entry is not visible on the pool until the commit.
    let row: ViewRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        ViewRow,
        &format!("{VIEW_SELECT} WHERE view_id = $1"),
        params.view_id
    )?;
    let view = row.into_dto();
    let payload = sync_log_service::sync_payload(&view, entity_types::VIEW, params.view_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::VIEW,
        params.view_id,
        &view.workspace_id,
        view_audience(&view),
        SyncActionType::Update,
        payload,
        ws_manager,
    )
    .await?;

    Ok(view)
}

/// Delete a view, and with it every favorite that pinned it.
///
/// The DELETE and its `sync_log` entry are one transaction — a delete that
/// commits without its sync row leaves the view in every other client's sidebar
/// forever, and no later delta can repair it: the row it would have to re-read
/// is gone.
///
/// The favorites go the same way and for the same reason. `favorites.target_id`
/// has no foreign key to `views`, so nothing in either dialect removes them
/// (TRA-10025); left behind they point at nothing, and being a cached type they
/// return after every `SyncReset` because the server still has the row. This is
/// one of the four delete paths
/// `every_favorite_target_is_deleted_with_its_target` holds to that.
pub async fn delete_view(
    db: &DbPool,
    view_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Read the view before deleting it: once the row is gone there is no way to
    // tell whether the delete was for a shared view (workspace-visible) or a
    // personal one (owner only), and the sync entry has to carry that scope.
    // This reads state that predates the transaction, so it stays on the pool
    // ahead of `begin` — once the transaction is open the pool is unreachable on
    // SQLite (see `DbTx`).
    let Some(view) = get_view(db, view_id).await? else {
        return Err(trakkt_core::Error::NotFound(format!(
            "view {view_id} not found"
        )));
    };

    let mut tx = db.begin().await?;

    // Ahead of the DELETE, because after it nothing connects a favorite to the
    // view it named. Nothing is removed yet — `delete_and_record` does that, so
    // the rows cannot go without the entries that evict them from their owners'
    // caches.
    let doomed_favorites =
        crate::favorite_service::doomed_favorites_tx(&mut tx, FavoriteTarget::View, view_id).await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM views WHERE view_id = $1",
        view_id
    )?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound(format!(
            "view {view_id} not found"
        )));
    }

    // A batch rather than `commit_and_deliver`: the view's own entry is no
    // longer the only one, and a favorite's is addressed to its owner alone
    // while the view's follows `view_audience`. One commit, N deliveries.
    let mut batch = sync_log_service::SyncBatch::new();

    batch
        .record(
            &mut tx,
            entity_types::VIEW,
            view_id,
            workspace_id,
            view_audience(&view),
            SyncActionType::Delete,
            None,
        )
        .await?;

    doomed_favorites
        .delete_and_record(&mut tx, &mut batch)
        .await?;

    batch.commit_and_deliver(tx, ws_manager).await?;

    Ok(())
}
