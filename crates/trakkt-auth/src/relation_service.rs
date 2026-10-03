// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relation service — CRUD operations for the `issue_relations` table.
//!
//! Issue relations model typed, directional relationships between issues
//! (e.g. "blocks"). The service supports creating, deleting, and listing
//! relations with full sync log integration.

use trakkt_core::DbPool;
use trakkt_types::models::{IssueRelation, IssueRelationWithDetails};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising basic relation queries.
#[derive(sqlx::FromRow)]
struct IssueRelationRow {
    relation_id: String,
    workspace_id: String,
    source_issue_id: String,
    target_issue_id: String,
    relation_type: String,
    created_by: Option<String>,
    created_at: String,
}

impl IssueRelationRow {
    fn into_dto(self) -> IssueRelation {
        IssueRelation {
            relation_id: self.relation_id,
            workspace_id: self.workspace_id,
            source_issue_id: self.source_issue_id,
            target_issue_id: self.target_issue_id,
            relation_type: self.relation_type,
            created_by: self.created_by,
            created_at: self.created_at,
        }
    }
}

/// Internal row type for relation queries joined with issue, team, and status tables.
#[derive(sqlx::FromRow)]
struct IssueRelationDetailRow {
    relation_id: String,
    relation_type: String,
    issue_id: String,
    team_key: String,
    number: i32,
    title: String,
    status_category: String,
    status_name: String,
    direction: String,
}

impl IssueRelationDetailRow {
    fn into_dto(self) -> IssueRelationWithDetails {
        IssueRelationWithDetails {
            relation_id: self.relation_id,
            relation_type: self.relation_type,
            issue_id: self.issue_id,
            team_key: self.team_key,
            number: self.number,
            title: self.title,
            status_category: self.status_category,
            status_name: self.status_name,
            direction: self.direction,
        }
    }
}

/// Internal row type for workspace ownership checks.
#[derive(sqlx::FromRow)]
struct WorkspaceIdRow {
    workspace_id: String,
}

/// Internal row type for single-column ID queries.
#[derive(sqlx::FromRow)]
struct IdRow {
    target_issue_id: String,
}

const VALID_RELATION_TYPES: &[&str] = &["blocks", "parent", "duplicate", "relates_to"];

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Validate that creating a "blocks" relation from `source_issue_id` to
/// `target_issue_id` would not create a circular blocking chain.
///
/// Uses BFS from `target_issue_id` through all existing "blocks" relations.
/// If `source_issue_id` is reachable, the new relation would create a cycle.
async fn validate_no_circular_blocking(
    db: &DbPool,
    source_issue_id: &str,
    target_issue_id: &str,
) -> trakkt_core::Result<()> {
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(target_issue_id.to_owned());

    while let Some(current_id) = queue.pop_front() {
        if !visited.insert(current_id.clone()) {
            continue;
        }
        if current_id == source_issue_id {
            return Err(trakkt_core::Error::BadRequest(
                "Circular blocking chain: this would create a cycle".to_string(),
            ));
        }
        let blocked = find_blocked_issue_ids(db, &current_id).await?;
        for next in blocked {
            queue.push_back(next);
        }
    }
    Ok(())
}

/// Validate that creating a parent relation from `source_issue_id` (parent) to
/// `target_issue_id` (child) would not create a circular parent chain.
///
/// Walks up from the proposed parent through existing parent relations. If
/// the child is encountered as an ancestor, the new relation would create a cycle.
async fn validate_no_circular_parent(
    db: &DbPool,
    source_issue_id: &str,
    target_issue_id: &str,
) -> trakkt_core::Result<()> {
    // Delegate to the shared cycle-detection logic in issue_service.
    // For parent relations, source = parent, target = child.
    // We walk up from the proposed parent (source) and check if the child (target)
    // is already an ancestor — which would create a cycle.
    crate::issue_service::validate_no_circular_reference(db, target_issue_id, source_issue_id).await
}

/// Validate that the child (`target_issue_id`) does not already have a parent relation.
async fn validate_single_parent(
    db: &DbPool,
    target_issue_id: &str,
) -> trakkt_core::Result<()> {
    let existing: Option<String> = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_scalar::<_, String>(
            "SELECT relation_id FROM issue_relations \
             WHERE target_issue_id = $1 AND relation_type = 'parent'",
        )
        .bind(target_issue_id)
        .fetch_optional(p)
        .await
    })?;

    if existing.is_some() {
        return Err(trakkt_core::Error::BadRequest(
            "This issue already has a parent. Remove the existing parent first.".to_string(),
        ));
    }
    Ok(())
}

/// Validate that the source issue does not already have an outward duplicate relation.
///
/// An issue can only be a duplicate of ONE other issue. Multiple issues can
/// point at the same original (i.e. the target can have many inward duplicates).
async fn validate_single_duplicate(
    db: &DbPool,
    source_issue_id: &str,
) -> trakkt_core::Result<()> {
    let existing: Option<String> = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_scalar::<_, String>(
            "SELECT relation_id FROM issue_relations \
             WHERE source_issue_id = $1 AND relation_type = 'duplicate'",
        )
        .bind(source_issue_id)
        .fetch_optional(p)
        .await
    })?;

    if existing.is_some() {
        return Err(trakkt_core::Error::BadRequest(
            "This issue is already marked as a duplicate. Remove the existing duplicate relation first.".to_string(),
        ));
    }
    Ok(())
}

// ─── Blocked-issue lookup ──────────────────────────────────────────────────

/// Returns the IDs of all issues that `blocker_issue_id` is blocking.
///
/// Queries the `issue_relations` table for rows where the source (blocker) is
/// `blocker_issue_id` and the relation type is "blocks".
pub async fn find_blocked_issue_ids(
    db: &DbPool,
    blocker_issue_id: &str,
) -> trakkt_core::Result<Vec<String>> {
    let rows: Vec<IdRow> = trakkt_core::db_fetch_all!(
        db,
        IdRow,
        "SELECT target_issue_id FROM issue_relations \
         WHERE source_issue_id = $1 AND relation_type = 'blocks'",
        blocker_issue_id
    )?;
    Ok(rows.into_iter().map(|r| r.target_issue_id).collect())
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Fetch a single relation by ID and workspace (returns `None` if not found).
pub async fn get_relation_by_id(
    db: &DbPool,
    relation_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Option<IssueRelation>> {
    let row: Option<IssueRelationRow> = trakkt_core::db_fetch_optional!(
        db,
        IssueRelationRow,
        "SELECT relation_id, workspace_id, source_issue_id, target_issue_id, \
                relation_type, created_by, CAST(created_at AS TEXT) AS created_at \
         FROM issue_relations \
         WHERE relation_id = $1 AND workspace_id = $2",
        relation_id,
        workspace_id
    )?;
    Ok(row.map(IssueRelationRow::into_dto))
}

/// Create a new relation between two issues.
///
/// Validates that:
/// - The source and target are different issues
/// - Both issues exist and belong to the same workspace
/// - No circular blocking chain would be created
///
/// Every validation above runs on the pool, before the transaction opens, and
/// has to stay there: they walk the relation graph with `&DbPool` queries, and
/// the SQLite pool is pinned to a single connection which the transaction holds
/// for its whole lifetime. A validation moved inside it would block waiting for
/// a connection only the transaction can release, until the pool's acquire
/// timeout elapsed — 30s, the sqlx default, which the SQLite branch of
/// `DbPool::connect` does not override — and then fail with `PoolTimedOut` (see
/// `DbTx`). So the cost is a request that stalls for half a minute and then
/// errors, not one that never returns. They are also pure reads of state that
/// predates the INSERT, so there is nothing to gain by moving them.
///
/// The INSERT and its `sync_log` entry are one transaction: a relation that
/// commits without its sync row is invisible to every future delta, so a failed
/// log write rolls the relation back rather than leaving it stranded.
pub async fn create_relation(
    db: &DbPool,
    workspace_id: &str,
    source_issue_id: &str,
    target_issue_id: &str,
    relation_type: &str,
    created_by: Option<&str>,
    action_source: trakkt_types::enums::ActionSource,
    action_source_label: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<IssueRelation> {
    // Self-relation check.
    if source_issue_id == target_issue_id {
        return Err(trakkt_core::Error::BadRequest(
            "An issue cannot be related to itself".to_string(),
        ));
    }

    // Validate relation type.
    if !VALID_RELATION_TYPES.contains(&relation_type) {
        return Err(trakkt_core::Error::BadRequest(
            format!("Invalid relation type: {relation_type}. Valid types: {}", VALID_RELATION_TYPES.join(", ")),
        ));
    }

    // Validate both issues exist and are in the same workspace.
    let source_ws = trakkt_core::db_fetch_optional!(
        db,
        WorkspaceIdRow,
        "SELECT workspace_id FROM issues WHERE issue_id = $1",
        source_issue_id
    )?;
    let target_ws = trakkt_core::db_fetch_optional!(
        db,
        WorkspaceIdRow,
        "SELECT workspace_id FROM issues WHERE issue_id = $1",
        target_issue_id
    )?;

    let Some(source_ws) = source_ws else {
        return Err(trakkt_core::Error::NotFound(
            "Source issue not found".to_string(),
        ));
    };
    let Some(target_ws) = target_ws else {
        return Err(trakkt_core::Error::NotFound(
            "Target issue not found".to_string(),
        ));
    };

    if source_ws.workspace_id != workspace_id || target_ws.workspace_id != workspace_id {
        return Err(trakkt_core::Error::BadRequest(
            "Both issues must belong to the same workspace".to_string(),
        ));
    }

    // Validate no circular blocking chains for "blocks" relations.
    if relation_type == "blocks" {
        validate_no_circular_blocking(db, source_issue_id, target_issue_id).await?;
    }

    // Validate parent relations: no cycles, and only one parent per child.
    if relation_type == "parent" {
        validate_no_circular_parent(db, source_issue_id, target_issue_id).await?;
        validate_single_parent(db, target_issue_id).await?;
    }

    // Validate duplicate relations: only one outward duplicate per source issue.
    if relation_type == "duplicate" {
        validate_single_duplicate(db, source_issue_id).await?;
    }

    // Insert the relation.
    let is_pg = db.is_postgres();
    let now = trakkt_core::sql_compat::now(is_pg);
    let relation_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO issue_relations \
            (relation_id, workspace_id, source_issue_id, target_issue_id, relation_type, created_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {now})"
    );
    let mut tx = db.begin().await?;

    // The UNIQUE(source, target, type) violation is a user-visible "you already
    // have this relation", not a server fault, so it is mapped before the `?`.
    // The mapping survives the move off `db_execute!` because both macros leave
    // the error as `sqlx::Error` and the driver builds it, not the executor: the
    // violation still arrives as `Error::Database` carrying the backend's code
    // and message. Pinned by
    // `sync_log_service::tests::a_duplicate_relation_is_rejected_as_a_bad_request`,
    // which asserts the variant; on SQLite the error it observes is code 2067,
    // message "UNIQUE constraint failed: …". Returning here drops `tx`, which
    // queues a rollback (see `DbTx`).
    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &relation_id,
        workspace_id,
        source_issue_id,
        target_issue_id,
        relation_type,
        created_by
    )
    .map_err(|e| {
        if let sqlx::Error::Database(ref db_err) = e {
            let is_unique = db_err
                .code()
                .map(|c| c == "23505" || c == "2067")
                .unwrap_or(false)
                || db_err.message().contains("UNIQUE constraint");
            if is_unique {
                return trakkt_core::Error::BadRequest(
                    "This relation already exists".to_string(),
                );
            }
        }
        trakkt_core::Error::from(e)
    })?;

    // Re-fetch to get DB-assigned timestamps. The row does not exist outside the
    // transaction yet, so the read runs on it.
    let row = trakkt_core::tx_fetch_one!(
        &mut tx,
        IssueRelationRow,
        "SELECT relation_id, workspace_id, source_issue_id, target_issue_id, \
                relation_type, created_by, \
                CAST(created_at AS TEXT) AS created_at \
         FROM issue_relations WHERE relation_id = $1",
        &relation_id
    )?;
    let relation = row.into_dto();

    // One payload for the persisted entry and the live frame alike. The stored
    // entry used to be `None` while the broadcast carried the serialized
    // relation, so a client that reconnected replayed a payload-less insert —
    // which `cache/apply.rs` drops at its data-less guard, leaving issue
    // relations permanently absent from delta sync.
    let payload = sync_log_service::sync_payload(
        &relation,
        entity_types::ISSUE_RELATION,
        &relation.relation_id,
    );

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::ISSUE_RELATION,
        &relation.relation_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Insert,
        payload,
        ws_manager,
    )
    .await?;

    // ── Notification trigger for relation_added (best-effort) ────────
    // Runs on the pool, strictly after the commit above released it.
    if let Some(actor_id) = created_by {
        // Gather watchers from both source and target issues.
        let source_watchers = crate::watcher_service::list_watchers_of_issue(db, source_issue_id).await;
        let target_watchers = crate::watcher_service::list_watchers_of_issue(db, target_issue_id).await;

        let mut all_watcher_ids = std::collections::HashSet::new();
        if let Ok(ref ids) = source_watchers {
            for id in ids {
                all_watcher_ids.insert(id.clone());
            }
        } else if let Err(ref e) = source_watchers {
            tracing::warn!(error = %e, "Failed to list watchers for source issue in relation notification");
        }
        if let Ok(ref ids) = target_watchers {
            for id in ids {
                all_watcher_ids.insert(id.clone());
            }
        } else if let Err(ref e) = target_watchers {
            tracing::warn!(error = %e, "Failed to list watchers for target issue in relation notification");
        }

        if !all_watcher_ids.is_empty() {
            let all_watcher_vec: Vec<String> = all_watcher_ids.iter().cloned().collect();
            let prefs_map = match crate::notification_service::batch_get_preferences(
                db, &all_watcher_vec, workspace_id,
            )
            .await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to fetch notification preferences for relation notification");
                    std::collections::HashMap::new()
                }
            };

            // Notify watchers of both issues about the relation.
            // Source issue watchers get notified on the source issue,
            // target issue watchers get notified on the target issue.
            // Build (issue_id, watcher_ids) pairs to iterate.
            let mut notify_pairs: Vec<(&str, &[String])> = Vec::new();
            if let Ok(ref ids) = source_watchers {
                notify_pairs.push((source_issue_id, ids));
            }
            if let Ok(ref ids) = target_watchers {
                notify_pairs.push((target_issue_id, ids));
            }

            for (issue_id, watcher_ids) in notify_pairs {
                for watcher_id in watcher_ids {
                    if crate::notification_service::should_suppress_self_notification(
                        watcher_id, actor_id, action_source, &prefs_map,
                    ) {
                        continue;
                    }

                    let type_enabled = prefs_map
                        .get(watcher_id.as_str())
                        .is_none_or(|p| p.notify_relation_changes);
                    if !type_enabled {
                        continue;
                    }

                    if let Err(e) = crate::notification_service::create_notification(
                        db,
                        workspace_id,
                        watcher_id,
                        issue_id,
                        crate::notification_service::TYPE_RELATION_ADDED,
                        Some(actor_id),
                        None,
                        action_source,
                        action_source_label,
                        ws_manager,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "Failed to create relation_added notification");
                    }
                }
            }
        }
    }

    Ok(relation)
}

/// Delete a relation by ID.
///
/// Verifies the relation belongs to the specified workspace before deleting.
///
/// The DELETE and its `sync_log` entry are one transaction: a delete that
/// commits without its sync row leaves the relation on every other client
/// forever, and no later delta can repair it — the row it would have to re-read
/// is gone.
pub async fn delete_relation(
    db: &DbPool,
    relation_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let mut tx = db.begin().await?;

    let result = trakkt_core::tx_execute!(
        &mut tx,
        "DELETE FROM issue_relations WHERE relation_id = $1 AND workspace_id = $2",
        relation_id,
        workspace_id
    )?;

    if result.rows_affected() == 0 {
        // `tx` is dropped here, which rolls it back (see `DbTx`).
        return Err(trakkt_core::Error::NotFound(
            "Relation not found".to_string(),
        ));
    }

    // The sync entry follows the DELETE it describes; a delete carries no
    // payload, since there is no row left to send.
    sync_log_service::commit_and_deliver(
        tx,
        entity_types::ISSUE_RELATION,
        relation_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Delete,
        None,
        ws_manager,
    )
    .await
}

/// List all relations for an issue, from both perspectives.
///
/// Returns relations where the issue is either the source or target, with
/// joined details about the *other* issue in each relation.
///
/// Direction mapping:
/// - "blocks": source blocks target. If issue is source => "blocks", if target => "blocked_by"
/// - "parent": source is parent, target is child. If issue is source => "parent" (has child),
///   if target => "child_of" (has parent)
/// - "duplicate": source is duplicate of target. If issue is source => "duplicate",
///   if target => "has_duplicate"
pub async fn list_relations_for_issue(
    db: &DbPool,
    issue_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<IssueRelationWithDetails>> {
    let rows: Vec<IssueRelationDetailRow> = trakkt_core::db_fetch_all!(
        db,
        IssueRelationDetailRow,
        "SELECT \
            r.relation_id, \
            r.relation_type, \
            other_i.issue_id AS issue_id, \
            t.key AS team_key, \
            other_i.number, \
            other_i.title, \
            s.category AS status_category, \
            s.name AS status_name, \
            CASE \
                WHEN r.source_issue_id = $1 AND r.relation_type = 'blocks' THEN 'blocks' \
                WHEN r.target_issue_id = $1 AND r.relation_type = 'blocks' THEN 'blocked_by' \
                WHEN r.source_issue_id = $1 AND r.relation_type = 'parent' THEN 'parent' \
                WHEN r.target_issue_id = $1 AND r.relation_type = 'parent' THEN 'child_of' \
                WHEN r.source_issue_id = $1 AND r.relation_type = 'duplicate' THEN 'duplicate' \
                WHEN r.target_issue_id = $1 AND r.relation_type = 'duplicate' THEN 'has_duplicate' \
                WHEN r.source_issue_id = $1 AND r.relation_type = 'relates_to' THEN 'relates_to' \
                WHEN r.target_issue_id = $1 AND r.relation_type = 'relates_to' THEN 'relates_to' \
                ELSE r.relation_type \
            END AS direction \
         FROM issue_relations r \
         JOIN issues other_i ON other_i.issue_id = CASE \
            WHEN r.source_issue_id = $1 THEN r.target_issue_id \
            ELSE r.source_issue_id \
         END \
         JOIN teams t ON t.team_id = other_i.team_id \
         JOIN statuses s ON s.status_id = other_i.status_id \
         WHERE (r.source_issue_id = $1 OR r.target_issue_id = $1) \
           AND r.workspace_id = $2 \
         ORDER BY r.relation_type, r.created_at",
        issue_id,
        workspace_id
    )?;
    Ok(rows.into_iter().map(IssueRelationDetailRow::into_dto).collect())
}

// ─── Parent helpers ────────────────────────────────────────────────────────

/// Get the parent issue_id for a given child issue.
///
/// Returns `Some(parent_id)` if a parent relation exists, `None` otherwise.
pub async fn get_parent_issue_id(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<Option<String>> {
    let parent: Option<String> = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_scalar::<_, String>(
            "SELECT source_issue_id FROM issue_relations \
             WHERE target_issue_id = $1 AND relation_type = 'parent'",
        )
        .bind(issue_id)
        .fetch_optional(p)
        .await
    })?;
    Ok(parent)
}

/// Set the parent of a child issue.
///
/// Clears any existing parent first, then creates the new parent relation.
/// Validates single-parent constraint and cycle prevention.
pub async fn set_parent(
    db: &DbPool,
    workspace_id: &str,
    child_issue_id: &str,
    parent_issue_id: &str,
    created_by: Option<&str>,
    action_source: trakkt_types::enums::ActionSource,
    action_source_label: Option<&str>,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<IssueRelation> {
    // Clear any existing parent first (idempotent).
    clear_parent(db, workspace_id, child_issue_id, ws_manager).await?;

    // Create the new parent relation (source = parent, target = child).
    create_relation(
        db,
        workspace_id,
        parent_issue_id,
        child_issue_id,
        "parent",
        created_by,
        action_source,
        action_source_label,
        ws_manager,
    )
    .await
}

/// Clear the parent relation for a child issue.
///
/// Finds and deletes the parent relation where the child is the target.
/// No-op if the child has no parent.
pub async fn clear_parent(
    db: &DbPool,
    workspace_id: &str,
    child_issue_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let relation_id: Option<String> = trakkt_core::db_with_pool!(db, |p| {
        sqlx::query_scalar::<_, String>(
            "SELECT relation_id FROM issue_relations \
             WHERE target_issue_id = $1 AND relation_type = 'parent' AND workspace_id = $2",
        )
        .bind(child_issue_id)
        .bind(workspace_id)
        .fetch_optional(p)
        .await
    })?;

    if let Some(rid) = relation_id {
        delete_relation(db, &rid, workspace_id, ws_manager).await?;
    }

    Ok(())
}
