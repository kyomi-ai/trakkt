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

use trakkt_core::sql_compat;
use trakkt_core::{db_execute, db_fetch_all, db_fetch_scalar, DbPool, MessageType, WebSocketMessage};
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

/// Insert a row into `sync_log` and return the assigned `sync_id`.
///
/// Uses `RETURNING sync_id` on Postgres and `SELECT last_insert_rowid()` on
/// SQLite because the ID is assigned by the database (BIGSERIAL / AUTOINCREMENT).
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
    let now_expr = sql_compat::now(is_pg);
    let action_str = action_type_to_str(&action);

    // Serialise the data payload.
    // Postgres: stored as JSONB — pass the JSON string with ::jsonb cast.
    // SQLite:   stored as TEXT — pass the JSON string directly.
    let data_str: Option<String> = data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| {
            trakkt_core::Error::Internal(format!("failed to serialise sync entry data: {e}"))
        })?;

    let sync_id: i64 = if is_pg {
        // Postgres: use RETURNING to get the assigned BIGSERIAL id.
        let json_cast = sql_compat::cast_to_json(is_pg, "$5");
        let sql = format!(
            r#"
            INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data, visibility_user_id, created_at)
            VALUES ($1, $2, $3, $4, {json_cast}, $6, {now_expr})
            RETURNING sync_id
            "#
        );
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
        // SQLite: INSERT then query last_insert_rowid().
        let sql = format!(
            r#"
            INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data, visibility_user_id, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, {now_expr})
            "#
        );
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

/// Broadcast a sync notification to all connected workspace members via WebSocket.
///
/// Broadcast a notification that an entity changed. Clients receiving this
/// should perform a delta sync to fetch the actual data.
///
/// Used by entity services that don't yet send full SyncResponse data
/// (team, comment, notification).
pub async fn broadcast_sync_notify(
    ws_manager: &WebSocketManager,
    entity_type: &str,
    workspace_id: &str,
) {
    let message = WebSocketMessage::new(MessageType::SyncAction).with_data(
        serde_json::json!({
            "entity_type": entity_type,
            "workspace_id": workspace_id,
        }),
    );

    ws_manager
        .broadcast_to_workspace(workspace_id, message, None)
        .await;
}

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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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
}
