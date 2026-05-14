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
            direction: self.direction,
        }
    }
}

/// Internal row type for workspace ownership checks.
#[derive(sqlx::FromRow)]
struct WorkspaceIdRow {
    workspace_id: String,
}

const VALID_RELATION_TYPES: &[&str] = &["blocks", "parent"];

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
        let blocked: Vec<String> = trakkt_core::db_with_pool!(db, |p| {
            sqlx::query_scalar::<_, String>(
                "SELECT target_issue_id FROM issue_relations \
                 WHERE source_issue_id = $1 AND relation_type = 'blocks'",
            )
            .bind(&current_id)
            .fetch_all(p)
            .await
        })?;
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

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new relation between two issues.
///
/// Validates that:
/// - The source and target are different issues
/// - Both issues exist and belong to the same workspace
/// - No circular blocking chain would be created
pub async fn create_relation(
    db: &DbPool,
    workspace_id: &str,
    source_issue_id: &str,
    target_issue_id: &str,
    relation_type: &str,
    created_by: Option<&str>,
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

    // Insert the relation.
    let is_pg = db.is_postgres();
    let now = trakkt_core::sql_compat::now(is_pg);
    let relation_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO issue_relations \
            (relation_id, workspace_id, source_issue_id, target_issue_id, relation_type, created_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {now})"
    );
    trakkt_core::db_execute!(
        db,
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

    // Re-fetch to get DB-assigned timestamps.
    let row = trakkt_core::db_fetch_one!(
        db,
        IssueRelationRow,
        "SELECT relation_id, workspace_id, source_issue_id, target_issue_id, \
                relation_type, created_by, \
                CAST(created_at AS TEXT) AS created_at \
         FROM issue_relations WHERE relation_id = $1",
        &relation_id
    )?;
    let relation = row.into_dto();

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE_RELATION,
        &relation.relation_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, relation_id = %relation.relation_id, "Failed to write sync log for relation create");
    }

    // WebSocket broadcast — send full entity data as SyncResponse.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ISSUE_RELATION,
            &relation.relation_id,
            SyncActionType::Insert,
            serde_json::to_value(&relation).ok(),
        )
        .await;
    }

    Ok(relation)
}

/// Delete a relation by ID.
///
/// Verifies the relation belongs to the specified workspace before deleting.
pub async fn delete_relation(
    db: &DbPool,
    relation_id: &str,
    workspace_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let result = trakkt_core::db_execute!(
        db,
        "DELETE FROM issue_relations WHERE relation_id = $1 AND workspace_id = $2",
        relation_id,
        workspace_id
    )?;

    if result.rows_affected() == 0 {
        return Err(trakkt_core::Error::NotFound(
            "Relation not found".to_string(),
        ));
    }

    // Sync log — best-effort.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        entity_types::ISSUE_RELATION,
        relation_id,
        workspace_id,
        SyncActionType::Delete,
        None,
    )
    .await
    {
        tracing::warn!(error = %e, relation_id = %relation_id, "Failed to write sync log for relation delete");
    }

    // WebSocket broadcast — delete has no entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::ISSUE_RELATION,
            relation_id,
            SyncActionType::Delete,
            None,
        )
        .await;
    }

    Ok(())
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
            CASE \
                WHEN r.source_issue_id = $1 AND r.relation_type = 'blocks' THEN 'blocks' \
                WHEN r.target_issue_id = $1 AND r.relation_type = 'blocks' THEN 'blocked_by' \
                WHEN r.source_issue_id = $1 AND r.relation_type = 'parent' THEN 'parent' \
                WHEN r.target_issue_id = $1 AND r.relation_type = 'parent' THEN 'child_of' \
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
