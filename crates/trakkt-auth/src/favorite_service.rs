// SPDX-License-Identifier: AGPL-3.0-or-later

//! Favorite service — CRUD operations for the `favorites` table.
//!
//! Favorites are per-user pinned items (teams, projects, views) that appear
//! in a "Favorites" section at the top of the sidebar for quick access.

use trakkt_core::DbPool;
use trakkt_types::models::Favorite;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising `favorites` query results.
#[derive(sqlx::FromRow)]
struct FavoriteRow {
    favorite_id: String,
    user_id: String,
    workspace_id: String,
    target_type: String,
    target_id: String,
    sort_order: f64,
    created_at: String,
}

impl FavoriteRow {
    fn into_dto(self) -> Favorite {
        Favorite {
            favorite_id: self.favorite_id,
            user_id: self.user_id,
            workspace_id: self.workspace_id,
            target_type: self.target_type,
            target_id: self.target_id,
            sort_order: self.sort_order,
            created_at: self.created_at,
        }
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Base SELECT for favorite queries.
const FAVORITE_SELECT: &str = "\
    SELECT favorite_id, user_id, workspace_id, target_type, target_id, \
           sort_order, \
           CAST(created_at AS TEXT) AS created_at \
    FROM favorites";

// ─── Favorite CRUD ─────────────────────────────────────────────────────────

/// List all favorites for a user in a workspace, ordered by sort_order.
pub async fn list_favorites(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<Favorite>> {
    let sql = format!(
        "{FAVORITE_SELECT} WHERE user_id = $1 AND workspace_id = $2 \
         ORDER BY sort_order ASC, created_at ASC"
    );
    let rows: Vec<FavoriteRow> = trakkt_core::db_fetch_all!(
        db,
        FavoriteRow,
        &sql,
        user_id,
        workspace_id
    )?;
    Ok(rows.into_iter().map(FavoriteRow::into_dto).collect())
}

/// Add a favorite. Uses ON CONFLICT DO NOTHING so duplicate adds are idempotent.
///
/// Returns the favorite (newly created or existing).
pub async fn add_favorite(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    target_type: &str,
    target_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Favorite> {
    let is_pg = db.is_postgres();
    let now = trakkt_core::sql_compat::now(is_pg);
    let favorite_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO favorites \
            (favorite_id, user_id, workspace_id, target_type, target_id, sort_order, created_at) \
         VALUES ($1, $2, $3, $4, $5, 0, {now}) \
         ON CONFLICT (user_id, workspace_id, target_type, target_id) DO NOTHING"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &favorite_id,
        user_id,
        workspace_id,
        target_type,
        target_id
    )?;

    // Re-fetch to get the actual row (may be the existing one on conflict).
    let sql = format!(
        "{FAVORITE_SELECT} WHERE user_id = $1 AND workspace_id = $2 \
         AND target_type = $3 AND target_id = $4"
    );
    let row = trakkt_core::db_fetch_one!(
        db,
        FavoriteRow,
        &sql,
        user_id,
        workspace_id,
        target_type,
        target_id
    )?;
    let favorite = row.into_dto();

    // Sync log — best-effort.
    let sync_id = sync_log_service::write_sync_entry(
        db,
        entity_types::FAVORITE,
        &favorite.favorite_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, favorite_id = %favorite.favorite_id, "Failed to write sync log entry for favorite add");
        0
    });

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::FAVORITE,
            &favorite.favorite_id,
            SyncActionType::Insert,
            serde_json::to_value(&favorite).ok(),
            sync_id,
        )
        .await;
    }

    Ok(favorite)
}

/// Remove a favorite by target type and ID.
pub async fn remove_favorite(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    target_type: &str,
    target_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch the favorite_id before deleting so we can broadcast the correct entity_id.
    let sql = "SELECT favorite_id FROM favorites \
         WHERE user_id = $1 AND workspace_id = $2 AND target_type = $3 AND target_id = $4"
        .to_string();

    #[derive(sqlx::FromRow)]
    struct IdRow {
        favorite_id: String,
    }

    let maybe_row = trakkt_core::db_fetch_optional!(
        db,
        IdRow,
        &sql,
        user_id,
        workspace_id,
        target_type,
        target_id
    )?;

    let Some(id_row) = maybe_row else {
        // Already removed — idempotent.
        return Ok(());
    };

    let result = trakkt_core::db_execute!(
        db,
        "DELETE FROM favorites WHERE favorite_id = $1",
        &id_row.favorite_id
    )?;

    if result.rows_affected() == 0 {
        // Race condition — already gone. That's fine.
        return Ok(());
    }

    // Sync log — best-effort.
    let sync_id = sync_log_service::write_sync_entry(
        db,
        entity_types::FAVORITE,
        &id_row.favorite_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, favorite_id = %id_row.favorite_id, "Failed to write sync log entry for favorite remove");
        0
    });

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::FAVORITE,
            &id_row.favorite_id,
            SyncActionType::Delete,
            None,
            sync_id,
        )
        .await;
    }

    Ok(())
}

/// Check whether a target is favorited by a user.
pub async fn is_favorite(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    target_type: &str,
    target_id: &str,
) -> trakkt_core::Result<bool> {
    let count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM favorites \
         WHERE user_id = $1 AND workspace_id = $2 AND target_type = $3 AND target_id = $4",
        user_id,
        workspace_id,
        target_type,
        target_id
    )?;
    Ok(count > 0)
}
