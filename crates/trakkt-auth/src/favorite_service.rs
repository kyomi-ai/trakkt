// SPDX-License-Identifier: AGPL-3.0-or-later

//! Favorite service — CRUD operations for the `favorites` table.
//!
//! Favorites are per-user pinned items (teams, projects, views) that appear
//! in a "Favorites" section at the top of the sidebar for quick access.

use trakkt_core::DbPool;
use trakkt_types::models::Favorite;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service::{self, SyncAudience};
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
///
/// The INSERT and its `sync_log` entry are one transaction: a favorite that
/// commits without its sync row never reaches the sidebar of the browser that
/// did not issue the request, and no later delta can repair it.
///
/// A favorite is private to the user who pinned it — `list_favorites` filters on
/// `user_id` — so the entry and the live frame are both scoped to them via
/// [`SyncAudience::User`].
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
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &favorite_id,
        user_id,
        workspace_id,
        target_type,
        target_id
    )?;

    // Re-fetch to get the actual row (may be the existing one on conflict). The
    // row does not exist outside the transaction yet, so the read runs on it.
    let row: FavoriteRow = trakkt_core::tx_fetch_one!(
        &mut tx,
        FavoriteRow,
        &format!(
            "{FAVORITE_SELECT} WHERE user_id = $1 AND workspace_id = $2 \
             AND target_type = $3 AND target_id = $4"
        ),
        user_id,
        workspace_id,
        target_type,
        target_id
    )?;
    let favorite = row.into_dto();
    let payload =
        sync_log_service::sync_payload(&favorite, entity_types::FAVORITE, &favorite.favorite_id);

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::FAVORITE,
        &favorite.favorite_id,
        workspace_id,
        SyncAudience::User(user_id),
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(favorite)
}

/// Remove a favorite by target type and ID.
///
/// The DELETE and its `sync_log` entry are one transaction — an unpin that
/// commits without its sync row leaves the favorite in the sidebar of every
/// other browser the user has open, and no later delta can repair it: the row it
/// would have to re-read is gone.
///
/// Scoped to the owner via [`SyncAudience::User`], matching the insert: only that
/// user's cache ever held the favorite, so only they need the delete.
pub async fn remove_favorite(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    target_type: &str,
    target_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // Fetch the favorite_id before deleting so we can address the correct
    // entity_id. This reads state that predates the transaction, so it stays on
    // the pool ahead of `begin` — once the transaction is open the pool is
    // unreachable on SQLite (see `DbTx`).
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

    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM favorites WHERE favorite_id = $1",
        &id_row.favorite_id
    )?;

    if result.rows_affected() == 0 {
        // Race condition — already gone. That's fine, but the transaction has to
        // be released before returning: nothing was written, and on SQLite it
        // holds the only connection until it ends.
        tx.rollback().await?;
        return Ok(());
    }

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::FAVORITE,
        &id_row.favorite_id,
        workspace_id,
        SyncAudience::User(user_id),
        SyncActionType::Delete,
        None,
        ws_manager,
    )
    .await?;

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
