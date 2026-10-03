// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notification service — operations for the `notifications` table.
//!
//! Notifications inform users about issue events (assignment, mention,
//! status change, etc.). They are user-scoped and support read/unread
//! tracking.

use trakkt_core::db::DbTx;
use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;
use trakkt_types::models::{Notification, NotificationPreferences};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service::{self, SyncAudience, SyncBatch};
use crate::websocket::WebSocketManager;

pub const DEFAULT_NOTIFICATION_LIMIT: i64 = 50;

pub const TYPE_COMMENTED: &str = "commented";
pub const TYPE_STATUS_CHANGED: &str = "status_changed";
pub const TYPE_ASSIGNED: &str = "assigned";
pub const TYPE_PRIORITY_CHANGED: &str = "priority_changed";
pub const TYPE_LABEL_CHANGED: &str = "label_changed";
pub const TYPE_DUE_DATE_CHANGED: &str = "due_date_changed";
pub const TYPE_ESTIMATE_CHANGED: &str = "estimate_changed";
pub const TYPE_MILESTONE_CHANGED: &str = "milestone_changed";
pub const TYPE_PROJECT_CHANGED: &str = "project_changed";
pub const TYPE_TEAM_CHANGED: &str = "team_changed";
pub const TYPE_RELATION_ADDED: &str = "relation_added";

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising notification queries.
///
/// The `read` field is `bool` — sqlx handles the Postgres `boolean` and
/// SQLite `INTEGER` mapping transparently.
#[derive(sqlx::FromRow)]
struct NotificationRow {
    notification_id: String,
    workspace_id: String,
    user_id: String,
    issue_id: String,
    notification_type: String,
    read: bool,
    issue_title: Option<String>,
    issue_number: Option<i32>,
    team_key: Option<String>,
    actor_id: Option<String>,
    actor_name: Option<String>,
    action_source: String,
    action_source_label: Option<String>,
    created_at: String,
    deleted_at: Option<String>,
    context_id: Option<String>,
}

impl NotificationRow {
    fn into_dto(self) -> Notification {
        Notification {
            notification_id: self.notification_id,
            workspace_id: self.workspace_id,
            user_id: self.user_id,
            issue_id: self.issue_id,
            notification_type: self.notification_type,
            read: self.read,
            issue_title: self.issue_title,
            issue_number: self.issue_number,
            team_key: self.team_key,
            actor_id: self.actor_id,
            actor_name: self.actor_name,
            action_source: self.action_source
                .parse::<ActionSource>()
                .unwrap_or_else(|_| {
                    tracing::warn!(raw = %self.action_source, "Unknown action_source value; defaulting to User");
                    ActionSource::User
                }),
            action_source_label: self.action_source_label,
            created_at: self.created_at,
            deleted_at: self.deleted_at,
            context_id: self.context_id,
        }
    }
}

/// Base SELECT for every query that reads a whole notification.
///
/// Shared by the create path, the list path and the state-change paths below so
/// the column list cannot drift between them — the same role
/// [`PREFERENCES_SELECT`] plays for preferences. The three LEFT JOINs are what
/// fill in `issue_title`, `issue_number`, `team_key` and `actor_name`; a payload
/// built without them hands the client ids it has nothing to render.
///
/// Callers append their own `WHERE`, and the aliases (`n`, `i`, `t`, `u_actor`)
/// are part of the contract because they do.
const NOTIFICATION_SELECT: &str = "\
    SELECT n.notification_id, n.workspace_id, n.user_id, n.issue_id, \
           n.type AS notification_type, n.read, \
           i.title AS issue_title, i.number AS issue_number, \
           t.key AS team_key, \
           n.actor_id, \
           u_actor.name AS actor_name, \
           n.action_source, n.action_source_label, \
           CAST(n.created_at AS TEXT) AS created_at, \
           CAST(n.deleted_at AS TEXT) AS deleted_at, \
           n.context_id \
    FROM notifications n \
    LEFT JOIN issues i ON i.issue_id = n.issue_id \
    LEFT JOIN teams t ON t.team_id = i.team_id \
    LEFT JOIN users u_actor ON u_actor.user_id = n.actor_id";

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a notification for a user about an issue event.
///
/// The INSERT and its `sync_log` entry are one transaction: a notification that
/// commits without its sync row never reaches the recipient's inbox live, and no
/// later delta can replay it.
///
/// A notification belongs to its recipient alone — the payload carries the issue
/// title, the actor's name and the read state — so the entry and the live frame
/// are both scoped to `user_id` via [`SyncAudience::User`].
///
/// # Transaction nesting
///
/// This opens its own transaction, so no caller may hold one across the call.
/// All four call sites are clear: `comment_service::create_comment` (commits at
/// its own line before notifying), `issue_service::update_issue` and
/// `issue_service::set_issue_labels` (likewise), and
/// `relation_service::create_relation`, which opens no transaction at all.
pub async fn create_notification(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    issue_id: &str,
    notification_type: &str,
    actor_id: Option<&str>,
    context_id: Option<&str>,
    action_source: ActionSource,
    action_source_label: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let notification_id = uuid::Uuid::new_v4().to_string();
    let action_source_str = action_source.as_str();

    let sql = format!(
        "INSERT INTO notifications \
            (notification_id, workspace_id, user_id, issue_id, type, actor_id, context_id, action_source, action_source_label, read, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, {bf}, {now})",
        bf = sql_compat::bool_false(is_pg),
    );
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &notification_id,
        workspace_id,
        user_id,
        issue_id,
        notification_type,
        actor_id,
        context_id,
        action_source_str,
        action_source_label
    )?;

    // Read back on the transaction — the row does not exist outside it yet, and
    // the joins that fill in the issue title, team key and actor name are what
    // make the payload usable by the client.
    let notification: Option<NotificationRow> = trakkt_core::tx_fetch_optional!(
        &mut tx,
        NotificationRow,
        &format!("{NOTIFICATION_SELECT} WHERE n.notification_id = $1"),
        &notification_id
    )?;
    let notification_data = notification.map(|r| r.into_dto());
    let payload = notification_data.as_ref().and_then(|n| {
        sync_log_service::sync_payload(n, entity_types::NOTIFICATION, &notification_id)
    });

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::NOTIFICATION,
        &notification_id,
        workspace_id,
        SyncAudience::User(user_id),
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(())
}

/// List notifications for a user with optional filters.
///
/// Joins with `issues` to include issue title and number.
/// Results are ordered by creation date (newest first), limited to 50.
///
/// Optional filters narrow results by notification type, team key, or
/// text search (issue title / identifier). The nullable parameter pattern
/// (`$N IS NULL OR column = $N`) keeps bind positions fixed regardless
/// of which filters are active.
pub async fn list_notifications(
    db: &DbPool,
    user_id: &str,
    unread_only: bool,
    deleted_only: bool,
    notification_type: Option<&str>,
    team_key: Option<&str>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> trakkt_core::Result<Vec<Notification>> {
    let is_pg = db.is_postgres();
    let cast_text = if is_pg { "::TEXT" } else { "" };

    let unread_filter = if unread_only {
        let bf = sql_compat::bool_false(is_pg);
        format!("AND n.read = {bf}")
    } else {
        String::new()
    };

    let deleted_filter = if deleted_only {
        "AND n.deleted_at IS NOT NULL".to_string()
    } else {
        "AND n.deleted_at IS NULL".to_string()
    };

    // $1 = user_id, $2 = notification_type, $3 = team_key, $4 = search pattern
    let sql = format!(
        "{NOTIFICATION_SELECT} \
         WHERE n.user_id = $1 {deleted_filter} {unread_filter} \
           AND ($2{cast_text} IS NULL OR n.type = $2) \
           AND ($3{cast_text} IS NULL OR t.key = $3) \
           AND ($4{cast_text} IS NULL OR i.title LIKE $4 ESCAPE '\\' \
                OR (t.key || '-' || CAST(i.number AS TEXT)) LIKE $4 ESCAPE '\\') \
         ORDER BY n.created_at DESC \
         LIMIT {limit} OFFSET {offset}"
    );

    // Prepare the search term with wildcards, escaping LIKE special chars.
    let search_pattern = search.map(|s| {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{escaped}%")
    });

    let rows: Vec<NotificationRow> = trakkt_core::db_fetch_all!(
        db,
        NotificationRow,
        &sql,
        user_id,
        notification_type,
        team_key,
        search_pattern.as_deref()
    )?;
    Ok(rows.into_iter().map(NotificationRow::into_dto).collect())
}

/// Count notifications matching the given filters.
///
/// Uses the same WHERE conditions as `list_notifications` but returns
/// only the total count. Useful for pagination.
pub async fn count_notifications(
    db: &DbPool,
    user_id: &str,
    unread_only: bool,
    deleted_only: bool,
    notification_type: Option<&str>,
    team_key: Option<&str>,
    search: Option<&str>,
) -> trakkt_core::Result<i64> {
    let is_pg = db.is_postgres();
    let cast_text = if is_pg { "::TEXT" } else { "" };

    let unread_filter = if unread_only {
        let bf = sql_compat::bool_false(is_pg);
        format!("AND n.read = {bf}")
    } else {
        String::new()
    };

    let deleted_filter = if deleted_only {
        "AND n.deleted_at IS NOT NULL".to_string()
    } else {
        "AND n.deleted_at IS NULL".to_string()
    };

    // $1 = user_id, $2 = notification_type, $3 = team_key, $4 = search pattern
    let sql = format!(
        "SELECT COUNT(*) \
         FROM notifications n \
         LEFT JOIN issues i ON i.issue_id = n.issue_id \
         LEFT JOIN teams t ON t.team_id = i.team_id \
         WHERE n.user_id = $1 {deleted_filter} {unread_filter} \
           AND ($2{cast_text} IS NULL OR n.type = $2) \
           AND ($3{cast_text} IS NULL OR t.key = $3) \
           AND ($4{cast_text} IS NULL OR i.title LIKE $4 ESCAPE '\\' \
                OR (t.key || '-' || CAST(i.number AS TEXT)) LIKE $4 ESCAPE '\\')"
    );

    // Prepare the search term with wildcards, escaping LIKE special chars.
    let search_pattern = search.map(|s| {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{escaped}%")
    });

    let count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        &sql,
        user_id,
        notification_type,
        team_key,
        search_pattern.as_deref()
    )?;
    Ok(count)
}

// ─── Notification state changes ─────────────────────────────────────────────
//
// The six entry points below (`mark_as_read`, `mark_all_as_read` and the four
// `bulk_*` functions) all have the same shape: change some state on a subset of
// one user's notifications, and log one `sync_log` entry per row that subset
// names. They share the three helpers here.
//
// "Per row the subset names" and not "per row whose state moved": five of the
// six select on the very state they are about to change, so for them the two
// coincide. `mark_as_read` does not — its only predicate is
// `deleted_at IS NULL` — so re-marking an already-read notification read writes
// an entry for a row nothing moved. Asserted on both backends by
// `apps/server/tests/postgres_dialect.rs`.
//
// Granularity is one entry per notification rather than one entry describing
// the batch. An aggregate entry would carry only the request — "mark these read"
// — leaving every client to re-derive the server's WHERE clause locally against
// its own possibly-stale cache, and to diverge silently whenever it got a
// different answer. A row per row says what actually changed.

/// The ids of the notifications a change is about to touch, read inside the
/// transaction that will make it.
///
/// The five entry points that select on state pass a predicate the change
/// itself falsifies: `mark_all_as_read` and `bulk_mark_as_read` select on
/// `read = false` and then set `read = true`; `bulk_delete_notifications`
/// selects on `deleted_at IS NULL` and then stamps it. After the UPDATE those
/// rows no longer match, so the set cannot be recovered afterwards — it has to
/// be read first. That is what makes "one entry per row the predicate selected"
/// expressible; the alternative, one entry per row *requested*, announces
/// changes to rows the predicate excluded and leaves other clients applying
/// updates that never happened.
///
/// `mark_as_read` is the exception, and it does not weaken the rule. Its
/// predicate is only `deleted_at IS NULL`, which setting `read` leaves standing,
/// so its set would survive a read-back. It reads first anyway because it needs
/// the same `user_id` scoping and the same not-found-versus-not-owned
/// distinction as the other five, and a second shape here would be one more
/// thing to keep in step for no gain. The observable consequence is that marking
/// an already-read notification read writes an entry where the five
/// state-selecting callers write none —
/// `postgres_dialect.rs::notification_state_changes_log_one_recipient_scoped_update_each`
/// asserts it on both backends.
///
/// `RETURNING` would collapse the select and the update into one statement, and
/// is deliberately not used. The one place this workspace reaches for it —
/// `sync_log_service::sync_entry_insert_sql` — appends it on the Postgres arm
/// only and uses `last_insert_rowid()` on the SQLite arm, so a `RETURNING`
/// read-back here would need a dialect split of its own and two SQL shapes to
/// keep in step, for no gain over the plain select below.
///
/// `$1` is `user_id`, so a caller can never address another user's rows.
/// `restrict_to` narrows to an explicit id set for the `bulk_*` entry points;
/// `None` lets `state_predicate` select on its own, which is `mark_all_as_read`.
///
/// `restrict_to` must be `None` or a *non-empty* slice. `Some(&[])` builds
/// `IN ()`, which is a syntax error in both dialects. No caller can reach that
/// today — `change_notifications` returns early on an empty set and
/// `mark_as_read` passes exactly one id — but the constraint belongs to the SQL
/// built here, so a new caller has to read it here rather than infer it from the
/// existing ones.
async fn affected_notification_ids(
    tx: &mut DbTx,
    user_id: &str,
    state_predicate: &str,
    restrict_to: Option<&[String]>,
) -> trakkt_core::Result<Vec<String>> {
    #[derive(sqlx::FromRow)]
    struct IdRow {
        notification_id: String,
    }

    let mut sql = format!(
        "SELECT notification_id FROM notifications \
         WHERE user_id = $1 AND {state_predicate}"
    );
    if let Some(ids) = restrict_to {
        let (in_clause, _) = trakkt_core::db::in_clause_placeholders(ids.len(), 2);
        sql.push_str(" AND notification_id IN ");
        sql.push_str(&in_clause);
    }

    // The bind count is decided at runtime, so this needs the runtime-bind form.
    // `tx_with!` is the transaction-scoped `db_with_pool!`; running the pool
    // form here would read outside the transaction that is about to write.
    let rows: Vec<IdRow> = trakkt_core::tx_with!(&mut *tx, |e| {
        let mut query = sqlx::query_as::<_, IdRow>(&sql).bind(user_id);
        for id in restrict_to.unwrap_or(&[]) {
            query = query.bind(id);
        }
        query.fetch_all(e).await
    })?;

    Ok(rows.into_iter().map(|r| r.notification_id).collect())
}

/// Apply `set_clause` to exactly `ids`, then record one `Update` entry per row.
///
/// Updating by id rather than by re-stating the predicate is what keeps the
/// entries and the rows in step: the predicate could match a row that arrived
/// between the select and the update, and that row would then be changed with
/// nothing on the wire to announce it.
///
/// The read-back runs on the transaction because the new state is not visible on
/// the pool until the commit — and on SQLite the pool is not reachable at all
/// while the transaction is open. Each entry takes its `workspace_id` from its
/// own row: the `bulk_*` entry points take a bare id list, which nothing
/// constrains to a single workspace.
///
/// The action is [`SyncActionType::Update`] for all six, soft-delete and restore
/// included. `Delete` means the row has ceased to exist, which is what
/// `issue_service::delete_issue` reports when its cascade physically destroys a
/// notification; a soft-deleted row is still there, and the two have to stay
/// distinguishable on the wire.
///
/// `ids` must be non-empty. It is interpolated into two `IN` clauses, and an
/// empty slice builds `IN ()`, which is a syntax error in both dialects. Both
/// callers pass a non-empty [`affected_notification_ids`] result, having
/// returned early when it came back empty; the constraint is recorded here
/// because it is a property of the SQL below and not of those two call sites.
async fn update_and_record_notifications<'a>(
    tx: &mut DbTx,
    batch: &mut SyncBatch<'a>,
    user_id: &'a str,
    set_clause: &str,
    ids: &[String],
) -> trakkt_core::Result<()> {
    let (in_clause, _) = trakkt_core::db::in_clause_placeholders(ids.len(), 1);

    let update_sql =
        format!("UPDATE notifications SET {set_clause} WHERE notification_id IN {in_clause}");
    trakkt_core::tx_with!(&mut *tx, |e| {
        let mut query = sqlx::query(&update_sql);
        for id in ids {
            query = query.bind(id);
        }
        // Mapped to `()` so both backend arms agree on a type; the row count is
        // not consulted — `ids` already names exactly the rows to change.
        query.execute(e).await.map(|_| ())
    })?;

    let select_sql = format!("{NOTIFICATION_SELECT} WHERE n.notification_id IN {in_clause}");
    let rows: Vec<NotificationRow> = trakkt_core::tx_with!(&mut *tx, |e| {
        let mut query = sqlx::query_as::<_, NotificationRow>(&select_sql);
        for id in ids {
            query = query.bind(id);
        }
        query.fetch_all(e).await
    })?;

    for row in rows {
        let notification = row.into_dto();
        let payload = sync_log_service::sync_payload(
            &notification,
            entity_types::NOTIFICATION,
            &notification.notification_id,
        );

        batch
            .record(
                &mut *tx,
                entity_types::NOTIFICATION,
                &notification.notification_id,
                &notification.workspace_id,
                // A notification belongs to its recipient alone. Downgrading
                // this to `SyncAudience::Workspace` is the TRA-9920 leak.
                SyncAudience::User(user_id),
                SyncActionType::Update,
                payload,
            )
            .await?;
    }

    Ok(())
}

/// Open a transaction, apply `set_clause` to the notifications of `user_id` that
/// `state_predicate` selects, log an entry for each, then commit and deliver.
///
/// Delivery happens through [`SyncBatch`] and therefore strictly after the
/// commit. It cannot be moved next to the writes in
/// [`update_and_record_notifications`]: resolving a frame's recipients reads the
/// pool, and on SQLite this transaction is holding the only connection.
async fn change_notifications(
    db: &DbPool,
    user_id: &str,
    set_clause: &str,
    state_predicate: &str,
    restrict_to: Option<&[String]>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // An empty id set would build `IN ()`, which is a syntax error in both
    // dialects, and asks for nothing anyway.
    if matches!(restrict_to, Some(ids) if ids.is_empty()) {
        return Ok(());
    }

    let mut tx = db.begin().await?;

    let affected =
        affected_notification_ids(&mut tx, user_id, state_predicate, restrict_to).await?;
    if affected.is_empty() {
        // Nothing matched, so there is nothing to change and nothing to
        // announce. End the transaction rather than committing an empty one —
        // on SQLite it holds the only connection until it does.
        tx.rollback().await?;
        return Ok(());
    }

    let mut batch = SyncBatch::new();
    update_and_record_notifications(&mut tx, &mut batch, user_id, set_clause, &affected).await?;
    batch.commit_and_deliver(tx, ws_manager).await
}

/// Mark a single notification as read. Verifies the user owns it.
///
/// The UPDATE and its `sync_log` entry are one transaction: a read state that
/// commits without its entry leaves the user's other tabs showing the
/// notification unread through every reconnect, since only a full bootstrap
/// would correct them.
///
/// Note this has no read-state predicate, so marking an already-read
/// notification read succeeds rather than erroring — which is the behaviour
/// before this became transactional — and now also writes an entry. Only the
/// success is prior behaviour; the entry is new, because nothing here wrote
/// `sync_log` entries at all before this change.
pub async fn mark_as_read(
    db: &DbPool,
    notification_id: &str,
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let requested = [notification_id.to_string()];

    let mut tx = db.begin().await?;

    let affected =
        affected_notification_ids(&mut tx, user_id, "deleted_at IS NULL", Some(&requested)).await?;

    if affected.is_empty() {
        // Distinguish "not found" from "not owned by user". This probe runs on
        // the transaction, not the pool: the transaction is already open and on
        // SQLite it holds the only connection, so a `db_fetch_scalar!` here
        // would block until sqlx's acquire timeout and then fail.
        let exists: i64 = trakkt_core::tx_fetch_scalar!(
            &mut tx,
            i64,
            "SELECT COUNT(*) FROM notifications WHERE notification_id = $1",
            notification_id
        )?;
        tx.rollback().await?;

        if exists == 0 {
            return Err(trakkt_core::Error::NotFound(format!(
                "notification {notification_id} not found"
            )));
        }
        return Err(trakkt_core::Error::Forbidden(
            "you can only mark your own notifications as read".to_string(),
        ));
    }

    let mut batch = SyncBatch::new();
    update_and_record_notifications(
        &mut tx,
        &mut batch,
        user_id,
        &format!("read = {bt}"),
        &affected,
    )
    .await?;
    batch.commit_and_deliver(tx, ws_manager).await
}

/// Mark all of a user's notifications as read.
///
/// One entry per notification actually flipped — the already-read ones are
/// excluded by the predicate and get none.
pub async fn mark_all_as_read(
    db: &DbPool,
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);

    change_notifications(
        db,
        user_id,
        &format!("read = {bt}"),
        &format!("read = {bf} AND deleted_at IS NULL"),
        None,
        ws_manager,
    )
    .await
}

/// Count unread notifications for a user.
pub async fn count_unread(
    db: &DbPool,
    user_id: &str,
) -> trakkt_core::Result<i64> {
    let is_pg = db.is_postgres();
    let bf = sql_compat::bool_false(is_pg);

    let sql = format!(
        "SELECT COUNT(*) FROM notifications \
         WHERE user_id = $1 AND read = {bf} AND deleted_at IS NULL"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(db, i64, &sql, user_id)?;
    Ok(count)
}

/// Bulk mark specific notifications as read. Only affects the given user's
/// non-deleted, currently-unread notifications — the already-read ones among
/// `notification_ids` are excluded by the predicate and get no entry.
pub async fn bulk_mark_as_read(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);

    change_notifications(
        db,
        user_id,
        &format!("read = {bt}"),
        &format!("read = {bf} AND deleted_at IS NULL"),
        Some(notification_ids),
        ws_manager,
    )
    .await
}

/// Bulk mark specific notifications as unread. Only affects the given user's
/// non-deleted, currently-read notifications.
pub async fn bulk_mark_as_unread(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);

    change_notifications(
        db,
        user_id,
        &format!("read = {bf}"),
        &format!("read = {bt} AND deleted_at IS NULL"),
        Some(notification_ids),
        ws_manager,
    )
    .await
}

/// Soft-delete specific notifications. Only affects the given user's
/// non-deleted notifications.
///
/// The entry is an `Update`, not a `Delete`: the row is still there with
/// `deleted_at` stamped, and `issue_service::delete_issue` uses `Delete` for
/// notifications its cascade physically destroys. Keeping the two apart is what
/// lets a client tell "hidden from my inbox" from "gone".
pub async fn bulk_delete_notifications(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    change_notifications(
        db,
        user_id,
        &format!("deleted_at = {now}"),
        "deleted_at IS NULL",
        Some(notification_ids),
        ws_manager,
    )
    .await
}

/// Restore soft-deleted notifications. Only affects the given user's
/// currently-deleted notifications.
pub async fn bulk_restore_notifications(
    db: &DbPool,
    notification_ids: &[String],
    user_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // NULL is dialect-neutral; no sql_compat call needed.
    change_notifications(
        db,
        user_id,
        "deleted_at = NULL",
        "deleted_at IS NOT NULL",
        Some(notification_ids),
        ws_manager,
    )
    .await
}

// ─── Notification Preferences ─────────────────────────────────────────────

/// Internal row type for deserialising notification preferences queries.
#[derive(sqlx::FromRow)]
struct NotificationPreferencesRow {
    preference_id: String,
    user_id: String,
    workspace_id: String,
    notify_status_changes: bool,
    notify_comments: bool,
    notify_assignments: bool,
    notify_priority_changes: bool,
    notify_label_changes: bool,
    notify_due_date_changes: bool,
    notify_estimate_changes: bool,
    notify_milestone_changes: bool,
    notify_project_changes: bool,
    notify_team_changes: bool,
    notify_relation_changes: bool,
    notify_own_agent_actions: bool,
    notify_own_api_actions: bool,
    delivery_channel: String,
}

impl NotificationPreferencesRow {
    fn into_dto(self) -> NotificationPreferences {
        NotificationPreferences {
            preference_id: self.preference_id,
            user_id: self.user_id,
            workspace_id: self.workspace_id,
            notify_status_changes: self.notify_status_changes,
            notify_comments: self.notify_comments,
            notify_assignments: self.notify_assignments,
            notify_priority_changes: self.notify_priority_changes,
            notify_label_changes: self.notify_label_changes,
            notify_due_date_changes: self.notify_due_date_changes,
            notify_estimate_changes: self.notify_estimate_changes,
            notify_milestone_changes: self.notify_milestone_changes,
            notify_project_changes: self.notify_project_changes,
            notify_team_changes: self.notify_team_changes,
            notify_relation_changes: self.notify_relation_changes,
            notify_own_agent_actions: self.notify_own_agent_actions,
            notify_own_api_actions: self.notify_own_api_actions,
            delivery_channel: self.delivery_channel,
        }
    }
}

/// Base SELECT for notification preferences queries.
///
/// Shared by the pool, transaction and batch reads so the column list cannot
/// drift between them — the same shape the client is handed as a sync payload.
const PREFERENCES_SELECT: &str = "\
    SELECT preference_id, user_id, workspace_id, \
           notify_status_changes, notify_comments, notify_assignments, \
           notify_priority_changes, \
           notify_label_changes, notify_due_date_changes, notify_estimate_changes, \
           notify_milestone_changes, notify_project_changes, notify_team_changes, \
           notify_relation_changes, \
           notify_own_agent_actions, notify_own_api_actions, \
           delivery_channel \
    FROM notification_preferences";

/// Get notification preferences for a user in a workspace.
///
/// Uses an upsert (INSERT ... ON CONFLICT DO NOTHING / INSERT OR IGNORE)
/// to atomically create a default row if none exists, avoiding a TOCTOU race
/// between the existence check and the insert.
///
/// When the upsert actually inserts, that INSERT and its `sync_log` entry are
/// one transaction: a preferences row that commits without its sync row leaves
/// the settings screen on the user's other browsers showing nothing until they
/// next bootstrap. When the row already existed nothing was written, so the
/// transaction is rolled back and no sync entry is produced — this function is
/// called on every preferences read, and logging an entry per read would flood
/// the delta stream.
///
/// The returned defaults are read back from the row the database actually
/// stored rather than reconstructed in Rust, so a change to the column defaults
/// cannot leave the sync payload describing preferences the user does not have.
pub async fn get_or_default_preferences(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<NotificationPreferences> {
    let preference_id = uuid::Uuid::new_v4().to_string();
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let bf = sql_compat::bool_false(is_pg);

    // Upsert: insert with defaults if not exists, ignore if already present.
    let insert_prefix = if is_pg { "INSERT INTO" } else { "INSERT OR IGNORE INTO" };
    let conflict_clause = if is_pg { "ON CONFLICT (user_id, workspace_id) DO NOTHING" } else { "" };

    let sql = format!(
        "{insert_prefix} notification_preferences \
            (preference_id, user_id, workspace_id, \
             notify_status_changes, notify_comments, notify_assignments, \
             notify_priority_changes, \
             notify_label_changes, notify_due_date_changes, notify_estimate_changes, \
             notify_milestone_changes, notify_project_changes, notify_team_changes, \
             notify_relation_changes, \
             notify_own_agent_actions, notify_own_api_actions, \
             delivery_channel, created_at, updated_at) \
         VALUES ($1, $2, $3, {bt}, {bt}, {bt}, {bt}, {bt}, {bt}, {bt}, {bt}, {bt}, {bt}, {bt}, {bf}, {bf}, $4, {now}, {now}) \
         {conflict_clause}"
    );
    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &preference_id,
        user_id,
        workspace_id,
        "in_app"
    )?;
    let inserted = result.rows_affected() > 0;

    // Always fetch the current state (whether we just inserted or it already
    // existed). This runs on the transaction: on the insert path the row does
    // not exist outside it yet.
    let row: Option<NotificationPreferencesRow> = trakkt_core::tx_fetch_optional!(
        &mut tx,
        NotificationPreferencesRow,
        &format!("{PREFERENCES_SELECT} WHERE user_id = $1 AND workspace_id = $2"),
        user_id,
        workspace_id
    )?;

    let Some(row) = row else {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound("Notification preferences".into()));
    };
    let prefs = row.into_dto();

    if !inserted {
        // The row already existed, so nothing was written and there is nothing
        // to log. Release the transaction before returning — on SQLite it holds
        // the only connection until it ends.
        tx.rollback().await?;
        return Ok(prefs);
    }

    // Preferences are one user's settings (the table is keyed
    // UNIQUE(user_id, workspace_id)) — scope the row and the frame to them.
    let payload = sync_log_service::sync_payload(
        &prefs,
        entity_types::NOTIFICATION_PREFERENCES,
        &prefs.preference_id,
    );

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::NOTIFICATION_PREFERENCES,
        &prefs.preference_id,
        workspace_id,
        SyncAudience::User(user_id),
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    Ok(prefs)
}

/// Update a single boolean preference field.
///
/// The field name is validated against a fixed allow-list to prevent SQL
/// injection. Returns the updated preferences.
///
/// The UPDATE and its `sync_log` entry are one transaction, scoped to the owning
/// user via [`SyncAudience::User`].
///
/// # Why the row is ensured before the transaction opens
///
/// [`get_or_default_preferences`] takes a `&DbPool` and opens a transaction of
/// its own. Calling it between our `begin` and `commit` would nest one
/// transaction inside another and, on SQLite, deadlock outright — the pool is
/// pinned to a single connection which ours already holds. So it runs first, on
/// the pool, and finishes before `begin`. The read-back afterwards cannot use it
/// for the same reason and goes through [`PREFERENCES_SELECT`] on the
/// transaction instead — which is also the only way to see the row this
/// transaction just wrote.
///
/// That ordering has a second consequence worth stating: when the preferences
/// row does not exist yet, this function produces **two** sync entries — an
/// `Insert` from `get_or_default_preferences` and then this `Update`. They are
/// separate transactions, so each rolls back independently.
pub async fn update_preference(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    field: &str,
    value: bool,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<NotificationPreferences> {
    let column = match field {
        "notify_status_changes" => "notify_status_changes",
        "notify_comments" => "notify_comments",
        "notify_assignments" => "notify_assignments",
        "notify_priority_changes" => "notify_priority_changes",
        "notify_label_changes" => "notify_label_changes",
        "notify_due_date_changes" => "notify_due_date_changes",
        "notify_estimate_changes" => "notify_estimate_changes",
        "notify_milestone_changes" => "notify_milestone_changes",
        "notify_project_changes" => "notify_project_changes",
        "notify_team_changes" => "notify_team_changes",
        "notify_relation_changes" => "notify_relation_changes",
        "notify_own_agent_actions" => "notify_own_agent_actions",
        "notify_own_api_actions" => "notify_own_api_actions",
        _ => {
            return Err(trakkt_core::Error::BadRequest(format!(
                "Unknown preference field: {field}"
            )));
        }
    };

    // Ensure the row exists (inserts defaults if missing). Runs on the pool and
    // completes its own transaction before ours opens — see the note above.
    let _prefs = get_or_default_preferences(db, user_id, workspace_id, ws_manager).await?;

    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let bool_val = if value {
        sql_compat::bool_true(is_pg)
    } else {
        sql_compat::bool_false(is_pg)
    };

    let sql = format!(
        "UPDATE notification_preferences SET {column} = {bool_val}, updated_at = {now} \
         WHERE user_id = $1 AND workspace_id = $2"
    );
    let mut tx = db.begin().await?;

    trakkt_core::tx_execute!(&mut tx, &sql, user_id, workspace_id)?;

    // Re-fetch the updated preferences on the transaction — the new value is not
    // visible on the pool until the commit.
    let row: Option<NotificationPreferencesRow> = trakkt_core::tx_fetch_optional!(
        &mut tx,
        NotificationPreferencesRow,
        &format!("{PREFERENCES_SELECT} WHERE user_id = $1 AND workspace_id = $2"),
        user_id,
        workspace_id
    )?;

    let Some(row) = row else {
        tx.rollback().await?;
        return Err(trakkt_core::Error::NotFound("Notification preferences".into()));
    };
    let updated = row.into_dto();

    let payload = sync_log_service::sync_payload(
        &updated,
        entity_types::NOTIFICATION_PREFERENCES,
        &updated.preference_id,
    );

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::NOTIFICATION_PREFERENCES,
        &updated.preference_id,
        workspace_id,
        SyncAudience::User(user_id),
        SyncActionType::Update,
        payload,
        ws_manager,
    )
    .await?;

    Ok(updated)
}

/// Batch-fetch notification preferences for multiple users in a workspace.
///
/// Returns a map keyed by `user_id`. Users without a preferences row are
/// simply absent from the map — callers should treat missing entries as
/// "all defaults enabled".
pub async fn batch_get_preferences(
    db: &DbPool,
    user_ids: &[String],
    workspace_id: &str,
) -> trakkt_core::Result<std::collections::HashMap<String, NotificationPreferences>> {
    if user_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let (in_clause, _) = trakkt_core::db::in_clause_placeholders(user_ids.len(), 2);
    let sql = format!(
        "{PREFERENCES_SELECT} WHERE workspace_id = $1 AND user_id IN {in_clause}"
    );

    let rows: Vec<NotificationPreferencesRow> = trakkt_core::db_with_pool!(db, |pool| {
        let mut query = sqlx::query_as::<_, NotificationPreferencesRow>(&sql)
            .bind(workspace_id);
        for uid in user_ids {
            query = query.bind(uid);
        }
        query.fetch_all(pool).await
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let uid = row.user_id.clone();
        map.insert(uid, row.into_dto());
    }
    Ok(map)
}

/// Returns `true` if the notification should be suppressed for this watcher
/// because they are the actor and their preferences say to skip self-notify.
pub fn should_suppress_self_notification(
    watcher_id: &str,
    actor_id: &str,
    action_source: ActionSource,
    prefs_map: &std::collections::HashMap<String, NotificationPreferences>,
) -> bool {
    if watcher_id != actor_id {
        return false;
    }
    match action_source {
        ActionSource::User => true,
        ActionSource::Agent => !prefs_map.get(watcher_id).is_some_and(|p| p.notify_own_agent_actions),
        ActionSource::Api => !prefs_map.get(watcher_id).is_some_and(|p| p.notify_own_api_actions),
        ActionSource::Github => false,
    }
}
