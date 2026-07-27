// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync log service — core persistence layer for the real-time sync protocol.
//!
//! This module provides the server-side CRUD operations for the `sync_log`
//! table. It is used by mutation instrumentation (Phase 2) to record every
//! entity change, and by the WebSocket sync handlers (Phase 3) to stream
//! changes to clients.
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching all other services
//! - `sync_id` is an auto-incrementing integer — Postgres BIGSERIAL, SQLite AUTOINCREMENT
//! - Postgres uses `RETURNING sync_id` to get the assigned ID; SQLite uses `last_insert_rowid()`
//! - `data` is stored as JSONB on Postgres and TEXT on SQLite

use trakkt_core::db::DbTx;
use trakkt_core::sql_compat;
use trakkt_core::{db_execute, db_fetch_all, db_fetch_scalar, tx_execute, tx_fetch_scalar, DbPool};
use trakkt_types::sync::{SyncAction, SyncActionType};

use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `sync_log` query results.
///
/// `data` is TEXT-compatible for both Postgres (JSONB reads as text via sqlx)
/// and SQLite (TEXT column).
#[derive(sqlx::FromRow)]
struct SyncLogRow {
    sync_id: i64,
    entity_type: String,
    entity_id: String,
    workspace_id: String,
    action: String,
    data: Option<String>,
    created_at: String,
}

impl SyncLogRow {
    fn into_sync_action(self) -> trakkt_core::Result<SyncAction> {
        let action = parse_action_type(&self.action)?;
        let data = self
            .data
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| {
                trakkt_core::Error::Internal(format!("failed to parse sync_log data JSON: {e}"))
            })?;

        // Normalise the stored timestamp to RFC 3339.
        // Postgres stores TIMESTAMPTZ which sqlx decodes into a formatted string.
        // SQLite stores TEXT in `datetime('now')` format (ISO-8601 without timezone).
        // We append 'Z' for SQLite timestamps that lack a timezone suffix.
        let timestamp = normalise_timestamp(&self.created_at);

        Ok(SyncAction {
            sync_id: self.sync_id,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            workspace_id: self.workspace_id,
            action,
            data,
            timestamp,
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn action_type_to_str(action: &SyncActionType) -> &'static str {
    match action {
        SyncActionType::Insert => "insert",
        SyncActionType::Update => "update",
        SyncActionType::Delete => "delete",
    }
}

fn parse_action_type(s: &str) -> trakkt_core::Result<SyncActionType> {
    match s {
        "insert" => Ok(SyncActionType::Insert),
        "update" => Ok(SyncActionType::Update),
        "delete" => Ok(SyncActionType::Delete),
        other => Err(trakkt_core::Error::Internal(format!(
            "unknown sync action type: {other}"
        ))),
    }
}

/// Ensure a timestamp string has a UTC timezone marker.
///
/// Postgres TIMESTAMPTZ comes back as e.g. `"2026-04-26T12:34:56.789Z"`.
/// SQLite `datetime('now')` comes back as `"2026-04-26 12:34:56"` (no `Z`).
fn normalise_timestamp(ts: &str) -> String {
    let has_tz = ts.ends_with('Z')
        || ts.contains('+')
        || (ts.contains('-') && ts.len() > 19);
    if has_tz {
        ts.to_string()
    } else {
        format!("{}Z", ts.replace(' ', "T"))
    }
}

// ─── write_sync_entry ────────────────────────────────────────────────────────

/// The `sync_log` INSERT for the active backend.
///
/// Postgres appends `RETURNING sync_id`; SQLite has no RETURNING support here
/// and reads the id back with `last_insert_rowid()` instead.
fn sync_entry_insert_sql(is_pg: bool) -> String {
    let now_expr = sql_compat::now(is_pg);
    // Postgres: `data` is JSONB — the bound JSON string needs the cast.
    // SQLite:   `data` is TEXT — the JSON string goes in as-is.
    let data_expr = sql_compat::cast_to_json(is_pg, "$5");
    let returning = if is_pg { "RETURNING sync_id" } else { "" };
    format!(
        r#"
        INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data, visibility_user_id, created_at)
        VALUES ($1, $2, $3, $4, {data_expr}, $6, {now_expr})
        {returning}
        "#
    )
}

/// Serialise a sync entry payload to the string bound as `data`.
fn serialise_sync_entry_data(
    data: Option<&serde_json::Value>,
) -> trakkt_core::Result<Option<String>> {
    data.map(serde_json::to_string).transpose().map_err(|e| {
        trakkt_core::Error::Internal(format!("failed to serialise sync entry data: {e}"))
    })
}

/// Insert a row into `sync_log` and return the assigned `sync_id`.
///
/// Uses `RETURNING sync_id` on Postgres and `SELECT last_insert_rowid()` on
/// SQLite because the ID is assigned by the database (BIGSERIAL / AUTOINCREMENT).
///
/// This is the non-transactional form: the insert auto-commits on its own, so a
/// failure leaves the caller's mutation already committed with no `sync_log`
/// row to replay it — permanently invisible to delta sync. Services whose
/// mutation and log write have been made atomic use
/// [`write_sync_entry_in_tx`] instead; this form remains for the services that
/// have not been converted yet.
///
/// `visibility_user_id` scopes who may receive this row on delta sync:
/// - `None` — workspace-visible: every member of `workspace_id` receives it.
/// - `Some(user_id)` — only that user receives it.
///
/// Per-user entities (notifications, favorites, notification preferences,
/// unshared views) MUST pass `Some(owner)`. Passing `None` for them replays one
/// member's private rows to the whole workspace, which is the leak TRA-9920
/// fixed. The scope must match what `sync_bootstrap` exposes to the same user,
/// otherwise a client's dataset depends on which sync path it took.
pub async fn write_sync_entry(
    db: &DbPool,
    entity_type: &str,
    entity_id: &str,
    workspace_id: &str,
    visibility_user_id: Option<&str>,
    action: SyncActionType,
    data: Option<serde_json::Value>,
) -> trakkt_core::Result<i64> {
    let is_pg = db.is_postgres();
    let action_str = action_type_to_str(&action);
    let data_str = serialise_sync_entry_data(data.as_ref())?;
    let sql = sync_entry_insert_sql(is_pg);

    let sync_id: i64 = if is_pg {
        // Postgres: RETURNING hands back the assigned BIGSERIAL id.
        db_fetch_scalar!(
            db,
            i64,
            &sql,
            entity_type,
            entity_id,
            workspace_id,
            action_str,
            data_str,
            visibility_user_id
        )
        .map_err(|e| trakkt_core::Error::Internal(format!("failed to write sync entry: {e}")))?
    } else {
        // SQLite: INSERT then query last_insert_rowid(). Correct only because
        // the SQLite pool is pinned to a single connection, so both statements
        // land on the same one — see `write_sync_entry_in_tx` for the form that
        // does not depend on that.
        db_execute!(
            db,
            &sql,
            entity_type,
            entity_id,
            workspace_id,
            action_str,
            data_str,
            visibility_user_id
        )
        .map_err(|e| trakkt_core::Error::Internal(format!("failed to write sync entry: {e}")))?;

        db_fetch_scalar!(db, i64, "SELECT last_insert_rowid()").map_err(|e| {
            trakkt_core::Error::Internal(format!(
                "failed to get last_insert_rowid after sync entry insert: {e}"
            ))
        })?
    };

    tracing::debug!(
        sync_id,
        entity_type,
        entity_id,
        workspace_id,
        visibility_user_id,
        action = action_str,
        "Wrote sync log entry"
    );

    Ok(sync_id)
}

// ─── write_sync_entry_in_tx ──────────────────────────────────────────────────

/// Insert a row into `sync_log` on the caller's open transaction and return the
/// assigned `sync_id`.
///
/// Same insert as [`write_sync_entry`], same `visibility_user_id` contract —
/// the difference is only where it runs. Because the row lands in the caller's
/// transaction, the mutation it describes and the log entry that replays it
/// commit together or not at all: a failure here rolls the mutation back
/// instead of leaving an entity change that no future delta can see.
///
/// The returned `sync_id` is the real id of the row that will be committed. It
/// is safe to broadcast **after** the commit succeeds; never broadcast while
/// this transaction is still open (see [`DbTx`]).
///
/// On SQLite the INSERT and `last_insert_rowid()` run on the transaction's own
/// connection, so the pairing is correct by construction rather than by the
/// pool being pinned to one connection.
pub async fn write_sync_entry_in_tx(
    tx: &mut DbTx,
    entity_type: &str,
    entity_id: &str,
    workspace_id: &str,
    visibility_user_id: Option<&str>,
    action: SyncActionType,
    data: Option<serde_json::Value>,
) -> trakkt_core::Result<i64> {
    let is_pg = tx.is_postgres();
    let action_str = action_type_to_str(&action);
    let data_str = serialise_sync_entry_data(data.as_ref())?;
    let sql = sync_entry_insert_sql(is_pg);

    let sync_id: i64 = if is_pg {
        tx_fetch_scalar!(
            &mut *tx,
            i64,
            &sql,
            entity_type,
            entity_id,
            workspace_id,
            action_str,
            data_str,
            visibility_user_id
        )
        .map_err(|e| trakkt_core::Error::Internal(format!("failed to write sync entry: {e}")))?
    } else {
        tx_execute!(
            &mut *tx,
            &sql,
            entity_type,
            entity_id,
            workspace_id,
            action_str,
            data_str,
            visibility_user_id
        )
        .map_err(|e| trakkt_core::Error::Internal(format!("failed to write sync entry: {e}")))?;

        tx_fetch_scalar!(&mut *tx, i64, "SELECT last_insert_rowid()").map_err(|e| {
            trakkt_core::Error::Internal(format!(
                "failed to get last_insert_rowid after sync entry insert: {e}"
            ))
        })?
    };

    tracing::debug!(
        sync_id,
        entity_type,
        entity_id,
        workspace_id,
        visibility_user_id,
        action = action_str,
        "Wrote sync log entry in transaction"
    );

    Ok(sync_id)
}

// ─── get_entries_since ───────────────────────────────────────────────────────

/// Fetch the sync entries with `sync_id > since_sync_id` that `user_id` is
/// allowed to see in a workspace.
///
/// Workspace-visible rows (`visibility_user_id IS NULL`) go to every member;
/// per-user rows go only to their owner. This is the enforcement point for the
/// per-user entity scope — the client applies whatever it receives, so a row
/// that reaches the wrong user is a leak.
///
/// Results are ordered by `sync_id ASC` (oldest first) and capped by `limit`.
pub async fn get_entries_since(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    since_sync_id: i64,
    limit: i64,
) -> trakkt_core::Result<Vec<SyncAction>> {
    // On Postgres, JSONB columns are decoded as String by sqlx when the target
    // field type is `String`.  On SQLite the column is already TEXT.
    let rows: Vec<SyncLogRow> = db_fetch_all!(
        db,
        SyncLogRow,
        r#"
        SELECT sync_id, entity_type, entity_id, workspace_id, action,
               CAST(data AS TEXT) AS data,
               CAST(created_at AS TEXT) AS created_at
        FROM sync_log
        WHERE workspace_id = $1 AND sync_id > $2
          AND (visibility_user_id IS NULL OR visibility_user_id = $3)
        ORDER BY sync_id ASC
        LIMIT $4
        "#,
        workspace_id,
        since_sync_id,
        user_id,
        limit
    )
    .map_err(|e| trakkt_core::Error::Internal(format!("failed to get sync entries: {e}")))?;

    rows.into_iter()
        .map(SyncLogRow::into_sync_action)
        .collect()
}

// ─── get_latest_sync_id ──────────────────────────────────────────────────────

/// Get the highest `sync_id` for a workspace, or `0` if no entries exist.
///
/// Deliberately NOT filtered by `visibility_user_id`: this is a cursor
/// watermark, not a data read. The next `get_entries_since` call applies the
/// visibility filter, so a watermark that happens to sit on another user's row
/// discloses nothing — while filtering here would hand the client a cursor
/// behind the real head and make it re-request rows forever.
pub async fn get_latest_sync_id(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<i64> {
    // MAX() on an empty table returns a single NULL row — fetch_one with
    // Option<i64> handles this correctly.
    let max: Option<i64> = db_fetch_scalar!(
        db,
        Option<i64>,
        "SELECT MAX(sync_id) FROM sync_log WHERE workspace_id = $1",
        workspace_id
    )
    .map_err(|e| {
        trakkt_core::Error::Internal(format!("failed to get latest sync_id: {e}"))
    })?;

    Ok(max.unwrap_or(0))
}

// ─── is_sync_id_available ────────────────────────────────────────────────────

/// Check whether a specific `sync_id` still exists in `sync_log` for a
/// workspace (i.e. it has not been pruned).
///
/// Deliberately NOT filtered by `visibility_user_id`. A client's cursor is a
/// workspace-wide watermark, so it may legitimately point at a row belonging to
/// another user. Filtering here would report that row as pruned and trigger a
/// spurious `SyncReset` — a full re-bootstrap — on every reconnect.
pub async fn is_sync_id_available(
    db: &DbPool,
    workspace_id: &str,
    sync_id: i64,
) -> trakkt_core::Result<bool> {
    let count: i64 = db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM sync_log WHERE workspace_id = $1 AND sync_id = $2",
        workspace_id,
        sync_id
    )
    .map_err(|e| {
        trakkt_core::Error::Internal(format!("failed to check sync_id availability: {e}"))
    })?;

    Ok(count > 0)
}

// ─── prune_old_entries ───────────────────────────────────────────────────────

/// Delete `sync_log` entries older than `retention_days` days across all
/// workspaces. This is a global pruning operation, not workspace-scoped.
///
/// Returns the number of rows deleted.
pub async fn prune_old_entries(
    db: &DbPool,
    retention_days: i64,
) -> trakkt_core::Result<u64> {
    let is_pg = db.is_postgres();
    let age_filter = sql_compat::ago_days(is_pg, "created_at", "$1");
    let sql = format!("DELETE FROM sync_log WHERE {age_filter}");

    let result = db_execute!(db, &sql, retention_days)
        .map_err(|e| {
            trakkt_core::Error::Internal(format!("failed to prune sync log entries: {e}"))
        })?;

    let deleted = result.rows_affected();
    tracing::info!(deleted, retention_days, "Pruned old sync log entries");

    Ok(deleted)
}

// ─── Broadcast helper ────────────────────────────────────────────────────────

/// Broadcast a `SyncResponse::SyncAction` with the full entity data to all
/// connected clients in the workspace.
///
/// This sends the exact same format as bootstrap/delta sync, so the client's
/// `onmessage` handler can deserialize and apply it directly to the SyncStore.
///
/// `sync_id` must be the id returned by the [`write_sync_entry`] call that
/// recorded this same change, so a client that misses the live frame can spot
/// the gap in the sequence and re-fetch it. Pass `0` when that write failed —
/// `0` is never a real `sync_log` id and means "no sequence information for
/// this frame".
///
/// Best-effort: failures are logged but never propagated.
pub async fn broadcast_sync_action(
    ws_manager: &WebSocketManager,
    workspace_id: &str,
    entity_type: &str,
    entity_id: &str,
    action: SyncActionType,
    data: Option<serde_json::Value>,
    sync_id: i64,
) {
    let Some(json) = sync_action_frame(workspace_id, entity_type, entity_id, action, data, sync_id)
    else {
        return;
    };

    ws_manager.broadcast_raw_to_workspace(workspace_id, &json).await;
}

/// Send a `SyncResponse::SyncAction` with the full entity data to one user's
/// connections only.
///
/// The live-broadcast counterpart of a `write_sync_entry` call that passed
/// `Some(user_id)`: a row delta sync will only ever hand to its owner must not
/// reach the rest of the workspace over the socket either. All of that user's
/// connections receive it — every browser they have open is entitled to their
/// own data.
///
/// `sync_id` follows the same contract as [`broadcast_sync_action`]: the id
/// returned by the matching [`write_sync_entry`], or `0` when that write failed.
///
/// Best-effort: failures are logged but never propagated.
pub async fn send_sync_action_to_user(
    ws_manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    entity_type: &str,
    entity_id: &str,
    action: SyncActionType,
    data: Option<serde_json::Value>,
    sync_id: i64,
) {
    let Some(json) = sync_action_frame(workspace_id, entity_type, entity_id, action, data, sync_id)
    else {
        return;
    };

    ws_manager.send_to_user_raw(user_id, &json).await;
}

/// Serialize one `SyncResponse::SyncAction` frame.
///
/// Returns `None` when the payload cannot be serialized — an unsendable frame
/// is logged and dropped rather than propagated, since live delivery is
/// best-effort and the change is already durable in `sync_log`.
fn sync_action_frame(
    workspace_id: &str,
    entity_type: &str,
    entity_id: &str,
    action: SyncActionType,
    data: Option<serde_json::Value>,
    sync_id: i64,
) -> Option<String> {
    use trakkt_types::sync::SyncResponse;

    let sync_action = SyncAction {
        sync_id,
        entity_type: entity_type.to_string(),
        entity_id: entity_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action,
        data,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    match serde_json::to_string(&SyncResponse::SyncAction(sync_action)) {
        Ok(json) => Some(json),
        Err(e) => {
            tracing::warn!(
                entity_type,
                entity_id,
                "Failed to serialize SyncResponse for delivery: {e}"
            );
            None
        }
    }
}

// ─── Mutation completion ─────────────────────────────────────────────────────

/// Build the sync payload for an insert or update of `entity_type`.
///
/// Callers pass the row they just read back from the database, so the value
/// serialized here is the same shape a bootstrap would stream for that entity
/// type.
///
/// An entry with no payload is skipped outright by the client — on the live
/// frame and on delta alike — because `cache/apply.rs` returns on a data-less
/// insert/update *before* it reaches the entity-type match. A dropped payload is
/// therefore a silently frozen UI, not a cosmetic loss, so a serialization
/// failure is logged rather than discarded with `.ok()`.
pub(crate) fn sync_payload<T: serde::Serialize>(
    entity: &T,
    entity_type: &str,
    entity_id: &str,
) -> Option<serde_json::Value> {
    match serde_json::to_value(entity) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(
                error = %e,
                entity_type,
                entity_id,
                "Failed to serialize entity for sync payload"
            );
            None
        }
    }
}

/// Who a mutation's sync row is addressed to — on both sides of the sync
/// protocol at once.
///
/// A `sync_log` row reaches a client two ways: live over the socket, and later
/// as part of a delta replay. Those are the same audience, and getting them to
/// disagree is precisely the TRA-9920 leak — a row persisted as one member's
/// private data but pushed live to everyone, or the reverse, where a member's
/// live frame never arrives again after a reconnect.
///
/// So the two are not separate parameters. This one value decides the
/// `visibility_user_id` column *and* the delivery call, and
/// [`commit_and_deliver`] is the only thing that reads it. A caller cannot scope
/// the persisted row to one user and broadcast the frame to the workspace,
/// because there is no pair of arguments to disagree about.
///
/// It is also deliberately not `Option<&str>`. `None` is the kind of thing that
/// gets typed when a `user_id` is not to hand, and it would silently mean
/// "publish this to the whole workspace"; `Workspace` has to be chosen on
/// purpose and reads as a decision at the call site.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SyncAudience<'a> {
    /// Visible to every member of the workspace: `visibility_user_id` is NULL
    /// and the live frame is broadcast workspace-wide.
    Workspace,
    /// Visible to one user: `visibility_user_id` is that user, and the live
    /// frame goes only to their connections.
    ///
    /// Required for every entity whose read path filters by `user_id` —
    /// notifications, notification preferences, favorites, and unshared views.
    /// Downgrading one of these to [`SyncAudience::Workspace`] republishes one
    /// member's private rows to the whole workspace.
    User(&'a str),
}

impl<'a> SyncAudience<'a> {
    /// The `visibility_user_id` column value for this audience.
    fn visibility_user_id(self) -> Option<&'a str> {
        match self {
            Self::Workspace => None,
            Self::User(user_id) => Some(user_id),
        }
    }
}

/// Finish a mutation whose statements have already run on `tx`: log the change,
/// commit, then deliver it to `audience`.
///
/// This is the shared tail of every transactional mutation in the service layer,
/// and the ordering is the part that has to be right every time — the `sync_log`
/// entry inside the transaction so the change and the row that replays it commit
/// together, the delivery strictly after the commit so it carries a `sync_id`
/// that exists and so it never runs while the transaction holds the SQLite
/// connection (see [`DbTx`]). Note that even the workspace broadcast reads
/// `workspace_users` from the pool, so "after the commit" is a hard requirement
/// and not a stylistic one.
///
/// `audience` is mandatory rather than defaulted because the wrong answer is a
/// data leak, not a cosmetic bug — see [`SyncAudience`].
///
/// Takes the transaction by value: committing it is part of the job, and no
/// caller has anything left to do on it. Anything the entry's payload needs read
/// back from the database is read by the caller *on this transaction* before
/// handing it over — the new state is not visible on the pool until the commit,
/// and on SQLite the pool is not reachable at all while the transaction is open.
///
/// Not every commit-then-deliver helper belongs here.
/// `team_service::commit_team_update` reads the same way at a glance but is a
/// different function: it does its own transaction-scoped read-back of the team,
/// hard-codes the TEAM entity type and the `Update` action, and returns the row
/// it read to its caller. Generalising it into this signature would take a
/// read-back callback and a second return type to serve one module — a worse
/// abstraction, not a shared one. Leave it where it is.
pub(crate) async fn commit_and_deliver(
    mut tx: DbTx,
    entity_type: &str,
    entity_id: &str,
    workspace_id: &str,
    audience: SyncAudience<'_>,
    action: SyncActionType,
    payload: Option<serde_json::Value>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let sync_id = write_sync_entry_in_tx(
        &mut tx,
        entity_type,
        entity_id,
        workspace_id,
        audience.visibility_user_id(),
        action.clone(),
        payload.clone(),
    )
    .await?;

    tx.commit().await?;

    // Delivery mirrors the column written above — the same `audience` drives
    // both, so the live frame can never reach an audience the persisted row
    // would not.
    if let Some(ws) = ws_manager {
        match audience {
            SyncAudience::Workspace => {
                broadcast_sync_action(
                    ws,
                    workspace_id,
                    entity_type,
                    entity_id,
                    action,
                    payload,
                    sync_id,
                )
                .await;
            }
            SyncAudience::User(user_id) => {
                send_sync_action_to_user(
                    ws,
                    user_id,
                    workspace_id,
                    entity_type,
                    entity_id,
                    action,
                    payload,
                    sync_id,
                )
                .await;
            }
        }
    }

    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trakkt_types::models::{
        Favorite, IssueWithDetails, Label, Project, ProjectMember, ProjectMilestone, ProjectUpdate,
        Status, Team, View,
    };
    use trakkt_types::sync::{entity_types, SyncResponse};

    /// A single-instance manager over a workspace with one member.
    /// `broadcast_raw_to_workspace` resolves recipients from `workspace_users`,
    /// so the rows have to exist for the frame to be delivered anywhere.
    async fn broadcast_fixture(user_id: &str, workspace_id: &str) -> WebSocketManager {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");

        db_execute!(
            &db,
            "INSERT INTO users (user_id, email) VALUES ($1, $2)",
            user_id,
            format!("{user_id}@example.test")
        )
        .expect("insert user");
        db_execute!(
            &db,
            "INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ($1, $2)",
            workspace_id,
            user_id
        )
        .expect("insert workspace");
        db_execute!(
            &db,
            "INSERT INTO workspace_users (workspace_id, user_id) VALUES ($1, $2)",
            workspace_id,
            user_id
        )
        .expect("insert workspace membership");

        WebSocketManager::new(None, db)
    }

    /// Broadcast one action and return the `SyncAction` the client actually
    /// received off the wire.
    async fn broadcast_and_receive(sync_id: i64) -> SyncAction {
        let user_id = "usr_sync_id_probe";
        let workspace_id = "ws_sync_id_probe";
        let manager = broadcast_fixture(user_id, workspace_id).await;

        let mut conn = manager.connect(user_id).expect("connection");
        // Discard the connect heartbeat.
        conn.rx.recv().await.expect("heartbeat frame");

        broadcast_sync_action(
            &manager,
            workspace_id,
            entity_types::ISSUE,
            "iss_probe",
            SyncActionType::Update,
            None,
            sync_id,
        )
        .await;

        let frame = conn.rx.recv().await.expect("broadcast frame");
        match serde_json::from_str::<SyncResponse>(&frame)
            .expect("broadcast frame is a SyncResponse")
        {
            SyncResponse::SyncAction(action) => action,
            other => panic!("expected a sync_action frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn broadcast_carries_the_supplied_sync_id() {
        let action = broadcast_and_receive(4242).await;

        assert_eq!(
            action.sync_id, 4242,
            "the live frame must carry the sync_log id of the change it reports"
        );
        assert_eq!(action.entity_type, entity_types::ISSUE);
        assert_eq!(action.entity_id, "iss_probe");
        assert_eq!(action.workspace_id, "ws_sync_id_probe");
    }

    #[tokio::test]
    async fn broadcast_carries_zero_when_the_sync_entry_was_not_written() {
        let action = broadcast_and_receive(0).await;

        assert_eq!(
            action.sync_id, 0,
            "0 means the change has no sequence information"
        );
    }

    #[test]
    fn test_action_type_to_str_roundtrip() {
        for (action, expected) in [
            (SyncActionType::Insert, "insert"),
            (SyncActionType::Update, "update"),
            (SyncActionType::Delete, "delete"),
        ] {
            assert_eq!(action_type_to_str(&action), expected);
            let parsed = parse_action_type(expected).expect("should parse");
            assert_eq!(action_type_to_str(&parsed), expected);
        }
    }

    #[test]
    fn test_parse_action_type_unknown() {
        let err = parse_action_type("upsert").unwrap_err();
        assert!(err.to_string().contains("unknown sync action type"));
    }

    #[test]
    fn test_normalise_timestamp_postgres_utc() {
        // Postgres-style timestamp already has Z — leave unchanged.
        let ts = "2026-04-26T12:34:56.789Z";
        assert_eq!(normalise_timestamp(ts), ts);
    }

    #[test]
    fn test_normalise_timestamp_sqlite_space_separator() {
        // SQLite datetime('now') produces "2026-04-26 12:34:56".
        let ts = "2026-04-26 12:34:56";
        assert_eq!(normalise_timestamp(ts), "2026-04-26T12:34:56Z");
    }

    #[test]
    fn test_normalise_timestamp_already_has_plus_offset() {
        let ts = "2026-04-26T12:34:56+00:00";
        assert_eq!(normalise_timestamp(ts), ts);
    }

    #[test]
    fn test_normalise_timestamp_negative_offset() {
        let ts = "2026-04-26T07:34:56-05:00";
        assert_eq!(normalise_timestamp(ts), ts);
    }

    // ─── Per-user visibility (TRA-9920) ──────────────────────────────────────

    const WS: &str = "ws_visibility";
    const USER_A: &str = "usr_alice";
    const USER_B: &str = "usr_bob";

    /// A workspace with two members, a team and an issue — enough for real
    /// notifications, favorites and views to be created through their services.
    ///
    /// SQLite runs with `PRAGMA foreign_keys=ON`, so every referenced row has to
    /// exist; this is a real schema, not a stub.
    async fn two_user_workspace() -> DbPool {
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

        db_execute!(
            &db,
            "INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ($1, $2)",
            WS,
            USER_A
        )
        .expect("insert workspace");

        for user_id in [USER_A, USER_B] {
            db_execute!(
                &db,
                "INSERT INTO workspace_users (workspace_id, user_id) VALUES ($1, $2)",
                WS,
                user_id
            )
            .expect("insert workspace membership");
        }

        db_execute!(
            &db,
            "INSERT INTO teams (team_id, workspace_id, name, key) VALUES ($1, $2, $3, $4)",
            "team_vis",
            WS,
            "Visibility",
            "VIS"
        )
        .expect("insert team");

        db_execute!(
            &db,
            "INSERT INTO statuses (status_id, workspace_id, team_id, name, category) \
             VALUES ($1, $2, $3, $4, $5)",
            "sts_vis",
            WS,
            "team_vis",
            "Backlog",
            "backlog"
        )
        .expect("insert status");

        db_execute!(
            &db,
            "INSERT INTO issues \
                (issue_id, workspace_id, team_id, number, title, creator_id, status_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            "iss_vis",
            WS,
            "team_vis",
            1_i32,
            "A leaky issue",
            USER_A,
            "sts_vis"
        )
        .expect("insert issue");

        db
    }

    /// Every `entity_id` of the given type that `user_id`'s delta-from-zero
    /// stream carries.
    async fn delta_entity_ids(db: &DbPool, user_id: &str, entity_type: &str) -> Vec<String> {
        get_entries_since(db, WS, user_id, 0, 10_000)
            .await
            .expect("delta entries")
            .into_iter()
            .filter(|e| e.entity_type == entity_type)
            .map(|e| e.entity_id)
            .collect()
    }

    /// Give A a notification and a favorite. Returns their entity ids.
    async fn seed_user_a_private_entities(db: &DbPool) -> (String, String) {
        crate::notification_service::create_notification(
            db,
            WS,
            USER_A,
            "iss_vis",
            "assigned",
            Some(USER_B),
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("create notification for A");

        let notification_id = crate::notification_service::list_notifications(
            db, USER_A, false, false, None, None, None, 50, 0,
        )
        .await
        .expect("list A's notifications")
        .first()
        .expect("A has a notification")
        .notification_id
        .clone();

        let favorite =
            crate::favorite_service::add_favorite(db, USER_A, WS, "issue", "iss_vis", None)
                .await
                .expect("A favorites the issue");

        (notification_id, favorite.favorite_id)
    }

    #[tokio::test]
    async fn delta_never_carries_another_members_notifications_or_favorites() {
        let db = two_user_workspace().await;
        let (a_notification, a_favorite) = seed_user_a_private_entities(&db).await;

        let b_entries = get_entries_since(&db, WS, USER_B, 0, 10_000)
            .await
            .expect("B's delta");

        // Assert on the rows themselves, not just a count: nothing of A's may
        // appear, by entity id or in any payload.
        for entry in &b_entries {
            assert_ne!(
                entry.entity_id, a_notification,
                "B's delta carried A's notification row: {entry:?}"
            );
            assert_ne!(
                entry.entity_id, a_favorite,
                "B's delta carried A's favorite row: {entry:?}"
            );
            let payload_owner = entry
                .data
                .as_ref()
                .and_then(|d| d.get("user_id"))
                .and_then(|v| v.as_str());
            assert_ne!(
                payload_owner,
                Some(USER_A),
                "B's delta carried a payload owned by A: {entry:?}"
            );
        }

        assert!(
            delta_entity_ids(&db, USER_B, entity_types::NOTIFICATION)
                .await
                .is_empty(),
            "B has no notifications of their own, so their delta must contain none"
        );
        assert!(
            delta_entity_ids(&db, USER_B, entity_types::FAVORITE)
                .await
                .is_empty(),
            "B has no favorites of their own, so their delta must contain none"
        );
    }

    #[tokio::test]
    async fn delta_still_carries_the_users_own_notifications_and_favorites() {
        let db = two_user_workspace().await;
        let (a_notification, a_favorite) = seed_user_a_private_entities(&db).await;

        assert_eq!(
            delta_entity_ids(&db, USER_A, entity_types::NOTIFICATION).await,
            vec![a_notification.clone()],
            "the filter must not over-restrict: A still receives A's notification"
        );
        assert_eq!(
            delta_entity_ids(&db, USER_A, entity_types::FAVORITE).await,
            vec![a_favorite.clone()],
            "the filter must not over-restrict: A still receives A's favorite"
        );

        // And the notification payload — the part that actually leaked — is intact.
        let notification = get_entries_since(&db, WS, USER_A, 0, 10_000)
            .await
            .expect("A's delta")
            .into_iter()
            .find(|e| e.entity_id == a_notification)
            .expect("A's notification row");
        assert_eq!(
            notification
                .data
                .as_ref()
                .and_then(|d| d.get("issue_title"))
                .and_then(|v| v.as_str()),
            Some("A leaky issue"),
            "A's own notification must still arrive with its payload"
        );
    }

    #[tokio::test]
    async fn delta_from_zero_matches_bootstrap_for_per_user_entity_types() {
        let db = two_user_workspace().await;
        seed_user_a_private_entities(&db).await;

        // One of each kind of view, so the parity check covers both branches.
        for (name, is_shared, owner) in [
            ("A's private view", false, USER_A),
            ("A's shared view", true, USER_A),
            ("B's private view", false, USER_B),
        ] {
            crate::view_service::create_view(
                &db,
                &crate::view_service::CreateViewParams {
                    workspace_id: WS,
                    user_id: owner,
                    name,
                    icon: None,
                    filters: "{}",
                    display_options: "{}",
                    is_shared,
                    team_id: None,
                    position: 0,
                },
                None,
            )
            .await
            .expect("create view");
        }

        for user_id in [USER_A, USER_B] {
            // Bootstrap's per-user queries — the reference set. These are the
            // exact calls `handle_sync_bootstrap` makes.
            let bootstrap_notifications: Vec<String> =
                crate::notification_service::list_notifications(
                    &db,
                    user_id,
                    false,
                    false,
                    None,
                    None,
                    None,
                    crate::notification_service::DEFAULT_NOTIFICATION_LIMIT,
                    0,
                )
                .await
                .expect("bootstrap notifications")
                .into_iter()
                .map(|n| n.notification_id)
                .collect();

            let bootstrap_favorites: Vec<String> =
                crate::favorite_service::list_favorites(&db, user_id, WS)
                    .await
                    .expect("bootstrap favorites")
                    .into_iter()
                    .map(|f| f.favorite_id)
                    .collect();

            let bootstrap_views: Vec<String> =
                crate::view_service::list_views(&db, WS, user_id, None)
                    .await
                    .expect("bootstrap views")
                    .into_iter()
                    .map(|v| v.view_id)
                    .collect();

            for (entity_type, bootstrap_ids) in [
                (entity_types::NOTIFICATION, bootstrap_notifications),
                (entity_types::FAVORITE, bootstrap_favorites),
                (entity_types::VIEW, bootstrap_views),
            ] {
                let mut from_delta = delta_entity_ids(&db, user_id, entity_type).await;
                from_delta.sort();
                let mut from_bootstrap = bootstrap_ids;
                from_bootstrap.sort();

                assert_eq!(
                    from_delta, from_bootstrap,
                    "{user_id}'s {entity_type} set must be identical whether they \
                     bootstrapped or delta-synced"
                );
            }
        }
    }

    #[tokio::test]
    async fn shared_views_reach_every_member_and_personal_views_only_their_owner() {
        let db = two_user_workspace().await;

        let shared = crate::view_service::create_view(
            &db,
            &crate::view_service::CreateViewParams {
                workspace_id: WS,
                user_id: USER_A,
                name: "Team roadmap",
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared: true,
                team_id: None,
                position: 0,
            },
            None,
        )
        .await
        .expect("create shared view");

        let personal = crate::view_service::create_view(
            &db,
            &crate::view_service::CreateViewParams {
                workspace_id: WS,
                user_id: USER_A,
                name: "My scratch filter",
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared: false,
                team_id: None,
                position: 1,
            },
            None,
        )
        .await
        .expect("create personal view");

        let a_views = delta_entity_ids(&db, USER_A, entity_types::VIEW).await;
        let b_views = delta_entity_ids(&db, USER_B, entity_types::VIEW).await;

        assert!(
            a_views.contains(&shared.view_id) && a_views.contains(&personal.view_id),
            "the creator sees both their shared and their personal view: {a_views:?}"
        );
        assert_eq!(
            b_views,
            vec![shared.view_id.clone()],
            "another member sees the shared view and only the shared view"
        );

        // Un-sharing must pull the view back to its owner.
        crate::view_service::update_view(
            &db,
            &crate::view_service::UpdateViewParams {
                view_id: &shared.view_id,
                name: None,
                icon: None,
                filters: None,
                display_options: None,
                is_shared: Some(false),
                sort_order: None,
                team_id: None,
                position: None,
            },
            None,
        )
        .await
        .expect("un-share the view");

        let b_after: Vec<String> = get_entries_since(&db, WS, USER_B, 0, 10_000)
            .await
            .expect("B's delta")
            .into_iter()
            .filter(|e| {
                e.entity_type == entity_types::VIEW && matches!(e.action, SyncActionType::Update)
            })
            .map(|e| e.entity_id)
            .collect();
        assert!(
            b_after.is_empty(),
            "once un-shared, the view's updates are owner-only: {b_after:?}"
        );
    }

    #[tokio::test]
    async fn live_notification_frame_reaches_only_its_recipient() {
        let db = two_user_workspace().await;
        let manager = WebSocketManager::new(None, db.clone());

        let mut a_conn = manager.connect(USER_A).expect("A connects");
        let mut b_conn = manager.connect(USER_B).expect("B connects");
        a_conn.rx.recv().await.expect("A's heartbeat");
        b_conn.rx.recv().await.expect("B's heartbeat");

        crate::notification_service::create_notification(
            &db,
            WS,
            USER_A,
            "iss_vis",
            "assigned",
            Some(USER_B),
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            Some(&manager),
        )
        .await
        .expect("notify A");

        let frame = a_conn.rx.recv().await.expect("A receives the notification");
        match serde_json::from_str::<SyncResponse>(&frame).expect("a SyncResponse") {
            SyncResponse::SyncAction(action) => {
                assert_eq!(action.entity_type, entity_types::NOTIFICATION);
                assert_eq!(
                    action
                        .data
                        .as_ref()
                        .and_then(|d| d.get("user_id"))
                        .and_then(|v| v.as_str()),
                    Some(USER_A)
                );
            }
            other => panic!("expected a sync_action frame, got {other:?}"),
        }

        assert!(
            b_conn.rx.try_recv().is_err(),
            "B must not receive a frame for A's notification"
        );
    }

    #[tokio::test]
    async fn cursor_helpers_stay_unfiltered_so_a_foreign_row_is_not_a_reset() {
        let db = two_user_workspace().await;
        let (_notification, _favorite) = seed_user_a_private_entities(&db).await;

        // The newest row in the workspace belongs to A. B's cursor legitimately
        // points at it, and asking about it must not look like a pruned log.
        let head = get_latest_sync_id(&db, WS).await.expect("latest sync id");
        assert!(head > 0, "A's writes advanced the workspace watermark");

        assert!(
            is_sync_id_available(&db, WS, head)
                .await
                .expect("availability check"),
            "a cursor on another user's row must not be reported as pruned — that \
             would force a spurious SyncReset on every reconnect"
        );

        // ...and B's delta from that cursor is simply empty, not a reset.
        assert!(
            get_entries_since(&db, WS, USER_B, head, 10_000)
                .await
                .expect("B's delta")
                .is_empty()
        );
    }

    /// The backfill half of the migration.
    ///
    /// `DbPool::connect` has already applied the migration to this (empty)
    /// database, so the schema change itself is exercised by every test in this
    /// module. What is left to prove is the classification, so this test seeds
    /// rows in their pre-migration shape (`visibility_user_id` NULL, favorites
    /// and views with a NULL payload as those services wrote them) and then runs
    /// the migration's own UPDATE statements, read verbatim from the migration
    /// file — not a copy that could drift from it.
    #[tokio::test]
    async fn migration_backfill_classifies_pre_migration_rows() {
        let db = two_user_workspace().await;

        // Source rows the backfill joins against.
        db_execute!(
            &db,
            "INSERT INTO notifications (notification_id, workspace_id, user_id, issue_id, type) \
             VALUES ($1, $2, $3, $4, $5)",
            "ntf_legacy",
            WS,
            USER_A,
            "iss_vis",
            "assigned"
        )
        .expect("legacy notification");
        db_execute!(
            &db,
            "INSERT INTO favorites (favorite_id, user_id, workspace_id, target_type, target_id) \
             VALUES ($1, $2, $3, $4, $5)",
            "fav_legacy",
            USER_B,
            WS,
            "issue",
            "iss_vis"
        )
        .expect("legacy favorite");
        for (view_id, owner, is_shared) in [
            ("view_personal", USER_A, 0_i32),
            ("view_shared", USER_A, 1_i32),
        ] {
            db_execute!(
                &db,
                "INSERT INTO views (view_id, workspace_id, created_by, name, is_shared) \
                 VALUES ($1, $2, $3, $4, $5)",
                view_id,
                WS,
                owner,
                view_id,
                is_shared
            )
            .expect("legacy view");
        }
        db_execute!(
            &db,
            "INSERT INTO notification_preferences \
                (preference_id, user_id, workspace_id, delivery_channel) \
             VALUES ($1, $2, $3, $4)",
            "pref_legacy",
            USER_B,
            WS,
            "in_app"
        )
        .expect("legacy preferences");

        // Pre-migration sync_log rows: no visibility, and the payloads exactly as
        // the old services wrote them (notifications carried one, the rest did not).
        let legacy_rows: [(&str, &str, Option<String>); 6] = [
            (
                entity_types::NOTIFICATION,
                "ntf_legacy",
                Some(format!(r#"{{"user_id":"{USER_A}"}}"#)),
            ),
            (entity_types::FAVORITE, "fav_legacy", None),
            (entity_types::VIEW, "view_personal", None),
            (entity_types::VIEW, "view_shared", None),
            (
                entity_types::NOTIFICATION_PREFERENCES,
                "pref_legacy",
                Some(format!(r#"{{"user_id":"{USER_B}"}}"#)),
            ),
            (entity_types::ISSUE, "iss_vis", None),
        ];
        for (entity_type, entity_id, data) in &legacy_rows {
            db_execute!(
                &db,
                "INSERT INTO sync_log \
                    (entity_type, entity_id, workspace_id, action, data, visibility_user_id) \
                 VALUES ($1, $2, $3, 'insert', $4, NULL)",
                *entity_type,
                *entity_id,
                WS,
                data.as_deref()
            )
            .expect("legacy sync_log row");
        }

        // Run the migration's backfill, straight from the file.
        const MIGRATION: &str = include_str!(
            "../../../apps/server/migrations-sqlite/20260610600000_sync_log_visibility_user_id.sql"
        );
        // Comment lines are stripped before splitting on `;` — the header prose
        // contains semicolons, and none of the statements do outside of the
        // terminator.
        let sql_only: String = MIGRATION
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let updates: Vec<String> = sql_only
            .split(';')
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| stmt.to_uppercase().starts_with("UPDATE"))
            .collect();
        assert_eq!(
            updates.len(),
            4,
            "expected one backfill statement per per-user entity type"
        );
        for stmt in &updates {
            db_execute!(&db, stmt).expect("run migration backfill statement");
        }

        #[derive(sqlx::FromRow)]
        struct Classified {
            entity_id: String,
            visibility_user_id: Option<String>,
        }

        let classified: Vec<Classified> = db_fetch_all!(
            &db,
            Classified,
            "SELECT entity_id, visibility_user_id FROM sync_log WHERE workspace_id = $1 \
             ORDER BY entity_id",
            WS
        )
        .expect("read back classifications");

        let actual: Vec<(&str, Option<&str>)> = classified
            .iter()
            .map(|c| (c.entity_id.as_str(), c.visibility_user_id.as_deref()))
            .collect();

        assert_eq!(
            actual,
            vec![
                ("fav_legacy", Some(USER_B)),
                ("iss_vis", None),
                ("ntf_legacy", Some(USER_A)),
                ("pref_legacy", Some(USER_B)),
                ("view_personal", Some(USER_A)),
                ("view_shared", None),
            ],
            "backfill must scope notifications, favorites, preferences and personal \
             views to their owner, and leave shared views and workspace entities NULL"
        );
    }

    // ─── Status / project frames and payloads (TRA-9929) ─────────────────────
    //
    // These changes have to survive both halves of the protocol: the live frame
    // has to be a shape the client can parse, and the `sync_log` row it pairs
    // with has to carry a payload the client can apply on reconnect. Each test
    // drives the real service against the real manager and reads the frame back
    // off a connection channel.

    /// A second workspace member watching over a live connection, with the
    /// connect heartbeat already drained.
    async fn watching_member(db: &DbPool) -> (WebSocketManager, crate::websocket::manager::ConnectionHandle) {
        let manager = WebSocketManager::new(None, db.clone());
        let mut conn = manager.connect(USER_B).expect("B connects");
        conn.rx.recv().await.expect("connect heartbeat");
        (manager, conn)
    }

    /// The next frame on a connection, parsed the way the client parses it.
    ///
    /// Going through `SyncResponse` is the point of the test: it is the exact
    /// call `cache/websocket.rs` makes, and it is where the old envelope frame
    /// failed.
    async fn next_sync_action(conn: &mut crate::websocket::manager::ConnectionHandle) -> SyncAction {
        let frame = conn.rx.recv().await.expect("a broadcast frame");
        match serde_json::from_str::<SyncResponse>(&frame).unwrap_or_else(|e| {
            panic!("frame did not parse as a SyncResponse: {e}\nframe: {frame}")
        }) {
            SyncResponse::SyncAction(action) => action,
            other => panic!("expected a sync_action frame, got {other:?}"),
        }
    }

    /// Assert the shape every live insert/update frame must have, and return
    /// its payload.
    fn payload_of(action: &SyncAction, entity_type: &str, entity_id: &str) -> serde_json::Value {
        assert_eq!(action.entity_type, entity_type);
        assert_eq!(action.entity_id, entity_id);
        assert!(
            action.sync_id > 0,
            "the frame must carry the sync_log id of its own row so a client that \
             missed it can spot the gap, got {}",
            action.sync_id
        );
        action.data.clone().unwrap_or_else(|| {
            panic!("an insert/update frame with no payload is skipped by the client: {action:?}")
        })
    }

    async fn create_test_status(db: &DbPool, ws: Option<&WebSocketManager>) -> Status {
        crate::status_service::create_status(
            db,
            &crate::status_service::CreateStatusParams {
                workspace_id: WS,
                team_id: Some("team_vis"),
                name: "Blocked",
                category: "started",
                position: 7,
                color: Some("#0D9488"),
            },
            ws,
        )
        .await
        .expect("create status")
    }

    async fn create_test_project(db: &DbPool, ws: Option<&WebSocketManager>) -> Project {
        crate::project_service::create_project(
            db,
            &crate::project_service::CreateProjectParams {
                workspace_id: WS,
                name: "Apollo",
                description: None,
                icon: None,
                color: None,
                lead_id: None,
                start_date: None,
                target_date: None,
            },
            ws,
        )
        .await
        .expect("create project")
    }

    #[tokio::test]
    async fn status_create_frame_carries_the_new_status() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;

        let status = create_test_status(&db, Some(&manager)).await;

        let action = next_sync_action(&mut conn).await;
        assert!(matches!(action.action, SyncActionType::Insert));
        let data = payload_of(&action, entity_types::STATUS, &status.status_id);

        let received: Status =
            serde_json::from_value(data).expect("payload deserializes into a Status");
        assert_eq!(
            received, status,
            "the frame must carry the same row the caller got back"
        );
        assert!(
            !received.created_at.is_empty(),
            "the payload is built after the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );
    }

    #[tokio::test]
    async fn project_create_frame_carries_the_new_project() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;

        let project = create_test_project(&db, Some(&manager)).await;

        let action = next_sync_action(&mut conn).await;
        assert!(matches!(action.action, SyncActionType::Insert));
        let data = payload_of(&action, entity_types::PROJECT, &project.project_id);

        let received: Project =
            serde_json::from_value(data).expect("payload deserializes into a Project");
        assert_eq!(received, project);
        assert!(
            !received.created_at.is_empty() && !received.updated_at.is_empty(),
            "the DB-assigned timestamps have to be in the payload"
        );
    }

    #[tokio::test]
    async fn project_update_frame_carries_the_updated_project() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;
        let project = create_test_project(&db, Some(&manager)).await;
        next_sync_action(&mut conn).await; // the create frame

        let updated = crate::project_service::update_project(
            &db,
            &crate::project_service::UpdateProjectParams {
                project_id: &project.project_id,
                name: Some("Apollo II"),
                description: None,
                icon: None,
                color: None,
                status: None,
                lead_id: None,
                start_date: None,
                target_date: None,
                archived_at: None,
            },
            Some(&manager),
        )
        .await
        .expect("update project");

        let action = next_sync_action(&mut conn).await;
        assert!(matches!(action.action, SyncActionType::Update));
        let data = payload_of(&action, entity_types::PROJECT, &project.project_id);

        let received: Project =
            serde_json::from_value(data).expect("payload deserializes into a Project");
        assert_eq!(received, updated);
        assert_eq!(
            received.name, "Apollo II",
            "the frame must carry the new value, not the row as it was before"
        );
    }

    // ─── Member and posted-update frames and payloads (TRA-9940) ─────────────
    //
    // Both used to be reported as an Update to the *parent project*, carrying a
    // project row that neither operation changes — `add_project_member`,
    // `remove_project_member` and `create_project_update` each write to exactly
    // one satellite table and never touch `projects`. So the frame said
    // "something about this project changed", the client re-upserted an
    // identical project, and the membership or the posted update itself was
    // never on the wire at all, live or on reconnect.

    /// The sync entity id for a membership: `project_members` has a composite
    /// primary key and no surrogate id, so the two columns are joined. Written
    /// out here rather than shared with the service, so a change to the id
    /// scheme has to be made deliberately in both places.
    fn member_entity_id(project_id: &str, user_id: &str) -> String {
        format!("{project_id}:{user_id}")
    }

    #[tokio::test]
    async fn project_member_add_frame_carries_the_new_member() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;
        let project = create_test_project(&db, Some(&manager)).await;
        next_sync_action(&mut conn).await; // the create frame

        crate::project_service::add_project_member(
            &db,
            &project.project_id,
            USER_B,
            "member",
            WS,
            Some(&manager),
        )
        .await
        .expect("add member");

        let action = next_sync_action(&mut conn).await;
        assert!(
            matches!(action.action, SyncActionType::Insert),
            "adding a member creates a membership row, so the frame is an \
             Insert of that row — not an Update of the parent project"
        );
        let data = payload_of(
            &action,
            entity_types::PROJECT_MEMBER,
            &member_entity_id(&project.project_id, USER_B),
        );

        let received: ProjectMember =
            serde_json::from_value(data).expect("payload deserializes into a ProjectMember");
        assert_eq!(received.project_id, project.project_id);
        assert_eq!(
            received.user_id, USER_B,
            "the frame has to name who was added — a project row cannot say that"
        );
        assert_eq!(received.role, "member");
        assert!(
            !received.created_at.is_empty(),
            "the payload is built after the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );
    }

    #[tokio::test]
    async fn project_member_remove_frame_is_a_member_delete() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;
        let project = create_test_project(&db, Some(&manager)).await;
        next_sync_action(&mut conn).await; // the create frame

        crate::project_service::add_project_member(
            &db,
            &project.project_id,
            USER_B,
            "member",
            WS,
            Some(&manager),
        )
        .await
        .expect("add member");
        next_sync_action(&mut conn).await; // the member-add frame

        crate::project_service::remove_project_member(
            &db,
            &project.project_id,
            USER_B,
            WS,
            Some(&manager),
        )
        .await
        .expect("remove member");

        let action = next_sync_action(&mut conn).await;
        assert!(
            matches!(action.action, SyncActionType::Delete),
            "removing a member deletes the membership row, so the frame is a \
             Delete — not an Update of a project that did not change"
        );
        assert_eq!(action.entity_type, entity_types::PROJECT_MEMBER);
        assert_eq!(
            action.entity_id,
            member_entity_id(&project.project_id, USER_B),
            "the delete has to name the same key the add upserted, or the \
             client's cache delete misses the row it is meant to remove"
        );
        assert!(
            action.sync_id > 0,
            "the frame must carry the sync_log id of its own row so a client \
             that missed it can spot the gap, got {}",
            action.sync_id
        );
        assert!(
            action.data.is_none(),
            "a delete has no row left to send: {action:?}"
        );
    }

    #[tokio::test]
    async fn project_update_create_frame_carries_the_new_update() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;
        let project = create_test_project(&db, Some(&manager)).await;
        next_sync_action(&mut conn).await; // the create frame

        let posted = crate::project_service::create_project_update(
            &db,
            &project.project_id,
            USER_A,
            "at_risk",
            Some("Blocked on the vendor"),
            Some(&manager),
            WS,
        )
        .await
        .expect("post a project update");

        let action = next_sync_action(&mut conn).await;
        assert!(
            matches!(action.action, SyncActionType::Insert),
            "posting an update creates a row, so the frame is an Insert of that \
             row — not an Update of the parent project"
        );
        let data = payload_of(&action, entity_types::PROJECT_UPDATE, &posted.update_id);

        let received: ProjectUpdate =
            serde_json::from_value(data).expect("payload deserializes into a ProjectUpdate");
        assert_eq!(
            received, posted,
            "the frame must carry the same row the caller got back"
        );
        assert_eq!(
            received.health, "at_risk",
            "the health the update was posted with has to be on the wire — the \
             parent project row carries no health at all"
        );
        assert_eq!(received.body.as_deref(), Some("Blocked on the vendor"));
        assert!(
            !received.created_at.is_empty(),
            "the payload is built after the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );
    }

    /// The durable half for memberships and posted updates. Run with **no
    /// `ws_manager`**, so the live frame cannot satisfy any of it: this is what
    /// a client that was offline for the whole thing replays on reconnect.
    ///
    /// "Reaches a second session" and "survives a reconnect" are separate
    /// criteria — the three tests above cover the first, this one covers the
    /// second, and neither can stand in for the other.
    #[tokio::test]
    async fn delta_carries_a_payload_for_every_member_and_posted_update_write() {
        let db = two_user_workspace().await;

        let project = create_test_project(&db, None).await;
        crate::project_service::add_project_member(
            &db,
            &project.project_id,
            USER_B,
            "member",
            WS,
            None,
        )
        .await
        .expect("add member");
        let posted = crate::project_service::create_project_update(
            &db,
            &project.project_id,
            USER_A,
            "at_risk",
            Some("Blocked on the vendor"),
            None,
            WS,
        )
        .await
        .expect("post a project update");
        // Removed last so the delete's stored row can be inspected alongside the
        // add's — the reconnecting client replays both in order.
        crate::project_service::remove_project_member(&db, &project.project_id, USER_B, WS, None)
            .await
            .expect("remove member");

        let members: Vec<ProjectMember> =
            delta_payloads(&db, USER_B, entity_types::PROJECT_MEMBER).await;
        assert_eq!(members.len(), 1, "one membership add");
        assert_eq!(members[0].project_id, project.project_id);
        assert_eq!(
            members[0].user_id, USER_B,
            "the stored row has to name who was added, or a reconnecting client \
             learns nothing it can act on"
        );
        assert_eq!(members[0].role, "member");
        assert!(
            !members[0].created_at.is_empty(),
            "the payload is built from the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );

        let updates: Vec<ProjectUpdate> =
            delta_payloads(&db, USER_B, entity_types::PROJECT_UPDATE).await;
        assert_eq!(updates.len(), 1, "one posted update");
        assert_eq!(updates[0], posted);
        assert_eq!(
            updates[0].health, "at_risk",
            "a posted update has to survive a reconnect with its health intact"
        );

        // `delta_payloads` skips deletes, so the removal is checked directly:
        // it must be a stored row of its own, not merely an absence.
        let entries = get_entries_since(&db, WS, USER_B, 0, 10_000)
            .await
            .expect("B's delta");
        let deletes: Vec<&SyncAction> = entries
            .iter()
            .filter(|e| {
                e.entity_type == entity_types::PROJECT_MEMBER
                    && matches!(e.action, SyncActionType::Delete)
            })
            .collect();
        assert_eq!(
            deletes.len(),
            1,
            "the removal has to be its own stored row, or a client that was \
             offline for it still shows the member after reconnecting"
        );
        assert_eq!(
            deletes[0].entity_id,
            member_entity_id(&project.project_id, USER_B),
            "the stored delete has to name the same key the stored add did"
        );

        assert!(
            !entries.iter().any(|e| e.entity_type == entity_types::PROJECT
                && matches!(e.action, SyncActionType::Update)),
            "none of these three operations writes to the `projects` table, so \
             none of them may claim the project changed: {entries:?}"
        );
    }

    // ─── Milestone frames and payloads (TRA-9938) ────────────────────────────
    //
    // Milestones showed up at bootstrap and then froze: the sync entries were
    // written with no payload, so the client's data-less guard dropped every
    // insert and update before it could reach any entity arm.

    async fn create_test_milestone(
        db: &DbPool,
        project_id: &str,
        name: &str,
        ws: Option<&WebSocketManager>,
    ) -> ProjectMilestone {
        crate::project_service::create_milestone(
            db,
            project_id,
            name,
            Some("Ship the first cut"),
            Some("2026-09-01"),
            ws,
            WS,
        )
        .await
        .expect("create milestone")
    }

    #[tokio::test]
    async fn milestone_create_frame_carries_the_new_milestone() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;
        let project = create_test_project(&db, Some(&manager)).await;
        next_sync_action(&mut conn).await; // the project create frame

        let milestone = create_test_milestone(&db, &project.project_id, "Beta", Some(&manager)).await;

        let action = next_sync_action(&mut conn).await;
        assert!(matches!(action.action, SyncActionType::Insert));
        let data = payload_of(
            &action,
            entity_types::PROJECT_MILESTONE,
            &milestone.milestone_id,
        );

        let received: ProjectMilestone =
            serde_json::from_value(data).expect("payload deserializes into a ProjectMilestone");
        assert_eq!(
            received, milestone,
            "the frame must carry the same row the caller got back"
        );
        assert!(
            !received.created_at.is_empty(),
            "the payload is built after the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );
        assert_eq!(
            received.target_date.as_deref(),
            Some("2026-09-01"),
            "the date the milestone was created with has to survive the round trip"
        );
    }

    #[tokio::test]
    async fn milestone_update_frame_carries_the_updated_milestone() {
        let db = two_user_workspace().await;
        let (manager, mut conn) = watching_member(&db).await;
        let project = create_test_project(&db, Some(&manager)).await;
        next_sync_action(&mut conn).await; // the project create frame
        let milestone = create_test_milestone(&db, &project.project_id, "Beta", Some(&manager)).await;
        next_sync_action(&mut conn).await; // the milestone create frame

        let updated = crate::project_service::update_milestone(
            &db,
            &milestone.milestone_id,
            Some("Beta 2"),
            None,
            Some(Some("2026-10-15")),
            Some(&manager),
            WS,
        )
        .await
        .expect("update milestone");

        let action = next_sync_action(&mut conn).await;
        assert!(matches!(action.action, SyncActionType::Update));
        let data = payload_of(
            &action,
            entity_types::PROJECT_MILESTONE,
            &milestone.milestone_id,
        );

        let received: ProjectMilestone =
            serde_json::from_value(data).expect("payload deserializes into a ProjectMilestone");
        assert_eq!(received, updated);
        assert_eq!(
            received.name, "Beta 2",
            "the frame must carry the new name, not the row as it was before"
        );
        assert_eq!(
            received.target_date.as_deref(),
            Some("2026-10-15"),
            "a re-dated milestone has to arrive re-dated"
        );
    }

    /// The durable half for milestones. Run with **no `ws_manager`**, so the
    /// live frame cannot satisfy any of it: this is what a client that was
    /// offline for the whole thing replays on reconnect.
    #[tokio::test]
    async fn delta_carries_a_payload_for_every_milestone_write() {
        let db = two_user_workspace().await;

        let project = create_test_project(&db, None).await;
        let created = create_test_milestone(&db, &project.project_id, "Beta", None).await;
        let updated = crate::project_service::update_milestone(
            &db,
            &created.milestone_id,
            Some("Beta 2"),
            None,
            Some(Some("2026-10-15")),
            None,
            WS,
        )
        .await
        .expect("update milestone");

        let payloads: Vec<ProjectMilestone> =
            delta_payloads(&db, USER_B, entity_types::PROJECT_MILESTONE).await;

        assert_eq!(
            payloads.len(),
            2,
            "one milestone create plus one milestone update"
        );
        assert_eq!(payloads[0], created);
        assert!(
            !payloads[0].created_at.is_empty(),
            "the payload is built from the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );
        assert_eq!(payloads[1], updated);
        assert_eq!(
            payloads[1].name, "Beta 2",
            "the update row must carry the new value, not the row as it was before"
        );
        assert_eq!(
            payloads[1].target_date.as_deref(),
            Some("2026-10-15"),
            "a re-dated milestone has to survive a reconnect re-dated"
        );
    }

    /// The durable half: what a client that was offline for all of it gets on
    /// reconnect. Run with no `ws_manager` at all, so nothing here can be
    /// satisfied by the live frame.
    #[tokio::test]
    async fn delta_carries_a_payload_for_every_status_and_project_write() {
        let db = two_user_workspace().await;

        let status = create_test_status(&db, None).await;
        let project = create_test_project(&db, None).await;
        crate::project_service::add_project_member(
            &db,
            &project.project_id,
            USER_B,
            "member",
            WS,
            None,
        )
        .await
        .expect("add member");
        crate::project_service::remove_project_member(&db, &project.project_id, USER_B, WS, None)
            .await
            .expect("remove member");
        crate::project_service::create_project_update(
            &db,
            &project.project_id,
            USER_A,
            "on_track",
            Some("Shipping this week"),
            None,
            WS,
        )
        .await
        .expect("post a project update");

        let entries = get_entries_since(&db, WS, USER_B, 0, 10_000)
            .await
            .expect("B's delta");

        let mut statuses = 0;
        let mut projects = 0;
        for entry in &entries {
            let entity_type = entry.entity_type.as_str();
            if entity_type != entity_types::STATUS && entity_type != entity_types::PROJECT {
                continue;
            }
            assert!(
                matches!(
                    entry.action,
                    SyncActionType::Insert | SyncActionType::Update
                ),
                "unexpected action in this delta: {entry:?}"
            );

            let data = entry.data.clone().unwrap_or_else(|| {
                panic!(
                    "delta row {} has no payload — the client skips insert/update rows \
                     without one, so the change never arrives on reconnect either: {entry:?}",
                    entry.sync_id
                )
            });

            if entity_type == entity_types::STATUS {
                let received: Status = serde_json::from_value(data)
                    .expect("status delta row deserializes into a Status");
                assert_eq!(received, status);
                statuses += 1;
            } else {
                let received: Project = serde_json::from_value(data)
                    .expect("project delta row deserializes into a Project");
                assert_eq!(received.project_id, project.project_id);
                projects += 1;
            }
        }

        assert_eq!(statuses, 1, "one status create");
        assert_eq!(
            projects, 1,
            "just the project create. The member add, the member remove and the \
             posted update each write to a satellite table and leave `projects` \
             byte-identical, so since TRA-9940 they report themselves rather \
             than claiming the parent project changed — see \
             `delta_carries_a_payload_for_every_member_and_posted_update_write`"
        );
    }
    // ─── Delta payloads for the remaining services (TRA-9939) ────────────────
    //
    // Every test below runs with **no `ws_manager` at all**. That is the point:
    // these paths already broadcast a full payload on the live frame, so a test
    // holding a connection would pass while the stored `sync_log` row stayed
    // empty. Reading the delta back is the only way to prove what a client that
    // missed the broadcast actually receives on reconnect.

    /// Every Insert/Update entry of `entity_type` in `user_id`'s delta-from-zero
    /// stream, deserialized into the model type the client uses for it.
    ///
    /// Panics on the first row with no payload: the client skips a data-less
    /// insert or update outright (`cache/apply.rs:47-53`), so such a row is a
    /// change that never arrives on reconnect. Deserializing rather than just
    /// checking for presence is what catches a payload of the wrong shape — a
    /// bare `Issue` where the client expects an `IssueWithDetails`, say.
    async fn delta_payloads<T: serde::de::DeserializeOwned>(
        db: &DbPool,
        user_id: &str,
        entity_type: &str,
    ) -> Vec<T> {
        get_entries_since(db, WS, user_id, 0, 10_000)
            .await
            .expect("delta entries")
            .into_iter()
            .filter(|e| e.entity_type == entity_type)
            .filter(|e| !matches!(e.action, SyncActionType::Delete))
            .map(|entry| {
                let data = entry.data.clone().unwrap_or_else(|| {
                    panic!(
                        "delta row {} ({} {:?}) has no payload — the client skips \
                         insert/update rows without one, so the change never \
                         arrives on reconnect either: {entry:?}",
                        entry.sync_id, entry.entity_type, entry.action
                    )
                });
                serde_json::from_value(data).unwrap_or_else(|e| {
                    panic!(
                        "delta row {} does not deserialize into the model the \
                         client applies for {}: {e} — {entry:?}",
                        entry.sync_id, entry.entity_type
                    )
                })
            })
            .collect()
    }

    /// `create_issue` and `delete_team` both resolve a workspace-scoped backlog
    /// status; the fixture's only status is team-scoped.
    async fn add_workspace_backlog_status(db: &DbPool) {
        db_execute!(
            db,
            "INSERT INTO statuses (status_id, workspace_id, team_id, name, category, position) \
             VALUES ($1, $2, NULL, $3, $4, $5)",
            "sts_ws_backlog",
            WS,
            "Backlog",
            "backlog",
            0_i32
        )
        .expect("insert workspace-scoped backlog status");
    }

    #[tokio::test]
    async fn delta_carries_a_payload_for_every_label_write() {
        let db = two_user_workspace().await;

        let created = crate::label_service::create_label(
            &db, WS, "Bug", "#DC2626", Some("team_vis"), None,
        )
        .await
        .expect("create label");
        let updated =
            crate::label_service::update_label(&db, &created.label_id, "Defect", "#B91C1C", None)
                .await
                .expect("update label");

        let payloads: Vec<Label> = delta_payloads(&db, USER_B, entity_types::LABEL).await;

        assert_eq!(payloads.len(), 2, "one label create plus one label update");
        assert_eq!(payloads[0], created);
        assert!(
            !payloads[0].created_at.is_empty(),
            "the payload is built from the re-fetch, so the DB-assigned \
             created_at has to be in it"
        );
        assert_eq!(payloads[1], updated);
        assert_eq!(
            payloads[1].name, "Defect",
            "the update row must carry the new value, not the row as it was before"
        );
    }

    #[tokio::test]
    async fn delta_carries_a_payload_for_every_view_write() {
        let db = two_user_workspace().await;

        let created = crate::view_service::create_view(
            &db,
            &crate::view_service::CreateViewParams {
                workspace_id: WS,
                user_id: USER_A,
                name: "My work",
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared: true,
                team_id: Some("team_vis"),
                position: 3,
            },
            None,
        )
        .await
        .expect("create view");

        let updated = crate::view_service::update_view(
            &db,
            &crate::view_service::UpdateViewParams {
                view_id: &created.view_id,
                name: Some("Everyone's work"),
                icon: None,
                filters: None,
                display_options: None,
                is_shared: None,
                sort_order: None,
                team_id: None,
                position: None,
            },
            None,
        )
        .await
        .expect("update view");

        let payloads: Vec<View> = delta_payloads(&db, USER_A, entity_types::VIEW).await;

        assert_eq!(payloads.len(), 2, "one view create plus one view update");
        assert_eq!(payloads[0], created);
        assert!(
            !payloads[0].created_at.is_empty() && !payloads[0].updated_at.is_empty(),
            "the DB-assigned timestamps have to be in the payload"
        );
        assert_eq!(payloads[1], updated);
        assert_eq!(payloads[1].name, "Everyone's work");
    }

    #[tokio::test]
    async fn delta_carries_a_payload_for_every_favorite_write() {
        let db = two_user_workspace().await;

        let favorite =
            crate::favorite_service::add_favorite(&db, USER_A, WS, "issue", "iss_vis", None)
                .await
                .expect("A favorites the issue");

        // A favorite is scoped to its owner, so it is A's delta that carries it.
        let payloads: Vec<Favorite> = delta_payloads(&db, USER_A, entity_types::FAVORITE).await;

        assert_eq!(payloads.len(), 1, "one favorite add");
        assert_eq!(payloads[0], favorite);
        assert_eq!(payloads[0].user_id, USER_A);
    }

    #[tokio::test]
    async fn delta_carries_a_payload_for_every_issue_write() {
        let db = two_user_workspace().await;
        add_workspace_backlog_status(&db).await;

        let label = crate::label_service::create_label(
            &db, WS, "Bug", "#DC2626", Some("team_vis"), None,
        )
        .await
        .expect("create label");

        let created = crate::issue_service::create_issue(
            &db,
            &trakkt_types::models::CreateIssueParams {
                workspace_id: WS.to_string(),
                team_id: "team_vis".to_string(),
                creator_id: USER_A.to_string(),
                title: "Sync me".to_string(),
                description: None,
                priority: 2,
                assignee_id: None,
                due_date: None,
                label_ids: Vec::new(),
                project_id: None,
                milestone_id: None,
                estimate: None,
            },
            None,
        )
        .await
        .expect("create issue");

        crate::issue_service::update_issue(
            &db,
            WS,
            "VIS",
            created.number,
            &trakkt_types::models::IssueUpdate {
                title: Some("Sync me properly".to_string()),
                ..Default::default()
            },
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("update issue");

        crate::issue_service::set_issue_labels(
            &db,
            &created.issue_id,
            std::slice::from_ref(&label.label_id),
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("set issue labels");

        crate::issue_service::set_sort_order(&db, WS, "VIS", created.number, 12.5, None)
            .await
            .expect("set sort order");

        let payloads: Vec<IssueWithDetails> =
            delta_payloads(&db, USER_B, entity_types::ISSUE).await;

        assert_eq!(
            payloads.len(),
            4,
            "one issue create plus three updates: title, labels and sort order"
        );
        for payload in &payloads {
            assert_eq!(payload.issue_id, created.issue_id);
            assert_eq!(
                payload.team_key, "VIS",
                "the client deserializes an IssueWithDetails, so the joined \
                 team_key has to be in every payload"
            );
            assert!(!payload.created_at.is_empty());
        }
        assert_eq!(payloads[0].title, "Sync me");
        assert_eq!(
            payloads[1].title, "Sync me properly",
            "the update row must carry the new title"
        );
        assert_eq!(
            payloads[2].labels,
            vec![label],
            "the relabelling only reaches a client through this payload"
        );
        assert_eq!(
            payloads[3].sort_order,
            Some(12.5),
            "the new sort order only reaches a client through this payload"
        );
    }

    #[tokio::test]
    async fn delta_carries_a_payload_for_every_release_write() {
        let db = two_user_workspace().await;

        let release = crate::release_service::create_release(
            &db,
            WS,
            "VIS",
            "v1.0.0",
            None,
            Some("First cut"),
            None,
            &["iss_vis".to_string()],
            USER_A,
            None,
        )
        .await
        .expect("create release");
        assert_eq!(release.tag_name, "v1.0.0");

        let payloads: Vec<IssueWithDetails> =
            delta_payloads(&db, USER_B, entity_types::ISSUE).await;

        assert_eq!(payloads.len(), 1, "one issue stamped with released_at");
        assert_eq!(payloads[0].issue_id, "iss_vis");
        assert!(
            payloads[0].released_at.is_some(),
            "the payload is read back after the stamp, so it has to carry the \
             released_at the release just wrote"
        );
    }

    #[tokio::test]
    async fn delta_carries_a_payload_for_every_team_write() {
        let db = two_user_workspace().await;
        add_workspace_backlog_status(&db).await;

        let team = crate::team_service::create_team(
            &db,
            &crate::team_service::CreateTeamParams {
                workspace_id: WS,
                name: "Syncing",
                key: "SYNC",
                description: None,
                icon: None,
                creator_id: Some(USER_A),
            },
            None,
        )
        .await
        .expect("create team");

        let renamed = crate::team_service::update_team(
            &db,
            &team.team_id,
            WS,
            Some("Syncing Well".to_string()),
            None,
            None,
        )
        .await
        .expect("update team");

        crate::team_service::update_team_icon(
            &db,
            &team.team_id,
            WS,
            Some("preset"),
            Some("rocket"),
            Some("#0D9488"),
            None,
        )
        .await
        .expect("update team icon");
        crate::team_service::upload_team_icon(&db, &team.team_id, WS, b"png-bytes", "image/png", None)
            .await
            .expect("upload team icon");
        crate::team_service::delete_team_icon(&db, &team.team_id, WS, None)
            .await
            .expect("delete team icon");

        crate::team_service::add_team_member(&db, &team.team_id, USER_B, "member", WS)
            .await
            .expect("add team member");
        crate::team_service::update_team_member_role(&db, &team.team_id, USER_B, "lead", WS)
            .await
            .expect("update team member role");
        crate::team_service::remove_team_member(&db, &team.team_id, USER_B, WS)
            .await
            .expect("remove team member");

        let payloads: Vec<Team> = delta_payloads(&db, USER_B, entity_types::TEAM).await;

        assert_eq!(
            payloads.len(),
            9,
            "a create writes both an Insert and the creator's member-add Update, \
             then one Update each for rename, icon set, icon upload, icon clear, \
             member add, member role change and member remove"
        );
        for payload in &payloads {
            assert_eq!(payload.team_id, team.team_id);
            assert!(
                !payload.created_at.is_empty(),
                "the payload is built from the re-fetch, so the DB-assigned \
                 created_at has to be in it"
            );
        }
        assert_eq!(payloads[0], team, "the Insert carries the team as created");
        assert_eq!(payloads[2], renamed);
        assert_eq!(
            payloads[2].name, "Syncing Well",
            "the rename row must carry the new name"
        );
        assert_eq!(payloads[3].icon_name.as_deref(), Some("rocket"));
        assert_eq!(payloads[4].icon_type.as_deref(), Some("custom"));
        assert_eq!(
            payloads[5].icon_type, None,
            "clearing the icon must be visible in the payload"
        );
    }

    /// `delete_team` reassigns the deleted team's issues, and reports each one
    /// as an ISSUE update. The reassignment changes the issue's team, number and
    /// status, none of which reaches a client without a payload.
    #[tokio::test]
    async fn delta_carries_a_payload_for_issues_moved_by_a_team_delete() {
        let db = two_user_workspace().await;
        add_workspace_backlog_status(&db).await;

        let doomed = crate::team_service::create_team(
            &db,
            &crate::team_service::CreateTeamParams {
                workspace_id: WS,
                name: "Doomed",
                key: "DOOM",
                description: None,
                icon: None,
                creator_id: Some(USER_A),
            },
            None,
        )
        .await
        .expect("create team");

        let issue = crate::issue_service::create_issue(
            &db,
            &trakkt_types::models::CreateIssueParams {
                workspace_id: WS.to_string(),
                team_id: doomed.team_id.clone(),
                creator_id: USER_A.to_string(),
                title: "Moves teams".to_string(),
                description: None,
                priority: 2,
                assignee_id: None,
                due_date: None,
                label_ids: Vec::new(),
                project_id: None,
                milestone_id: None,
                estimate: None,
            },
            None,
        )
        .await
        .expect("create issue on the doomed team");

        crate::team_service::delete_team(&db, &doomed.team_id, WS, Some("team_vis"), None, None)
            .await
            .expect("delete team, reassigning its issues");

        let payloads: Vec<IssueWithDetails> =
            delta_payloads(&db, USER_B, entity_types::ISSUE).await;

        assert_eq!(
            payloads.len(),
            2,
            "the issue's own create, then the update reporting its reassignment"
        );
        assert_eq!(payloads[0].team_key, "DOOM");
        assert_eq!(payloads[1].issue_id, issue.issue_id);
        assert_eq!(
            payloads[1].team_key, "VIS",
            "the reassignment row must carry the issue's new team, which is the \
             whole change being reported"
        );
    }

    // ─── Atomic mutation + sync entry (TRA-9923) ─────────────────────────────

    /// Reject every `sync_log` INSERT, at the database.
    ///
    /// `RAISE(ABORT)` fails the statement and backs out its changes while
    /// leaving the surrounding transaction open and usable — the exact shape of
    /// a sync entry write that fails after the mutation statements have already
    /// run. The service code is untouched and knows nothing about it: the
    /// failure arrives as an ordinary sqlx error from a real schema object.
    async fn reject_sync_log_inserts(db: &DbPool) {
        db_execute!(
            db,
            "CREATE TRIGGER reject_sync_log BEFORE INSERT ON sync_log \
             BEGIN SELECT RAISE(ABORT, 'sync_log insert rejected'); END"
        )
        .expect("install sync_log rejection trigger");
    }

    /// Reject `sync_log` INSERTs for one entity id only.
    ///
    /// Same real trigger as [`reject_sync_log_inserts`], narrowed by a `WHEN`
    /// clause. A service that writes several sync entries in one transaction
    /// would otherwise always fail on the first one; scoping the rejection is
    /// what puts the failure on a later write instead. `entity_id` is a test
    /// constant — `CREATE TRIGGER` bodies cannot take bind parameters.
    async fn reject_sync_log_inserts_for_entity(db: &DbPool, entity_id: &str) {
        let sql = format!(
            "CREATE TRIGGER reject_sync_log_for_entity BEFORE INSERT ON sync_log \
             WHEN NEW.entity_id = '{entity_id}' \
             BEGIN SELECT RAISE(ABORT, 'sync_log insert rejected'); END"
        );
        db_execute!(db, &sql).expect("install scoped sync_log rejection trigger");
    }

    async fn count_scalar(db: &DbPool, sql: &str, bind: &str) -> i64 {
        db_fetch_scalar!(db, i64, sql, bind).expect("count query")
    }

    /// The label ids currently attached to an issue, in a stable order.
    async fn issue_label_ids(db: &DbPool, issue_id: &str) -> Vec<String> {
        #[derive(sqlx::FromRow)]
        struct LabelIdRow {
            label_id: String,
        }

        let rows: Vec<LabelIdRow> = db_fetch_all!(
            db,
            LabelIdRow,
            "SELECT label_id FROM issue_labels WHERE issue_id = $1 ORDER BY label_id",
            issue_id
        )
        .expect("read issue labels back");
        rows.into_iter().map(|r| r.label_id).collect()
    }

    #[tokio::test]
    async fn issue_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        add_workspace_backlog_status(&db).await;
        reject_sync_log_inserts(&db).await;

        let err = crate::issue_service::create_issue(
            &db,
            &trakkt_types::models::CreateIssueParams {
                workspace_id: WS.to_string(),
                team_id: "team_vis".to_string(),
                creator_id: USER_A.to_string(),
                title: "Never happened".to_string(),
                description: None,
                priority: 2,
                assignee_id: None,
                due_date: None,
                label_ids: Vec::new(),
                project_id: None,
                milestone_id: None,
                estimate: None,
            },
            None,
        )
        .await
        .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            count_scalar(
                &db,
                "SELECT COUNT(*) FROM issues WHERE title = $1",
                "Never happened"
            )
            .await,
            0,
            "an issue with no sync_log row is invisible to every future delta, \
             so it must not survive the failed write"
        );
    }

    #[tokio::test]
    async fn comment_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err = crate::comment_service::create_comment(
            &db,
            "iss_vis",
            USER_A,
            "Never happened",
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect_err("a comment whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            count_scalar(
                &db,
                "SELECT COUNT(*) FROM comments WHERE body = $1",
                "Never happened"
            )
            .await,
            0,
            "a comment with no sync_log row never reaches another client, so it \
             must not survive the failed write"
        );
    }

    #[tokio::test]
    async fn issue_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err = crate::issue_service::update_issue(
            &db,
            WS,
            "VIS",
            1,
            &trakkt_types::models::IssueUpdate {
                title: Some("Renamed in a doomed transaction".to_string()),
                ..Default::default()
            },
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect_err("an update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        let title: String =
            db_fetch_scalar!(&db, String, "SELECT title FROM issues WHERE issue_id = $1", "iss_vis")
                .expect("read issue title back");
        assert_eq!(
            title, "A leaky issue",
            "the UPDATE must be rolled back, not left committed with no sync row"
        );
    }

    #[tokio::test]
    async fn issue_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err = crate::issue_service::delete_issue(&db, WS, "VIS", 1, None)
            .await
            .expect_err("a delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            count_scalar(
                &db,
                "SELECT COUNT(*) FROM issues WHERE issue_id = $1",
                "iss_vis"
            )
            .await,
            1,
            "a delete with no sync_log row leaves every other client showing the \
             issue forever, so the DELETE must be rolled back"
        );
    }

    #[tokio::test]
    async fn issue_labels_roll_back_when_their_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        let bug = crate::label_service::create_label(
            &db, WS, "Bug", "#DC2626", Some("team_vis"), None,
        )
        .await
        .expect("create label");
        let regression = crate::label_service::create_label(
            &db, WS, "Regression", "#B91C1C", Some("team_vis"), None,
        )
        .await
        .expect("create second label");

        // The prior state the rollback has to restore.
        crate::issue_service::set_issue_labels(
            &db,
            "iss_vis",
            std::slice::from_ref(&bug.label_id),
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("set the issue's initial labels");

        reject_sync_log_inserts(&db).await;

        let err = crate::issue_service::set_issue_labels(
            &db,
            "iss_vis",
            std::slice::from_ref(&regression.label_id),
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect_err("a relabelling whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            issue_label_ids(&db, "iss_vis").await,
            vec![bug.label_id.clone()],
            "set_issue_labels deletes before it inserts — a failed sync entry \
             must roll both back, not leave the issue relabelled with no sync \
             row to report it"
        );
    }

    #[tokio::test]
    async fn issue_sort_order_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        crate::issue_service::set_sort_order(&db, WS, "VIS", 1, 5.0, None)
            .await
            .expect("set the issue's initial sort order");

        reject_sync_log_inserts(&db).await;

        let err = crate::issue_service::set_sort_order(&db, WS, "VIS", 1, 99.5, None)
            .await
            .expect_err("a reorder whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        let sort_order: f64 = db_fetch_scalar!(
            &db,
            f64,
            "SELECT sort_order FROM issues WHERE issue_id = $1",
            "iss_vis"
        )
        .expect("read sort_order back");
        assert!(
            (sort_order - 5.0).abs() < f64::EPSILON,
            "the reorder must be rolled back — a board position no client is \
             ever told about is worse than no reorder at all; got {sort_order}"
        );
    }

    #[tokio::test]
    async fn comment_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        let comment = crate::comment_service::create_comment(
            &db,
            "iss_vis",
            USER_A,
            "The original body",
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("create the comment being edited");

        reject_sync_log_inserts(&db).await;

        let err = crate::comment_service::update_comment(
            &db,
            &comment.comment_id,
            USER_A,
            "An edit nobody will ever see",
            None,
        )
        .await
        .expect_err("an edit whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        let body: String = db_fetch_scalar!(
            &db,
            String,
            "SELECT body FROM comments WHERE comment_id = $1",
            &comment.comment_id
        )
        .expect("read the comment body back");
        assert_eq!(
            body, "The original body",
            "the edit must be rolled back, not left committed with no sync row \
             to carry it to anyone else"
        );
    }

    #[tokio::test]
    async fn comment_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        let comment = crate::comment_service::create_comment(
            &db,
            "iss_vis",
            USER_A,
            "The comment that survives",
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("create the comment being deleted");

        reject_sync_log_inserts(&db).await;

        let err =
            crate::comment_service::delete_comment(&db, &comment.comment_id, USER_A, None)
                .await
                .expect_err("a delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            count_scalar(
                &db,
                "SELECT COUNT(*) FROM comments WHERE comment_id = $1",
                &comment.comment_id
            )
            .await,
            1,
            "a delete with no sync_log row leaves the comment on every other \
             client forever, so the DELETE must be rolled back"
        );
        let body: String = db_fetch_scalar!(
            &db,
            String,
            "SELECT body FROM comments WHERE comment_id = $1",
            &comment.comment_id
        )
        .expect("read the comment body back");
        assert_eq!(body, "The comment that survives");
    }

    /// The blocked-issue loop inside `update_issue` writes its own sync entries,
    /// one per issue whose derived `is_blocked` flag moves with this status
    /// change. Those writes are reached only when `status_id` changes *and* a
    /// blocking relation exists, so a title-only update never exercises them.
    ///
    /// The rejection is scoped to the blocked issue's id so the update's own
    /// entry succeeds first and the loop's entry is the one that fails — the
    /// failure has to unwind a transaction that already contains a good sync
    /// row, which is the case a per-write `unwrap_or(0)` would silently commit.
    #[tokio::test]
    async fn issue_status_change_rolls_back_when_a_blocked_issues_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        // Moving the blocker into this status is what clears the blocked flag.
        db_execute!(
            &db,
            "INSERT INTO statuses (status_id, workspace_id, team_id, name, category) \
             VALUES ($1, $2, $3, $4, $5)",
            "sts_done",
            WS,
            "team_vis",
            "Done",
            "completed"
        )
        .expect("insert completed status");

        db_execute!(
            &db,
            "INSERT INTO issues \
                (issue_id, workspace_id, team_id, number, title, creator_id, status_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            "iss_blocked",
            WS,
            "team_vis",
            2_i32,
            "Waiting on the leaky issue",
            USER_A,
            "sts_vis"
        )
        .expect("insert the blocked issue");

        crate::relation_service::create_relation(
            &db,
            WS,
            "iss_vis",
            "iss_blocked",
            "blocks",
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect("iss_vis blocks iss_blocked");

        let blocked_before = crate::issue_service::get_issue_by_id(&db, "iss_blocked")
            .await
            .expect("read the blocked issue")
            .expect("the blocked issue exists");
        assert!(
            blocked_before.is_blocked,
            "precondition: the blocker is not completed, so the issue is blocked"
        );

        reject_sync_log_inserts_for_entity(&db, "iss_blocked").await;

        let err = crate::issue_service::update_issue(
            &db,
            WS,
            "VIS",
            1,
            &trakkt_types::models::IssueUpdate {
                status_id: Some("sts_done".to_string()),
                ..Default::default()
            },
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect_err(
            "an update whose blocked-issue sync entry cannot be written must fail",
        );

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        let status_id: String = db_fetch_scalar!(
            &db,
            String,
            "SELECT status_id FROM issues WHERE issue_id = $1",
            "iss_vis"
        )
        .expect("read the issue's status back");
        assert_eq!(
            status_id, "sts_vis",
            "the status change must be rolled back — a blocked issue that never \
             hears about it stays blocked on every other client"
        );

        let blocked_after = crate::issue_service::get_issue_by_id(&db, "iss_blocked")
            .await
            .expect("read the blocked issue")
            .expect("the blocked issue exists");
        assert!(
            blocked_after.is_blocked,
            "the blocked issue's derived state is computed from the blocker's \
             status, so a committed status change would silently unblock it \
             with no sync row to report either issue"
        );

        assert_eq!(
            count_scalar(
                &db,
                "SELECT COUNT(*) FROM sync_log WHERE entity_id = $1",
                "iss_vis"
            )
            .await,
            0,
            "the update's own sync entry succeeded before the loop failed, so it \
             is proof of the rollback: it must not be left committed on its own"
        );
    }

    #[tokio::test]
    async fn sync_id_returned_in_the_transaction_addresses_the_committed_row() {
        let db = two_user_workspace().await;

        let mut tx = db.begin().await.expect("begin transaction");
        let first = write_sync_entry_in_tx(
            &mut tx,
            entity_types::ISSUE,
            "iss_first",
            WS,
            None,
            SyncActionType::Insert,
            None,
        )
        .await
        .expect("first entry");
        let second = write_sync_entry_in_tx(
            &mut tx,
            entity_types::ISSUE,
            "iss_second",
            WS,
            None,
            SyncActionType::Update,
            None,
        )
        .await
        .expect("second entry");
        tx.commit().await.expect("commit transaction");

        assert!(first > 0 && second > 0, "0 is never a real sync_log id");
        assert_ne!(
            first, second,
            "each entry in the transaction must get its own id"
        );

        // The point of the check: the id handed back inside the transaction is
        // the id of the row that actually landed, not of some other insert.
        for (sync_id, expected_entity) in [(first, "iss_first"), (second, "iss_second")] {
            let entity_id: String = db_fetch_scalar!(
                &db,
                String,
                "SELECT entity_id FROM sync_log WHERE sync_id = $1",
                sync_id
            )
            .expect("committed sync_log row");
            assert_eq!(
                entity_id, expected_entity,
                "sync_id {sync_id} must address the row it was returned for"
            );
        }
    }

    #[tokio::test]
    async fn a_rolled_back_transaction_leaves_no_sync_entry() {
        let db = two_user_workspace().await;

        let mut tx = db.begin().await.expect("begin transaction");
        let sync_id = write_sync_entry_in_tx(
            &mut tx,
            entity_types::ISSUE,
            "iss_discarded",
            WS,
            None,
            SyncActionType::Insert,
            None,
        )
        .await
        .expect("entry written inside the transaction");
        tx.rollback().await.expect("roll back transaction");

        assert!(
            !is_sync_id_available(&db, WS, sync_id)
                .await
                .expect("check availability"),
            "an entry from a rolled-back transaction must not be readable"
        );
        assert_eq!(
            count_scalar(
                &db,
                "SELECT COUNT(*) FROM sync_log WHERE entity_id = $1",
                "iss_discarded"
            )
            .await,
            0,
            "the row must be gone entirely"
        );
    }

    #[tokio::test]
    async fn issue_update_broadcasts_the_sync_id_it_committed() {
        let db = two_user_workspace().await;
        let manager = WebSocketManager::new(None, db.clone());

        let mut conn = manager.connect(USER_B).expect("connection");
        // Discard the connect heartbeat.
        conn.rx.recv().await.expect("heartbeat frame");

        crate::issue_service::update_issue(
            &db,
            WS,
            "VIS",
            1,
            &trakkt_types::models::IssueUpdate {
                title: Some("Renamed".to_string()),
                ..Default::default()
            },
            Some(USER_A),
            trakkt_types::enums::ActionSource::User,
            None,
            Some(&manager),
        )
        .await
        .expect("update issue");

        let frame = conn.rx.recv().await.expect("broadcast frame");
        let action = match serde_json::from_str::<SyncResponse>(&frame)
            .expect("broadcast frame is a SyncResponse")
        {
            SyncResponse::SyncAction(action) => action,
            other => panic!("expected a sync_action frame, got {other:?}"),
        };

        assert_eq!(action.entity_id, "iss_vis");
        assert_ne!(
            action.sync_id, 0,
            "the live frame must carry the real sync_log id — 0 was the \
             warn-and-continue substitute this change removed"
        );

        let committed: i64 = db_fetch_scalar!(
            &db,
            i64,
            "SELECT MAX(sync_id) FROM sync_log WHERE entity_id = $1 AND action = $2",
            "iss_vis",
            "update"
        )
        .expect("committed sync_log row");
        assert_eq!(
            action.sync_id, committed,
            "the broadcast id has to be the id of the row that was committed, \
             or a client that misses the frame cannot spot the gap"
        );

        let payload: IssueWithDetails = serde_json::from_value(
            action.data.expect("the frame carries the issue"),
        )
        .expect("frame payload deserializes as an issue");
        assert_eq!(
            payload.title, "Renamed",
            "the broadcast must report the committed state"
        );
    }

    // ─── Atomic team mutation + sync entry (TRA-9947) ────────────────────────

    /// Reject `sync_log` INSERTs carrying one action only.
    ///
    /// Same real trigger as [`reject_sync_log_inserts`], narrowed by action
    /// rather than by entity. `create_team` writes both its entries against the
    /// same `entity_id` — the team's — so an entity-scoped rejection cannot tell
    /// them apart; the action can, which is what puts the failure on the second
    /// write while the first one succeeds.
    async fn reject_sync_log_inserts_for_action(db: &DbPool, action: &str) {
        let sql = format!(
            "CREATE TRIGGER reject_sync_log_for_action BEFORE INSERT ON sync_log \
             WHEN NEW.action = '{action}' \
             BEGIN SELECT RAISE(ABORT, 'sync_log insert rejected'); END"
        );
        db_execute!(db, &sql).expect("install action-scoped sync_log rejection trigger");
    }

    /// Every team in the database, in a stable order.
    async fn team_ids(db: &DbPool) -> Vec<String> {
        #[derive(sqlx::FromRow)]
        struct Row {
            team_id: String,
        }
        let rows: Vec<Row> =
            db_fetch_all!(db, Row, "SELECT team_id FROM teams ORDER BY team_id").expect("read teams");
        rows.into_iter().map(|r| r.team_id).collect()
    }

    /// Every team membership, in a stable order.
    async fn team_memberships(db: &DbPool) -> Vec<(String, String, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            team_id: String,
            user_id: String,
            role: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT team_id, user_id, role FROM team_members ORDER BY team_id, user_id"
        )
        .expect("read team members");
        rows.into_iter()
            .map(|r| (r.team_id, r.user_id, r.role))
            .collect()
    }

    /// Where every issue sits: the three columns a team delete reassigns.
    async fn issue_placements(db: &DbPool) -> Vec<(String, String, i64, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            issue_id: String,
            team_id: String,
            number: i64,
            status_id: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT issue_id, team_id, number, status_id FROM issues ORDER BY issue_id"
        )
        .expect("read issue placements");
        rows.into_iter()
            .map(|r| (r.issue_id, r.team_id, r.number, r.status_id))
            .collect()
    }

    /// Every favorite, in a stable order.
    async fn favorites(db: &DbPool) -> Vec<(String, String, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            favorite_id: String,
            target_type: String,
            target_id: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT favorite_id, target_type, target_id FROM favorites ORDER BY favorite_id"
        )
        .expect("read favorites");
        rows.into_iter()
            .map(|r| (r.favorite_id, r.target_type, r.target_id))
            .collect()
    }

    /// Every status and label id, in a stable order — the two tables the team
    /// delete reaches only through `ON DELETE CASCADE`.
    async fn cascade_child_ids(db: &DbPool) -> (Vec<String>, Vec<String>) {
        #[derive(sqlx::FromRow)]
        struct IdRow {
            id: String,
        }
        let statuses: Vec<IdRow> = db_fetch_all!(
            db,
            IdRow,
            "SELECT status_id AS id FROM statuses ORDER BY status_id"
        )
        .expect("read statuses");
        let labels: Vec<IdRow> =
            db_fetch_all!(db, IdRow, "SELECT label_id AS id FROM labels ORDER BY label_id")
                .expect("read labels");
        (
            statuses.into_iter().map(|r| r.id).collect(),
            labels.into_iter().map(|r| r.id).collect(),
        )
    }

    /// Every user's `default_team_id`, in a stable order.
    async fn user_default_teams(db: &DbPool) -> Vec<(String, Option<String>)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            user_id: String,
            default_team_id: Option<String>,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT user_id, default_team_id FROM users ORDER BY user_id"
        )
        .expect("read user default teams");
        rows.into_iter()
            .map(|r| (r.user_id, r.default_team_id))
            .collect()
    }

    /// A team's icon columns — including `icon_data` and `icon_mime`, which no
    /// `Team` read carries, so nothing else would notice an upload surviving.
    #[derive(sqlx::FromRow, Debug, PartialEq)]
    struct IconState {
        icon_type: Option<String>,
        icon_name: Option<String>,
        icon_color: Option<String>,
        icon_data: Option<Vec<u8>>,
        icon_mime: Option<String>,
    }

    async fn icon_state(db: &DbPool, team_id: &str) -> IconState {
        trakkt_core::db_fetch_optional!(
            db,
            IconState,
            "SELECT icon_type, icon_name, icon_color, icon_data, icon_mime \
             FROM teams WHERE team_id = $1",
            team_id
        )
        .expect("read team icon columns")
        .expect("the team exists")
    }

    async fn team_settings_json(db: &DbPool, team_id: &str) -> Option<String> {
        #[derive(sqlx::FromRow)]
        struct Row {
            settings: Option<String>,
        }
        trakkt_core::db_fetch_optional!(
            db,
            Row,
            "SELECT CAST(settings AS TEXT) AS settings FROM teams WHERE team_id = $1",
            team_id
        )
        .expect("read team settings")
        .expect("the team exists")
        .settings
    }

    async fn sync_log_row_count(db: &DbPool) -> i64 {
        db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM sync_log").expect("count sync_log rows")
    }

    /// Created with no creator, deliberately: that is what leaves the team's own
    /// Insert entry as the only sync write in the transaction. With a creator
    /// there is a second entry behind it, and this test would pass on that one
    /// failing even if the Insert entry's own failure were being swallowed.
    #[tokio::test]
    async fn team_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let teams_before = team_ids(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::create_team(
            &db,
            &crate::team_service::CreateTeamParams {
                workspace_id: WS,
                name: "Never happened",
                key: "NEVR",
                description: None,
                icon: None,
                creator_id: None,
            },
            None,
        )
        .await
        .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            team_ids(&db).await,
            teams_before,
            "a team with no sync_log row is invisible to every future delta, so \
             it must not survive the failed write"
        );
    }

    /// The member-add entry is `create_team`'s *second* sync write. Rejecting it
    /// alone leaves a transaction that already holds a good Insert row, two
    /// inserts, and has to unwind all of it — the case a per-write
    /// `unwrap_or(0)` would commit silently.
    #[tokio::test]
    async fn team_create_rolls_back_when_the_member_add_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let teams_before = team_ids(&db).await;
        let members_before = team_memberships(&db).await;

        reject_sync_log_inserts_for_action(&db, "update").await;

        let err = crate::team_service::create_team(
            &db,
            &crate::team_service::CreateTeamParams {
                workspace_id: WS,
                name: "Never happened",
                key: "NEVR",
                description: None,
                icon: None,
                creator_id: Some(USER_A),
            },
            None,
        )
        .await
        .expect_err("a create whose member-add sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(team_ids(&db).await, teams_before, "the team must not survive");
        assert_eq!(
            team_memberships(&db).await,
            members_before,
            "the creator's membership must not survive"
        );
        assert_eq!(
            sync_log_row_count(&db).await,
            0,
            "the team's own Insert entry was written and accepted before the \
             member-add entry failed, so it is the proof of the rollback: it \
             must not be left committed on its own"
        );
    }

    #[tokio::test]
    async fn team_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let before = crate::team_service::get_team(&db, "team_vis")
            .await
            .expect("read the team")
            .expect("the fixture team exists");

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::update_team(
            &db,
            "team_vis",
            WS,
            Some("Renamed in a doomed transaction".to_string()),
            Some("DOOM".to_string()),
            None,
        )
        .await
        .expect_err("an update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            crate::team_service::get_team(&db, "team_vis")
                .await
                .expect("read the team back")
                .expect("the team still exists"),
            before,
            "the rename must be rolled back, not left committed with no sync row"
        );
    }

    #[tokio::test]
    async fn team_icon_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let before = icon_state(&db, "team_vis").await;

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::update_team_icon(
            &db,
            "team_vis",
            WS,
            Some("preset"),
            Some("rocket"),
            Some("#0D9488"),
            None,
        )
        .await
        .expect_err("an icon change whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            icon_state(&db, "team_vis").await,
            before,
            "the icon change must be rolled back — an icon no client is ever \
             told about is worse than no icon change at all"
        );
    }

    #[tokio::test]
    async fn team_icon_upload_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let before = icon_state(&db, "team_vis").await;

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::upload_team_icon(
            &db,
            "team_vis",
            WS,
            b"png-bytes-nobody-will-see",
            "image/png",
            None,
        )
        .await
        .expect_err("an upload whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            icon_state(&db, "team_vis").await,
            before,
            "the uploaded bytes must be rolled back with everything else — \
             `icon_data` is in no sync payload, so a survivor here is invisible \
             to every client until it re-reads the team"
        );
    }

    #[tokio::test]
    async fn team_icon_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        crate::team_service::upload_team_icon(
            &db,
            "team_vis",
            WS,
            b"png-bytes-that-survive",
            "image/png",
            None,
        )
        .await
        .expect("upload the icon being cleared");
        let before = icon_state(&db, "team_vis").await;
        assert_eq!(
            before.icon_type.as_deref(),
            Some("custom"),
            "precondition: there is an icon to clear"
        );

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::delete_team_icon(&db, "team_vis", WS, None)
            .await
            .expect_err("an icon clear whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            icon_state(&db, "team_vis").await,
            before,
            "clearing the icon must be rolled back, leaving the uploaded icon \
             exactly as it was"
        );
    }

    /// A second team, doomed, carrying one of everything `delete_team` touches:
    /// an issue to reassign, a favorite, a user default, a membership, and the
    /// team-scoped status and label that only `ON DELETE CASCADE` removes.
    ///
    /// The ids are fixed because `CREATE TRIGGER` bodies cannot take bind
    /// parameters — the rejection has to name its target as a literal.
    async fn seed_doomed_team(db: &DbPool) {
        add_workspace_backlog_status(db).await;

        db_execute!(
            db,
            "INSERT INTO teams (team_id, workspace_id, name, key) VALUES ($1, $2, $3, $4)",
            "team_doomed",
            WS,
            "Doomed",
            "DOOM"
        )
        .expect("insert the doomed team");

        db_execute!(
            db,
            "INSERT INTO statuses (status_id, workspace_id, team_id, name, category) \
             VALUES ($1, $2, $3, $4, $5)",
            "sts_doomed",
            WS,
            "team_doomed",
            "Doomed Backlog",
            "backlog"
        )
        .expect("insert the doomed team's own status");

        db_execute!(
            db,
            "INSERT INTO labels (label_id, workspace_id, team_id, name, color) \
             VALUES ($1, $2, $3, $4, $5)",
            "lbl_doomed",
            WS,
            "team_doomed",
            "Doomed",
            "#DC2626"
        )
        .expect("insert the doomed team's own label");

        db_execute!(
            db,
            "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3)",
            "team_doomed",
            USER_B,
            "lead"
        )
        .expect("insert the doomed team's membership");

        db_execute!(
            db,
            "INSERT INTO issues \
                (issue_id, workspace_id, team_id, number, title, creator_id, status_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            "iss_doomed",
            WS,
            "team_doomed",
            1_i32,
            "Gets reassigned",
            USER_A,
            "sts_doomed"
        )
        .expect("insert the issue that has to be reassigned");

        db_execute!(
            db,
            "INSERT INTO favorites (favorite_id, user_id, workspace_id, target_type, target_id) \
             VALUES ($1, $2, $3, $4, $5)",
            "fav_doomed",
            USER_B,
            WS,
            "team",
            "team_doomed"
        )
        .expect("insert the favorite pointing at the doomed team");

        db_execute!(
            db,
            "UPDATE users SET default_team_id = $1 WHERE user_id = $2",
            "team_doomed",
            USER_B
        )
        .expect("point a user's default at the doomed team");
    }

    /// Everything `delete_team` writes, read straight from the tables: the
    /// issues it reassigns, the favorites and user defaults it clears, the teams
    /// it deletes, and the memberships, statuses and labels that go with the
    /// team through `ON DELETE CASCADE`.
    ///
    /// Every field is an ordered `Vec` of the rows themselves — a count would
    /// pass just as happily on rows that came back changed.
    #[derive(Debug, PartialEq)]
    struct TeamDeleteFootprint {
        issues: Vec<(String, String, i64, String)>,
        favorites: Vec<(String, String, String)>,
        user_defaults: Vec<(String, Option<String>)>,
        teams: Vec<String>,
        memberships: Vec<(String, String, String)>,
        cascaded_statuses: Vec<String>,
        cascaded_labels: Vec<String>,
    }

    async fn team_delete_footprint(db: &DbPool) -> TeamDeleteFootprint {
        let (cascaded_statuses, cascaded_labels) = cascade_child_ids(db).await;
        TeamDeleteFootprint {
            issues: issue_placements(db).await,
            favorites: favorites(db).await,
            user_defaults: user_default_teams(db).await,
            teams: team_ids(db).await,
            memberships: team_memberships(db).await,
            cascaded_statuses,
            cascaded_labels,
        }
    }

    /// The team delete's own sync entry is the last write in the transaction —
    /// after the reassignments, after the favorites and user-default clears,
    /// after the DELETE and its cascade. Rejecting it proves the whole cascade
    /// unwinds, including the reassignment entries that were already accepted.
    #[tokio::test]
    async fn team_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_doomed_team(&db).await;
        let before = team_delete_footprint(&db).await;

        reject_sync_log_inserts_for_entity(&db, "team_doomed").await;

        let err = crate::team_service::delete_team(&db, "team_doomed", WS, Some("team_vis"), None, None)
            .await
            .expect_err("a delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        let after = team_delete_footprint(&db).await;
        assert_eq!(
            after.issues, before.issues,
            "the reassigned issue must be back on its own team, with its \
             original number and status"
        );
        assert_eq!(
            after.favorites, before.favorites,
            "the deleted favorite must be back"
        );
        assert_eq!(
            after.user_defaults, before.user_defaults,
            "the cleared default_team_id must be back"
        );
        assert_eq!(
            after.teams, before.teams,
            "a team delete with no sync_log row leaves the team on every other \
             client forever, so the DELETE must be rolled back"
        );
        assert_eq!(
            after.memberships, before.memberships,
            "the membership the DELETE cascaded away must be back"
        );
        assert_eq!(
            (after.cascaded_statuses, after.cascaded_labels),
            (before.cascaded_statuses, before.cascaded_labels),
            "the team-scoped status and label the DELETE cascaded away must be \
             back — the rollback has to unwind the schema's cascade too"
        );

        assert_eq!(
            sync_log_row_count(&db).await,
            0,
            "the reassigned issue's sync entry was written and accepted before \
             the team entry failed, so it is the proof of the rollback: it must \
             not be left committed on its own"
        );
    }

    /// The reassignment loop's entry is the *first* sync write in the same
    /// transaction. Rejecting it instead proves the loop is inside the
    /// transaction at all — with the entry written on the pool, the `UPDATE
    /// issues` that precedes it commits regardless.
    #[tokio::test]
    async fn team_delete_rolls_back_when_a_reassigned_issues_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_doomed_team(&db).await;
        let before = team_delete_footprint(&db).await;

        reject_sync_log_inserts_for_entity(&db, "iss_doomed").await;

        let err = crate::team_service::delete_team(&db, "team_doomed", WS, Some("team_vis"), None, None)
            .await
            .expect_err("a delete whose reassignment sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        let after = team_delete_footprint(&db).await;
        assert_eq!(
            after.issues, before.issues,
            "an issue moved to another team with no sync row to report it keeps \
             its old team on every other client, and its number is now taken — \
             so the UPDATE must be rolled back"
        );
        assert_eq!(
            after.teams, before.teams,
            "the team must survive with its issue"
        );
        assert_eq!(
            after, before,
            "nothing else the delete touches may be left half-applied"
        );
    }

    #[tokio::test]
    async fn team_member_add_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let before = team_memberships(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::add_team_member(&db, "team_vis", USER_B, "member", WS)
            .await
            .expect_err("a member add whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            team_memberships(&db).await,
            before,
            "`team_members` is not a synced entity type, so no later delta can \
             repair a membership that committed without its sync row — the \
             INSERT must be rolled back"
        );
    }

    #[tokio::test]
    async fn team_member_remove_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        crate::team_service::add_team_member(&db, "team_vis", USER_B, "member", WS)
            .await
            .expect("add the member being removed");
        let before = team_memberships(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::remove_team_member(&db, "team_vis", USER_B, WS)
            .await
            .expect_err("a member remove whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            team_memberships(&db).await,
            before,
            "a removal with no sync row leaves the member on every other client \
             forever, so the DELETE must be rolled back"
        );
    }

    #[tokio::test]
    async fn team_member_role_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        crate::team_service::add_team_member(&db, "team_vis", USER_B, "member", WS)
            .await
            .expect("add the member being promoted");
        let before = team_memberships(&db).await;

        reject_sync_log_inserts(&db).await;

        let err =
            crate::team_service::update_team_member_role(&db, "team_vis", USER_B, "lead", WS)
                .await
                .expect_err("a role change whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            team_memberships(&db).await,
            before,
            "the promotion must be rolled back, not left committed with no sync \
             row to carry it to anyone else"
        );
    }

    #[tokio::test]
    async fn team_settings_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        crate::team_service::update_team_settings(
            &db,
            "team_vis",
            WS,
            &trakkt_types::models::TeamSettings {
                auto_archive_days: Some(7),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("write the settings the rollback has to restore");
        let before = team_settings_json(&db, "team_vis").await;
        assert!(
            before.as_deref().is_some_and(|s| s.contains("\"auto_archive_days\":7")),
            "precondition: the settings the rollback restores are really there; \
             got {before:?}"
        );

        reject_sync_log_inserts(&db).await;

        let err = crate::team_service::update_team_settings(
            &db,
            "team_vis",
            WS,
            &trakkt_types::models::TeamSettings {
                auto_archive_days: Some(30),
                ..Default::default()
            },
            None,
        )
        .await
        .expect_err("a settings change whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            team_settings_json(&db, "team_vis").await,
            before,
            "the settings change must be rolled back — auto-archiving that no \
             client is ever told about silently changes what the team sees"
        );
    }

    // ─── Atomic project mutation + sync entry (TRA-9948) ─────────────────────

    /// A project in `WS` carrying one of everything `delete_project` reaches:
    /// a member, a milestone, a posted update, and the fixture issue pointing at
    /// both the project and the milestone.
    ///
    /// Written with plain INSERTs rather than through the services, so the
    /// seeding itself writes no `sync_log` row — every test below installs its
    /// rejection trigger afterwards and the only write left to reject is the one
    /// under test. The ids are fixed for the same reason `seed_doomed_team`
    /// fixes its own: a narrowed `CREATE TRIGGER` cannot take bind parameters.
    async fn seed_project(db: &DbPool) {
        db_execute!(
            db,
            "INSERT INTO projects (project_id, workspace_id, name, description, status, lead_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            "prj_doomed",
            WS,
            "Doomed",
            "The project every rollback below has to restore",
            "planned",
            USER_A
        )
        .expect("insert the project");

        db_execute!(
            db,
            "INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, $3)",
            "prj_doomed",
            USER_B,
            "member"
        )
        .expect("insert the project membership");

        db_execute!(
            db,
            "INSERT INTO project_milestones \
                (milestone_id, project_id, name, description, target_date, sort_order) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            "mst_doomed",
            "prj_doomed",
            "Doomed Milestone",
            "Has a description a rename would overwrite",
            "2026-12-01",
            3_i32
        )
        .expect("insert the milestone");

        db_execute!(
            db,
            "INSERT INTO project_updates (update_id, project_id, user_id, health, body) \
             VALUES ($1, $2, $3, $4, $5)",
            "upd_doomed",
            "prj_doomed",
            USER_A,
            "on_track",
            "The only posted update"
        )
        .expect("insert the posted update");

        db_execute!(
            db,
            "UPDATE issues SET project_id = $1, milestone_id = $2 WHERE issue_id = $3",
            "prj_doomed",
            "mst_doomed",
            "iss_vis"
        )
        .expect("point the fixture issue at the project and milestone");
    }

    /// Every project, in a stable order.
    async fn project_ids(db: &DbPool) -> Vec<String> {
        #[derive(sqlx::FromRow)]
        struct Row {
            project_id: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT project_id FROM projects ORDER BY project_id"
        )
        .expect("read projects");
        rows.into_iter().map(|r| r.project_id).collect()
    }

    /// Every project membership, in a stable order.
    async fn project_memberships(db: &DbPool) -> Vec<(String, String, Option<String>)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            project_id: String,
            user_id: String,
            role: Option<String>,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT project_id, user_id, role FROM project_members \
             ORDER BY project_id, user_id"
        )
        .expect("read project members");
        rows.into_iter()
            .map(|r| (r.project_id, r.user_id, r.role))
            .collect()
    }

    /// Every milestone, in a stable order, with the columns an update rewrites.
    async fn milestones(
        db: &DbPool,
    ) -> Vec<(String, String, String, Option<String>, Option<String>)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            milestone_id: String,
            project_id: String,
            name: String,
            description: Option<String>,
            target_date: Option<String>,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT milestone_id, project_id, name, description, \
                    CAST(target_date AS TEXT) AS target_date \
             FROM project_milestones ORDER BY milestone_id"
        )
        .expect("read milestones");
        rows.into_iter()
            .map(|r| {
                (
                    r.milestone_id,
                    r.project_id,
                    r.name,
                    r.description,
                    r.target_date,
                )
            })
            .collect()
    }

    /// Every posted project update, in a stable order.
    async fn posted_project_updates(db: &DbPool) -> Vec<(String, String, String, Option<String>)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            update_id: String,
            project_id: String,
            health: String,
            body: Option<String>,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT update_id, project_id, health, body FROM project_updates \
             ORDER BY update_id"
        )
        .expect("read posted project updates");
        rows.into_iter()
            .map(|r| (r.update_id, r.project_id, r.health, r.body))
            .collect()
    }

    /// What every issue points at — the two columns a project or milestone
    /// delete clears through `ON DELETE SET NULL`.
    async fn issue_project_links(db: &DbPool) -> Vec<(String, Option<String>, Option<String>)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            issue_id: String,
            project_id: Option<String>,
            milestone_id: Option<String>,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT issue_id, project_id, milestone_id FROM issues ORDER BY issue_id"
        )
        .expect("read issue project links");
        rows.into_iter()
            .map(|r| (r.issue_id, r.project_id, r.milestone_id))
            .collect()
    }

    /// Everything `delete_project` removes: the project itself, and the members,
    /// milestones and posted updates the schema cascades with it, plus the issue
    /// links it clears through `ON DELETE SET NULL`.
    ///
    /// Every field is an ordered `Vec` of the rows themselves — a count would
    /// pass just as happily on rows that came back changed.
    #[derive(Debug, PartialEq)]
    struct ProjectDeleteFootprint {
        projects: Vec<String>,
        members: Vec<(String, String, Option<String>)>,
        milestones: Vec<(String, String, String, Option<String>, Option<String>)>,
        posted_updates: Vec<(String, String, String, Option<String>)>,
        issue_links: Vec<(String, Option<String>, Option<String>)>,
    }

    async fn project_delete_footprint(db: &DbPool) -> ProjectDeleteFootprint {
        ProjectDeleteFootprint {
            projects: project_ids(db).await,
            members: project_memberships(db).await,
            milestones: milestones(db).await,
            posted_updates: posted_project_updates(db).await,
            issue_links: issue_project_links(db).await,
        }
    }

    #[tokio::test]
    async fn project_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let before = project_ids(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::create_project(
            &db,
            &crate::project_service::CreateProjectParams {
                workspace_id: WS,
                name: "Never happened",
                description: None,
                icon: None,
                color: None,
                lead_id: None,
                start_date: None,
                target_date: None,
            },
            None,
        )
        .await
        .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            project_ids(&db).await,
            before,
            "a project with no sync_log row is invisible to every future delta, \
             so it must not survive the failed write"
        );
    }

    #[tokio::test]
    async fn project_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = crate::project_service::get_project(&db, "prj_doomed")
            .await
            .expect("read the project")
            .expect("the seeded project exists");

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::update_project(
            &db,
            &crate::project_service::UpdateProjectParams {
                project_id: "prj_doomed",
                name: Some("Renamed in a doomed transaction"),
                description: None,
                icon: None,
                color: None,
                status: Some("completed"),
                lead_id: Some(None),
                start_date: None,
                target_date: Some(Some("2027-01-01")),
                archived_at: None,
            },
            None,
        )
        .await
        .expect_err("an update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            crate::project_service::get_project(&db, "prj_doomed")
                .await
                .expect("read the project back")
                .expect("the project still exists"),
            before,
            "every column the UPDATE touched — the rename, the status, the \
             cleared lead and the new target date — must be rolled back, not \
             left committed with no sync row"
        );
    }

    /// The delete's sync entry is written after the DELETE and everything the
    /// schema cascades from it. Rejecting it proves the whole cascade unwinds,
    /// not just the `projects` row.
    #[tokio::test]
    async fn project_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = project_delete_footprint(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::delete_project(&db, "prj_doomed", None)
            .await
            .expect_err("a delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        let after = project_delete_footprint(&db).await;
        assert_eq!(
            after.projects, before.projects,
            "a project delete with no sync_log row leaves the project on every \
             other client forever, and no later delta can repair it — the row it \
             would have to re-read is gone. The DELETE must be rolled back"
        );
        assert_eq!(
            after.members, before.members,
            "the membership the DELETE cascaded away must be back"
        );
        assert_eq!(
            after.milestones, before.milestones,
            "the milestone the DELETE cascaded away must be back"
        );
        assert_eq!(
            after.posted_updates, before.posted_updates,
            "the posted update the DELETE cascaded away must be back"
        );
        assert_eq!(
            after.issue_links, before.issue_links,
            "the issue's project and milestone links must be back — the rollback \
             has to unwind the schema's ON DELETE SET NULL too"
        );
    }

    #[tokio::test]
    async fn project_member_add_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = project_memberships(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::add_project_member(
            &db,
            "prj_doomed",
            USER_A,
            "admin",
            WS,
            None,
        )
        .await
        .expect_err("a member add whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            project_memberships(&db).await,
            before,
            "`project_members` is not an entity type a delta re-reads, so no \
             later delta can repair a membership that committed without its sync \
             row — the INSERT must be rolled back"
        );
    }

    #[tokio::test]
    async fn project_member_remove_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = project_memberships(&db).await;
        assert!(
            before.contains(&(
                "prj_doomed".to_string(),
                USER_B.to_string(),
                Some("member".to_string())
            )),
            "precondition: the membership the remove has to fail on is really \
             there; got {before:?}"
        );

        reject_sync_log_inserts(&db).await;

        let err =
            crate::project_service::remove_project_member(&db, "prj_doomed", USER_B, WS, None)
                .await
                .expect_err("a member remove whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            project_memberships(&db).await,
            before,
            "a removal with no sync row leaves the member on every other client \
             forever, so the DELETE must be rolled back"
        );
    }

    #[tokio::test]
    async fn milestone_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = milestones(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::create_milestone(
            &db,
            "prj_doomed",
            "Never happened",
            Some("A milestone no client would ever hear about"),
            Some("2027-06-30"),
            None,
            WS,
        )
        .await
        .expect_err("a milestone create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            milestones(&db).await,
            before,
            "a milestone with no sync_log row is invisible to every future \
             delta, so it must not survive the failed write"
        );
    }

    #[tokio::test]
    async fn milestone_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = milestones(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::update_milestone(
            &db,
            "mst_doomed",
            Some("Renamed in a doomed transaction"),
            Some("Rewritten description"),
            Some(None),
            None,
            WS,
        )
        .await
        .expect_err("a milestone update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            milestones(&db).await,
            before,
            "the rename, the rewritten description and the cleared target date \
             must all be rolled back, not left committed with no sync row"
        );
    }

    #[tokio::test]
    async fn milestone_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let milestones_before = milestones(&db).await;
        let links_before = issue_project_links(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::delete_milestone(&db, "mst_doomed", None, WS)
            .await
            .expect_err("a milestone delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            milestones(&db).await,
            milestones_before,
            "a milestone delete with no sync row leaves it on every other client \
             forever, so the DELETE must be rolled back"
        );
        assert_eq!(
            issue_project_links(&db).await,
            links_before,
            "the issue's milestone link must be back — the rollback has to \
             unwind the schema's ON DELETE SET NULL too"
        );
    }

    #[tokio::test]
    async fn posted_project_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_project(&db).await;
        let before = posted_project_updates(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::project_service::create_project_update(
            &db,
            "prj_doomed",
            USER_A,
            "at_risk",
            Some("Nobody would ever see this"),
            None,
            WS,
        )
        .await
        .expect_err("a posted update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "got: {err}"
        );

        assert_eq!(
            posted_project_updates(&db).await,
            before,
            "`project_updates` is not an entity type a delta re-reads, so a \
             posted update that committed without its sync row would never reach \
             another client — the INSERT must be rolled back"
        );
    }

    // ─── Atomic attachment mutation + sync entry (TRA-9949) ──────────────────

    /// Two attachments in `WS`, one already linked to the fixture issue and one
    /// loose.
    ///
    /// Both are needed, and for opposite reasons. `detach_from_issue` returns
    /// `NotFound` before it ever reaches its sync entry unless the link is
    /// already there, and `attach_to_issue` inserts `ON CONFLICT DO NOTHING`, so
    /// pointing it at an already-linked attachment would make its INSERT a
    /// no-op and leave the rollback assertion with nothing to prove. `att_linked`
    /// covers the first case, `att_loose` the second.
    ///
    /// Written with plain INSERTs rather than through `attachment_service`, so
    /// the seeding itself writes no `sync_log` row — every test below installs
    /// its rejection trigger afterwards and the only write left to reject is the
    /// one under test. The ids are fixed for the same reason `seed_doomed_team`
    /// and `seed_project` fix theirs: a narrowed `CREATE TRIGGER` cannot take
    /// bind parameters.
    ///
    /// This is a service-layer fixture, so it says nothing about object storage.
    /// `storage_path` here is just the column the service carries; no test in
    /// this module can observe whether a blob exists, because the service never
    /// touches one — see `crates/trakkt-api/src/attachments.rs` for where the
    /// blob writes actually live.
    async fn seed_attachments(db: &DbPool) {
        for (attachment_id, filename) in [
            ("att_linked", "already-on-the-issue.png"),
            ("att_loose", "not-linked-yet.png"),
        ] {
            db_execute!(
                db,
                "INSERT INTO attachments \
                    (attachment_id, workspace_id, filename, content_type, size_bytes, \
                     storage_path, uploaded_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                attachment_id,
                WS,
                filename,
                "image/png",
                1234_i64,
                format!("{WS}/{attachment_id}.png"),
                USER_A
            )
            .expect("insert attachment");
        }

        db_execute!(
            db,
            "INSERT INTO issue_attachments (issue_id, attachment_id) VALUES ($1, $2)",
            "iss_vis",
            "att_linked"
        )
        .expect("link att_linked to the fixture issue");
    }

    /// Every attachment, in a stable order, with every column a rollback has to
    /// restore.
    async fn attachment_rows(db: &DbPool) -> Vec<(String, String, String, String, i64, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            attachment_id: String,
            workspace_id: String,
            filename: String,
            content_type: String,
            size_bytes: i64,
            uploaded_by: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT attachment_id, workspace_id, filename, content_type, size_bytes, \
                    uploaded_by \
             FROM attachments ORDER BY attachment_id"
        )
        .expect("read attachments");
        rows.into_iter()
            .map(|r| {
                (
                    r.attachment_id,
                    r.workspace_id,
                    r.filename,
                    r.content_type,
                    r.size_bytes,
                    r.uploaded_by,
                )
            })
            .collect()
    }

    /// Every issue/attachment link, in a stable order.
    async fn issue_attachment_links(db: &DbPool) -> Vec<(String, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            issue_id: String,
            attachment_id: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT issue_id, attachment_id FROM issue_attachments \
             ORDER BY issue_id, attachment_id"
        )
        .expect("read issue attachment links");
        rows.into_iter()
            .map(|r| (r.issue_id, r.attachment_id))
            .collect()
    }

    #[tokio::test]
    async fn attachment_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_attachments(&db).await;
        let before = attachment_rows(&db).await;

        reject_sync_log_inserts(&db).await;

        let err = crate::attachment_service::create_attachment(
            &db,
            WS,
            "never-happened.png",
            "image/png",
            4096,
            "ws_visibility/never-happened.png",
            USER_A,
            None,
        )
        .await
        .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            attachment_rows(&db).await,
            before,
            "an attachment row with no sync_log row is invisible to every future \
             delta, so it must not survive the failed write"
        );
    }

    /// `attachments.attachment_id` is the target of an `ON DELETE CASCADE` from
    /// `issue_attachments`, so rejecting the sync entry has to unwind the link
    /// the DELETE cascaded away as well as the attachment row itself.
    #[tokio::test]
    async fn attachment_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_attachments(&db).await;
        let attachments_before = attachment_rows(&db).await;
        let links_before = issue_attachment_links(&db).await;
        assert!(
            links_before.contains(&("iss_vis".to_string(), "att_linked".to_string())),
            "precondition: the link the DELETE has to cascade away is really \
             there, otherwise the cascade assertion below proves nothing; got \
             {links_before:?}"
        );

        reject_sync_log_inserts(&db).await;

        let err = crate::attachment_service::delete_attachment(
            &db,
            "att_linked",
            WS,
            USER_A,
            None,
        )
        .await
        .expect_err("a delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            attachment_rows(&db).await,
            attachments_before,
            "an attachment delete with no sync_log row leaves the attachment on \
             every other client forever, and no later delta can repair it — the \
             row it would have to re-read is gone. The DELETE must be rolled back"
        );
        assert_eq!(
            issue_attachment_links(&db).await,
            links_before,
            "the issue link the DELETE cascaded away must be back — the rollback \
             has to unwind the schema's ON DELETE CASCADE too"
        );
    }

    #[tokio::test]
    async fn issue_attach_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_attachments(&db).await;
        let before = issue_attachment_links(&db).await;
        assert!(
            !before.contains(&("iss_vis".to_string(), "att_loose".to_string())),
            "precondition: `att_loose` is not linked yet, so the INSERT under \
             test really inserts rather than falling into ON CONFLICT DO \
             NOTHING; got {before:?}"
        );

        reject_sync_log_inserts(&db).await;

        let err =
            crate::attachment_service::attach_to_issue(&db, WS, "iss_vis", "att_loose", None)
                .await
                .expect_err("an attach whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            issue_attachment_links(&db).await,
            before,
            "`issue_attachments` is not an entity type a delta re-reads, so no \
             later delta can repair a link that committed without its sync row — \
             the INSERT must be rolled back"
        );
    }

    #[tokio::test]
    async fn issue_detach_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        seed_attachments(&db).await;
        let before = issue_attachment_links(&db).await;
        assert!(
            before.contains(&("iss_vis".to_string(), "att_linked".to_string())),
            "precondition: the link the detach has to remove is really there, \
             otherwise the call returns NotFound before it ever reaches its sync \
             entry; got {before:?}"
        );

        reject_sync_log_inserts(&db).await;

        let err =
            crate::attachment_service::detach_from_issue(&db, WS, "iss_vis", "att_linked", None)
                .await
                .expect_err("a detach whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            issue_attachment_links(&db).await,
            before,
            "an unlink with no sync row leaves the attachment hanging off the \
             issue on every other client forever, so the DELETE must be rolled \
             back"
        );
    }

    // ─── Per-user services: atomicity and audience (TRA-9950) ────────────────
    //
    // Notifications, notification preferences, favorites and unshared views are
    // the entities whose sync rows carry a `visibility_user_id`. Two things have
    // to hold at once here, and the second is the one a shared commit-and-
    // broadcast helper quietly breaks:
    //
    //   1. the mutation and its `sync_log` row commit together, and
    //   2. both halves stay addressed to the owning user — the persisted row
    //      (`visibility_user_id`) and the live frame.
    //
    // Getting (2) wrong is the TRA-9920 leak, so it is asserted directly against
    // a real `WebSocketManager` rather than inferred from the diff.

    /// Every view, in a stable order.
    async fn views(db: &DbPool) -> Vec<(String, String, bool)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            view_id: String,
            name: String,
            is_shared: bool,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT view_id, name, is_shared FROM views ORDER BY view_id"
        )
        .expect("read views");
        rows.into_iter()
            .map(|r| (r.view_id, r.name, r.is_shared))
            .collect()
    }

    /// Every notification, in a stable order.
    async fn notifications(db: &DbPool) -> Vec<(String, String, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            notification_id: String,
            user_id: String,
            notification_type: String,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT notification_id, user_id, type AS notification_type \
             FROM notifications ORDER BY notification_id"
        )
        .expect("read notifications");
        rows.into_iter()
            .map(|r| (r.notification_id, r.user_id, r.notification_type))
            .collect()
    }

    /// Every notification preferences row, in a stable order.
    ///
    /// `notify_comments` is the field the update tests flip, so it is the one
    /// carried here.
    async fn preferences(db: &DbPool) -> Vec<(String, String, bool)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            preference_id: String,
            user_id: String,
            notify_comments: bool,
        }
        let rows: Vec<Row> = db_fetch_all!(
            db,
            Row,
            "SELECT preference_id, user_id, notify_comments \
             FROM notification_preferences ORDER BY preference_id"
        )
        .expect("read notification preferences");
        rows.into_iter()
            .map(|r| (r.preference_id, r.user_id, r.notify_comments))
            .collect()
    }

    /// Create a view for `owner`, sharing it or not.
    async fn make_view(
        db: &DbPool,
        owner: &str,
        name: &str,
        is_shared: bool,
        ws: Option<&WebSocketManager>,
    ) -> View {
        crate::view_service::create_view(
            db,
            &crate::view_service::CreateViewParams {
                workspace_id: WS,
                user_id: owner,
                name,
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared,
                team_id: None,
                position: 0,
            },
            ws,
        )
        .await
        .unwrap_or_else(|e| panic!("create view {name}: {e}"))
    }

    /// Give `user` a notification about the fixture issue.
    async fn notify(db: &DbPool, user: &str, ws: Option<&WebSocketManager>) {
        crate::notification_service::create_notification(
            db,
            WS,
            user,
            "iss_vis",
            TYPE_ASSIGNED,
            Some(USER_B),
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            ws,
        )
        .await
        .expect("create notification");
    }

    use crate::notification_service::TYPE_ASSIGNED;

    #[tokio::test]
    async fn favorite_add_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err = crate::favorite_service::add_favorite(&db, USER_A, WS, "issue", "iss_vis", None)
            .await
            .expect_err("an add whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            favorites(&db).await,
            Vec::new(),
            "a favorite with no sync_log row never reaches the user's other \
             browsers, so it must not survive the failed write"
        );
    }

    #[tokio::test]
    async fn favorite_remove_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let favorite =
            crate::favorite_service::add_favorite(&db, USER_A, WS, "issue", "iss_vis", None)
                .await
                .expect("A favorites the issue");

        let before = favorites(&db).await;
        assert_eq!(
            before,
            vec![(
                favorite.favorite_id.clone(),
                "issue".to_string(),
                "iss_vis".to_string()
            )],
            "the favorite has to exist for its removal to be rolled back"
        );

        reject_sync_log_inserts(&db).await;

        let err =
            crate::favorite_service::remove_favorite(&db, USER_A, WS, "issue", "iss_vis", None)
                .await
                .expect_err("a remove whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            favorites(&db).await,
            before,
            "an unpin with no sync row leaves the favorite in the sidebar of \
             every other browser forever, so the DELETE must be rolled back"
        );
    }

    #[tokio::test]
    async fn view_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err = crate::view_service::create_view(
            &db,
            &crate::view_service::CreateViewParams {
                workspace_id: WS,
                user_id: USER_A,
                name: "Never saved",
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared: false,
                team_id: None,
                position: 0,
            },
            None,
        )
        .await
        .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            views(&db).await,
            Vec::new(),
            "a view with no sync_log row is invisible to every future delta, so \
             it must not survive the failed write"
        );
    }

    #[tokio::test]
    async fn view_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let view = make_view(&db, USER_A, "Original name", false, None).await;

        let before = views(&db).await;
        assert_eq!(
            before,
            vec![(view.view_id.clone(), "Original name".to_string(), false)],
            "the view has to exist for its update to be rolled back"
        );

        reject_sync_log_inserts(&db).await;

        let err = crate::view_service::update_view(
            &db,
            &crate::view_service::UpdateViewParams {
                view_id: &view.view_id,
                name: Some("Renamed"),
                icon: None,
                filters: None,
                display_options: None,
                is_shared: Some(true),
                sort_order: None,
                team_id: None,
                position: None,
            },
            None,
        )
        .await
        .expect_err("an update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            views(&db).await,
            before,
            "an edit with no sync row leaves every other client showing the old \
             name and share state, so the UPDATE must be rolled back — including \
             the `is_shared` flip, which also decides who the row is addressed to"
        );
    }

    #[tokio::test]
    async fn view_delete_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        let view = make_view(&db, USER_A, "Keep me", false, None).await;

        let before = views(&db).await;
        assert_eq!(
            before,
            vec![(view.view_id.clone(), "Keep me".to_string(), false)],
            "the view has to exist for its deletion to be rolled back"
        );

        reject_sync_log_inserts(&db).await;

        let err = crate::view_service::delete_view(&db, &view.view_id, WS, None)
            .await
            .expect_err("a delete whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            views(&db).await,
            before,
            "a delete with no sync row leaves the view in every other sidebar \
             forever and no later delta can repair it, so the DELETE must be \
             rolled back"
        );
    }

    #[tokio::test]
    async fn notification_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err = crate::notification_service::create_notification(
            &db,
            WS,
            USER_A,
            "iss_vis",
            TYPE_ASSIGNED,
            Some(USER_B),
            None,
            trakkt_types::enums::ActionSource::User,
            None,
            None,
        )
        .await
        .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            notifications(&db).await,
            Vec::new(),
            "a notification with no sync_log row never reaches the recipient's \
             inbox live and no delta can replay it, so it must not survive the \
             failed write"
        );
    }

    #[tokio::test]
    async fn notification_preferences_create_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;
        reject_sync_log_inserts(&db).await;

        let err =
            crate::notification_service::get_or_default_preferences(&db, USER_A, WS, None)
                .await
                .expect_err("a create whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            preferences(&db).await,
            Vec::new(),
            "a preferences row with no sync_log row leaves the settings screen \
             on the user's other browsers stale, so it must not survive the \
             failed write"
        );
    }

    #[tokio::test]
    async fn notification_preferences_update_rolls_back_when_its_sync_entry_cannot_be_written() {
        let db = two_user_workspace().await;

        // Seed the row *before* installing the trigger. Without this,
        // `update_preference` writes two sync entries — the `Insert` from
        // `get_or_default_preferences` and then its own `Update` — and the
        // rejection would land on the first one, so the test would pass without
        // ever reaching the transaction it is meant to exercise.
        let seeded = crate::notification_service::get_or_default_preferences(&db, USER_A, WS, None)
            .await
            .expect("seed A's preferences");

        let before = preferences(&db).await;
        assert_eq!(
            before,
            vec![(seeded.preference_id.clone(), USER_A.to_string(), true)],
            "the preferences row has to exist, and `notify_comments` has to \
             start true, for the flip below to be observable"
        );

        reject_sync_log_inserts(&db).await;

        let err = crate::notification_service::update_preference(
            &db,
            USER_A,
            WS,
            "notify_comments",
            false,
            None,
        )
        .await
        .expect_err("an update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure, not a swallowed \
             warning; got: {err}"
        );

        assert_eq!(
            preferences(&db).await,
            before,
            "a preference change with no sync row leaves the user's other \
             browsers showing the old setting, so the UPDATE must be rolled back"
        );
    }

    // ─── Both halves of per-user visibility ──────────────────────────────────
    //
    // A per-user row has to stay private on *both* paths, and the two are easy
    // to get out of step: `visibility_user_id` governs the delta replay, the
    // delivery call governs the live frame. A conversion that routed these
    // through a workspace-wide commit helper would keep the first test passing
    // (the column would still be set) or the second (the frame would still be
    // addressed) depending on which half it got wrong — so both are asserted.

    /// Give A one of every per-user entity, plus a shared view as a control.
    /// Returns `(notification_id, favorite_id, preference_id, private_view_id,
    /// shared_view_id)`.
    async fn seed_every_per_user_entity(
        db: &DbPool,
    ) -> (String, String, String, String, String) {
        notify(db, USER_A, None).await;
        let notification_id = crate::notification_service::list_notifications(
            db, USER_A, false, false, None, None, None, 50, 0,
        )
        .await
        .expect("list A's notifications")
        .first()
        .expect("A has a notification")
        .notification_id
        .clone();

        let favorite =
            crate::favorite_service::add_favorite(db, USER_A, WS, "issue", "iss_vis", None)
                .await
                .expect("A favorites the issue");

        let prefs = crate::notification_service::get_or_default_preferences(db, USER_A, WS, None)
            .await
            .expect("A's preferences");

        let private = make_view(db, USER_A, "A's private view", false, None).await;
        let shared = make_view(db, USER_A, "A's shared view", true, None).await;

        (
            notification_id,
            favorite.favorite_id,
            prefs.preference_id,
            private.view_id,
            shared.view_id,
        )
    }

    #[tokio::test]
    async fn per_user_rows_never_reach_another_members_delta() {
        let db = two_user_workspace().await;
        let (notification, favorite, preference, private_view, shared_view) =
            seed_every_per_user_entity(&db).await;

        let b_entries = get_entries_since(&db, WS, USER_B, 0, 10_000)
            .await
            .expect("B's delta");
        let b_ids: Vec<String> = b_entries.iter().map(|e| e.entity_id.clone()).collect();

        for (label, id) in [
            ("notification", &notification),
            ("favorite", &favorite),
            ("notification preferences", &preference),
            ("private view", &private_view),
        ] {
            assert!(
                !b_ids.contains(id),
                "B's delta carried A's {label} ({id}); \
                 `visibility_user_id` must be Some(A) for it: {b_ids:?}"
            );
        }

        // Every per-user entity type, empty for B — B created none of their own.
        for entity_type in [
            entity_types::NOTIFICATION,
            entity_types::FAVORITE,
            entity_types::NOTIFICATION_PREFERENCES,
        ] {
            assert_eq!(
                delta_entity_ids(&db, USER_B, entity_type).await,
                Vec::<String>::new(),
                "B owns no {entity_type} of their own, so their delta must \
                 contain none"
            );
        }

        // The control: B does receive the shared view, so the filter is scoping
        // rows rather than simply withholding everything.
        assert_eq!(
            delta_entity_ids(&db, USER_B, entity_types::VIEW).await,
            vec![shared_view.clone()],
            "B must still receive the shared view — otherwise this test would \
             pass just as well with delta sync broken outright"
        );

        // And A still receives all of their own.
        let a_ids: Vec<String> = get_entries_since(&db, WS, USER_A, 0, 10_000)
            .await
            .expect("A's delta")
            .into_iter()
            .map(|e| e.entity_id)
            .collect();
        for (label, id) in [
            ("notification", &notification),
            ("favorite", &favorite),
            ("notification preferences", &preference),
            ("private view", &private_view),
        ] {
            assert!(
                a_ids.contains(id),
                "the scope must not over-restrict: A's own {label} ({id}) is \
                 missing from A's delta: {a_ids:?}"
            );
        }
    }

    #[tokio::test]
    async fn per_user_live_frames_reach_only_their_owner() {
        let db = two_user_workspace().await;
        let manager = WebSocketManager::new(None, db.clone());

        let mut a_conn = manager.connect(USER_A).expect("A connects");
        let mut b_conn = manager.connect(USER_B).expect("B connects");
        a_conn.rx.recv().await.expect("A's connect heartbeat");
        b_conn.rx.recv().await.expect("B's connect heartbeat");

        // One mutation per converted per-user site, in order.
        let favorite =
            crate::favorite_service::add_favorite(&db, USER_A, WS, "issue", "iss_vis", Some(&manager))
                .await
                .expect("A favorites the issue");
        notify(&db, USER_A, Some(&manager)).await;
        crate::notification_service::get_or_default_preferences(&db, USER_A, WS, Some(&manager))
            .await
            .expect("A's preferences are created");
        crate::notification_service::update_preference(
            &db,
            USER_A,
            WS,
            "notify_comments",
            false,
            Some(&manager),
        )
        .await
        .expect("A turns comment notifications off");
        let private = make_view(&db, USER_A, "A's private view", false, Some(&manager)).await;
        crate::favorite_service::remove_favorite(
            &db,
            USER_A,
            WS,
            "issue",
            "iss_vis",
            Some(&manager),
        )
        .await
        .expect("A unpins the issue");

        // A receives every one of them, in order, each addressed to A's own row.
        let mut a_frames = Vec::new();
        for _ in 0..6 {
            let action = next_sync_action(&mut a_conn).await;
            a_frames.push((action.entity_type.clone(), action.entity_id.clone()));
            assert!(
                action.sync_id > 0,
                "a per-user frame still has to carry the id of its committed \
                 row so a client that missed it can spot the gap: {action:?}"
            );
        }
        assert_eq!(
            a_frames,
            vec![
                (entity_types::FAVORITE.to_string(), favorite.favorite_id.clone()),
                (entity_types::NOTIFICATION.to_string(), a_frames[1].1.clone()),
                (
                    entity_types::NOTIFICATION_PREFERENCES.to_string(),
                    a_frames[2].1.clone()
                ),
                (
                    entity_types::NOTIFICATION_PREFERENCES.to_string(),
                    a_frames[3].1.clone()
                ),
                (entity_types::VIEW.to_string(), private.view_id.clone()),
                (entity_types::FAVORITE.to_string(), favorite.favorite_id.clone()),
            ],
            "the owner must receive a live frame for each of their own mutations"
        );

        // …and B receives none of them. This is the assertion a workspace-wide
        // commit helper breaks: the persisted rows would still be scoped, but
        // every frame above would have been pushed to B as well.
        assert!(
            b_conn.rx.try_recv().is_err(),
            "B received a live frame for one of A's per-user mutations — this is \
             the TRA-9920 leak on the socket"
        );

        // The control: a *shared* view does reach B over the same connection, so
        // the assertion above is about audience and not about delivery being
        // broken.
        let shared = make_view(&db, USER_A, "A's shared view", true, Some(&manager)).await;
        let b_frame = next_sync_action(&mut b_conn).await;
        assert_eq!(
            (b_frame.entity_type.as_str(), b_frame.entity_id.as_str()),
            (entity_types::VIEW, shared.view_id.as_str()),
            "a shared view must still broadcast to every member"
        );
    }
}
