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
use trakkt_core::{db_execute, db_fetch_all, db_fetch_scalar, DbPool};
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
    use trakkt_types::models::{
        Favorite, IssueWithDetails, Label, Project, Status, Team, View,
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

    #[tokio::test]
    async fn project_member_add_frame_carries_the_parent_project() {
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
        assert!(matches!(action.action, SyncActionType::Update));
        let data = payload_of(&action, entity_types::PROJECT, &project.project_id);

        // `project_members` is not a synced entity type, so the change is
        // reported as an update to the parent project and carries the project
        // row — which the membership change itself does not alter.
        let received: Project =
            serde_json::from_value(data).expect("payload deserializes into a Project");
        assert_eq!(received, project);
    }

    #[tokio::test]
    async fn project_member_remove_frame_carries_the_parent_project() {
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
        assert!(matches!(action.action, SyncActionType::Update));
        let data = payload_of(&action, entity_types::PROJECT, &project.project_id);

        let received: Project =
            serde_json::from_value(data).expect("payload deserializes into a Project");
        assert_eq!(received, project);
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
            projects, 4,
            "one project create plus three updates: member add, member remove, \
             and the posted project update"
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
}
