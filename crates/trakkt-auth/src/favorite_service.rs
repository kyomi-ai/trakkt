// SPDX-License-Identifier: AGPL-3.0-or-later

//! Favorite service — CRUD operations for the `favorites` table.
//!
//! Favorites are per-user pinned items (issues, projects, teams, views) that
//! appear in a "Favorites" section at the top of the sidebar for quick access.
//!
//! # Why the target is a [`FavoriteTarget`] and not a `&str`
//!
//! `favorites.target_id` names a row in whichever table `target_type` selects,
//! so no foreign key can express it and nothing in either dialect's schema takes
//! a favorite away when its target is deleted. The favorite survives, points at
//! nothing, and — because `favorite` is a cached type that `sync_bootstrap`
//! streams — comes back after every `SyncReset`, because the server still has
//! the row (TRA-10025).
//!
//! The fix is that each parent's delete path removes them itself, through
//! [`doomed_favorites_tx`]. That only works while every possible parent is
//! enumerated, which is what [`FavoriteTarget`] is for: taking the enum rather
//! than a client-supplied string means a favorite of a type no delete path
//! handles cannot be stored at all. See that type's docs for what adding a
//! variant obliges you to do, and which of those steps the compiler enforces.

use trakkt_core::db::DbTx;
use trakkt_core::DbPool;
use trakkt_types::enums::FavoriteTarget;
use trakkt_types::models::Favorite;
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service::{self, SyncAudience, SyncBatch};
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
    target: FavoriteTarget,
    target_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Favorite> {
    let target_type = target.as_str();
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
    target: FavoriteTarget,
    target_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let target_type = target.as_str();
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
    target: FavoriteTarget,
    target_id: &str,
) -> trakkt_core::Result<bool> {
    let target_type = target.as_str();
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

// ─── Deleting a target's favorites ──────────────────────────────────────────

/// One favorite about to be removed because its target is being deleted.
#[derive(sqlx::FromRow)]
struct DoomedFavoriteRow {
    favorite_id: String,
    /// The owner. Each row's entry is scoped to *this* user, not to the caller
    /// of the delete — see [`DoomedFavorites::delete_and_record`].
    user_id: String,
    /// Read off the favorite rather than taken from the parent being deleted.
    /// `add_favorite` binds the *pinning user's* workspace and any `target_id`
    /// it is given, so a member of workspace B can hold a favorite naming a
    /// project in workspace A. A `sync_log` row filed under the parent's
    /// workspace would reach no client that holds the favorite.
    workspace_id: String,
}

/// The favorites naming a target, read on a transaction but not yet deleted.
///
/// The two-step shape is deliberate, and it is the whole reason this is a type
/// rather than one function. Deleting the rows and writing their `sync_log`
/// entries are not separable: `favorite` is a cached type, so a favorite deleted
/// on the server without an entry stays in its owner's IndexedDB through every
/// reconnect, with no later delta able to evict it — the defect TRA-9971 and
/// TRA-9957 exist for, and the one `team_service::delete_team` shipped with
/// before TRA-10025, where a bare `DELETE FROM favorites` ran with no entry at
/// all.
///
/// So the DELETE lives inside [`DoomedFavorites::delete_and_record`], together
/// with the entries. Reading without deleting is harmless; deleting without
/// recording is unrepresentable. A caller who forgets the second call has simply
/// not fixed their cascade yet, which
/// `every_favorite_target_is_deleted_with_its_target`
/// (`apps/server/tests/postgres_dialect.rs`) fails on.
pub(crate) struct DoomedFavorites {
    rows: Vec<DoomedFavoriteRow>,
}

/// Read every favorite pointing at `target_id`, ahead of deleting the target.
///
/// Must be called *before* the parent row goes: after it, nothing connects the
/// favorite to anything, which is the entire problem this cascade exists to
/// solve.
///
/// Not filtered by workspace, and not by user. A favorite naming a target that
/// no longer exists is stale in whatever workspace and for whatever user holds
/// it; narrowing to the deleting caller's own would leave the rest behind
/// looking exactly like the bug being fixed.
pub(crate) async fn doomed_favorites_tx(
    tx: &mut DbTx,
    target: FavoriteTarget,
    target_id: &str,
) -> trakkt_core::Result<DoomedFavorites> {
    let rows: Vec<DoomedFavoriteRow> = trakkt_core::tx_fetch_all!(
        tx,
        DoomedFavoriteRow,
        "SELECT favorite_id, user_id, workspace_id FROM favorites \
         WHERE target_type = $1 AND target_id = $2",
        target.as_str(),
        target_id
    )?;

    Ok(DoomedFavorites { rows })
}

impl DoomedFavorites {
    /// Delete the favorites read by [`doomed_favorites_tx`] and record one
    /// `sync_log` entry per row on `batch`.
    ///
    /// Deletes by `favorite_id`, one row at a time, rather than by
    /// `(target_type, target_id)` in a single statement. The set deleted is then
    /// exactly the set recorded: a favorite inserted between the read and this
    /// call is left alone rather than removed with no entry to announce it.
    /// SQLite cannot produce that interleaving at all, holding one connection;
    /// Postgres can.
    ///
    /// Each entry is [`SyncAudience::User`] of *that row's* owner. A favorite is
    /// private — `list_favorites` filters on `user_id` and `sync_log`'s
    /// `visibility_user_id` carries the same scoping — so a workspace-wide entry
    /// here would republish who has pinned what to every member, which
    /// [`SyncAudience`]'s own docs name as the failure mode. One entry cannot
    /// serve several owners for the same reason the column holds one user, which
    /// is why this is a loop and not a single entry naming the target: a project
    /// pinned by four people is four rows, four owners and four entries.
    pub(crate) async fn delete_and_record<'a>(
        &'a self,
        tx: &mut DbTx,
        batch: &mut SyncBatch<'a>,
    ) -> trakkt_core::Result<()> {
        for row in &self.rows {
            trakkt_core::tx_execute!(
                tx,
                "DELETE FROM favorites WHERE favorite_id = $1",
                &row.favorite_id
            )?;

            batch
                .record(
                    tx,
                    entity_types::FAVORITE,
                    &row.favorite_id,
                    &row.workspace_id,
                    SyncAudience::User(&row.user_id),
                    SyncActionType::Delete,
                    None,
                )
                .await?;
        }

        Ok(())
    }
}
