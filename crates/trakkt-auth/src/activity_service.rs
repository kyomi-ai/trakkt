// SPDX-License-Identifier: AGPL-3.0-or-later

//! Activity service — records and retrieves issue activity history.
//!
//! Every field change, status transition, or significant action on an issue
//! is recorded as an `IssueActivity` row. The `ActivityRecorder` struct
//! provides a convenient API for recording individual actions or diffing
//! entire issue snapshots to detect all changes at once.

use std::hash::{DefaultHasher, Hash, Hasher};

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;
use trakkt_types::models::{IssueActivity, IssueWithDetails, WorkspaceActivity};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct IssueActivityRow {
    activity_id: String,
    issue_id: String,
    workspace_id: String,
    actor_id: String,
    actor_name: Option<String>,
    action_type: String,
    field: Option<String>,
    old_value: Option<String>,
    new_value: Option<String>,
    metadata: Option<String>,
    action_source: String,
    action_source_label: Option<String>,
    created_at: String,
}

impl IssueActivityRow {
    fn into_dto(self) -> IssueActivity {
        IssueActivity {
            activity_id: self.activity_id,
            issue_id: self.issue_id,
            workspace_id: self.workspace_id,
            actor_id: self.actor_id,
            actor_name: self.actor_name,
            action_type: self.action_type,
            field: self.field,
            old_value: self.old_value,
            new_value: self.new_value,
            metadata: self.metadata,
            action_source: self.action_source
                .parse::<ActionSource>()
                .unwrap_or_else(|_| {
                    tracing::warn!(raw = %self.action_source, "Unknown action_source value; defaulting to User");
                    ActionSource::User
                }),
            action_source_label: self.action_source_label,
            created_at: self.created_at,
        }
    }
}

// ─── Snapshot type ───────────────────────────────────────────────────────────

/// A label as captured in an issue snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotLabel {
    pub label_id: String,
    pub name: String,
    pub color: String,
}

/// A point-in-time snapshot of issue fields relevant to activity tracking.
///
/// Capture one before an update and one after to pass to
/// [`ActivityRecorder::record_issue_diff`].
#[derive(Debug, Clone)]
pub struct IssueSnapshot {
    pub status_id: String,
    pub status_name: String,
    pub priority: i32,
    pub assignee_id: Option<String>,
    pub assignee_name: Option<String>,
    pub title: String,
    pub description_hash: Option<u64>,
    pub estimate: Option<i32>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub milestone_id: Option<String>,
    pub milestone_name: Option<String>,
    pub parent_issue_id: Option<String>,
    pub parent_identifier: Option<String>,
    pub due_date: Option<String>,
    pub labels: Vec<SnapshotLabel>,
}

impl IssueSnapshot {
    /// Build a snapshot from an `IssueWithDetails` struct returned by `get_issue`
    /// or `get_issue_by_id`. Fields not available on the detail struct
    /// (milestone_name, parent_issue_id) default to `None`.
    pub fn from_issue_with_details(issue: &IssueWithDetails) -> Self {
        let description_hash = issue.description.as_ref().map(|d| {
            let mut h = DefaultHasher::new();
            d.hash(&mut h);
            h.finish()
        });

        let labels = issue
            .labels
            .iter()
            .map(|l| SnapshotLabel {
                label_id: l.label_id.clone(),
                name: l.name.clone(),
                color: l.color.clone(),
            })
            .collect();

        Self {
            status_id: issue.status_id.clone(),
            status_name: issue.status_name.clone(),
            priority: issue.priority,
            assignee_id: issue.assignee_id.clone(),
            assignee_name: issue.assignee_name.clone(),
            title: issue.title.clone(),
            description_hash,
            estimate: issue.estimate,
            project_id: issue.project_id.clone(),
            project_name: issue.project_name.clone(),
            milestone_id: issue.milestone_id.clone(),
            milestone_name: None,
            parent_issue_id: None,
            parent_identifier: issue.parent_identifier.clone(),
            due_date: issue.due_date.clone(),
            labels,
        }
    }
}

// ─── Priority helper ─────────────────────────────────────────────────────────

/// Maps a numeric priority value to its display label.
pub fn priority_label(priority: i32) -> &'static str {
    match priority {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "No priority",
    }
}

// ─── Params struct for record_field_change ───────────────────────────────────

/// Parameters for recording a single field change activity.
pub struct FieldChangeParams<'a> {
    pub issue_id: &'a str,
    pub action_type: &'a str,
    pub field: &'a str,
    pub old_value: Option<&'a str>,
    pub new_value: Option<&'a str>,
    pub metadata: Option<&'a serde_json::Value>,
}

// ─── ActivityRecorder ────────────────────────────────────────────────────────

/// Records activity entries for a specific workspace and actor.
///
/// Create one at the start of a request/operation and use its methods to
/// record all activities generated during that operation.
pub struct ActivityRecorder<'a> {
    db: &'a DbPool,
    workspace_id: &'a str,
    actor_id: &'a str,
    action_source: ActionSource,
    action_source_label: Option<String>,
    ws_manager: Option<&'a WebSocketManager>,
}

impl<'a> ActivityRecorder<'a> {
    /// Create a new recorder bound to a workspace and actor.
    ///
    /// The optional `ws_manager` enables real-time WebSocket broadcast of
    /// activity events. Pass `None` in tests or contexts without a live
    /// WebSocket server.
    pub fn new(
        db: &'a DbPool,
        workspace_id: &'a str,
        actor_id: &'a str,
        action_source: ActionSource,
        action_source_label: Option<String>,
        ws_manager: Option<&'a WebSocketManager>,
    ) -> Self {
        Self {
            db,
            workspace_id,
            actor_id,
            action_source,
            action_source_label,
            ws_manager,
        }
    }

    /// Record a simple action with optional metadata (no field change).
    pub async fn record(
        &self,
        issue_id: &str,
        action_type: &str,
        metadata: Option<&serde_json::Value>,
    ) -> trakkt_core::Result<()> {
        self.insert_activity(issue_id, action_type, None, None, None, metadata)
            .await
    }

    /// Record a single field change with old/new values and optional metadata.
    pub async fn record_field_change(
        &self,
        params: &FieldChangeParams<'_>,
    ) -> trakkt_core::Result<()> {
        self.insert_activity(
            params.issue_id,
            params.action_type,
            Some(params.field),
            params.old_value,
            params.new_value,
            params.metadata,
        )
        .await
    }

    /// Compare two issue snapshots and record an activity for each difference.
    pub async fn record_issue_diff(
        &self,
        issue_id: &str,
        before: &IssueSnapshot,
        after: &IssueSnapshot,
    ) -> trakkt_core::Result<()> {
        // Status
        if before.status_id != after.status_id {
            let meta = serde_json::json!({
                "old_status_id": before.status_id,
                "new_status_id": after.status_id,
            });
            self.insert_activity(
                issue_id,
                "status_changed",
                Some("status"),
                Some(&before.status_name),
                Some(&after.status_name),
                Some(&meta),
            )
            .await?;
        }

        // Priority
        if before.priority != after.priority {
            let meta = serde_json::json!({
                "old_priority": before.priority,
                "new_priority": after.priority,
            });
            self.insert_activity(
                issue_id,
                "priority_changed",
                Some("priority"),
                Some(priority_label(before.priority)),
                Some(priority_label(after.priority)),
                Some(&meta),
            )
            .await?;
        }

        // Assignee
        if before.assignee_id != after.assignee_id {
            let meta = serde_json::json!({
                "old_assignee_id": before.assignee_id,
                "new_assignee_id": after.assignee_id,
            });
            self.insert_activity(
                issue_id,
                "assignee_changed",
                Some("assignee"),
                before.assignee_name.as_deref(),
                after.assignee_name.as_deref(),
                Some(&meta),
            )
            .await?;
        }

        // Title
        if before.title != after.title {
            self.insert_activity(
                issue_id,
                "title_changed",
                Some("title"),
                Some(&before.title),
                Some(&after.title),
                None,
            )
            .await?;
        }

        // Description (hash-based detection, no content stored)
        if before.description_hash != after.description_hash {
            self.insert_activity(
                issue_id,
                "description_changed",
                Some("description"),
                None,
                None,
                None,
            )
            .await?;
        }

        // Estimate
        if before.estimate != after.estimate {
            let old_str = before
                .estimate
                .map(|e| format!("{e} Points"));
            let new_str = after
                .estimate
                .map(|e| format!("{e} Points"));
            self.insert_activity(
                issue_id,
                "estimate_changed",
                Some("estimate"),
                old_str.as_deref(),
                new_str.as_deref(),
                None,
            )
            .await?;
        }

        // Project
        if before.project_id != after.project_id {
            let meta = serde_json::json!({
                "old_project_id": before.project_id,
                "new_project_id": after.project_id,
            });
            self.insert_activity(
                issue_id,
                "project_changed",
                Some("project"),
                before.project_name.as_deref(),
                after.project_name.as_deref(),
                Some(&meta),
            )
            .await?;
        }

        // Milestone
        if before.milestone_id != after.milestone_id {
            let meta = serde_json::json!({
                "old_milestone_id": before.milestone_id,
                "new_milestone_id": after.milestone_id,
            });
            self.insert_activity(
                issue_id,
                "milestone_changed",
                Some("milestone"),
                before.milestone_name.as_deref(),
                after.milestone_name.as_deref(),
                Some(&meta),
            )
            .await?;
        }

        // Parent issue — compare by identifier since IssueWithDetails
        // doesn't expose parent_issue_id directly
        if before.parent_identifier != after.parent_identifier {
            let meta = serde_json::json!({
                "old_parent_identifier": before.parent_identifier,
                "new_parent_identifier": after.parent_identifier,
            });
            self.insert_activity(
                issue_id,
                "parent_changed",
                Some("parent"),
                before.parent_identifier.as_deref(),
                after.parent_identifier.as_deref(),
                Some(&meta),
            )
            .await?;
        }

        // Due date
        if before.due_date != after.due_date {
            self.insert_activity(
                issue_id,
                "due_date_changed",
                Some("due_date"),
                before.due_date.as_deref(),
                after.due_date.as_deref(),
                None,
            )
            .await?;
        }

        // Labels — diff the sets
        self.diff_labels(issue_id, &before.labels, &after.labels)
            .await?;

        Ok(())
    }

    // ─── Private helpers ─────────────────────────────────────────────────

    /// Insert a single activity row.
    async fn insert_activity(
        &self,
        issue_id: &str,
        action_type: &str,
        field: Option<&str>,
        old_value: Option<&str>,
        new_value: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> trakkt_core::Result<()> {
        let is_pg = self.db.is_postgres();
        let now = sql_compat::now(is_pg);
        let activity_id = uuid::Uuid::new_v4().to_string();

        let metadata_str: Option<String> = metadata
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| {
                trakkt_core::Error::Internal(format!(
                    "failed to serialise activity metadata: {e}"
                ))
            })?;

        let json_cast = sql_compat::cast_to_json(is_pg, "$9");
        let action_source_str = self.action_source.as_str();
        let sql = format!(
            "INSERT INTO issue_activities \
                (activity_id, issue_id, workspace_id, actor_id, action_type, field, old_value, new_value, metadata, action_source, action_source_label, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {json_cast}, $10, $11, {now})",
        );

        trakkt_core::db_execute!(
            self.db,
            &sql,
            &activity_id,
            issue_id,
            self.workspace_id,
            self.actor_id,
            action_type,
            field,
            old_value,
            new_value,
            metadata_str,
            action_source_str,
            self.action_source_label.as_deref()
        )?;

        // Sync log entry — best-effort, log on failure.
        if let Err(e) = sync_log_service::write_sync_entry(
            self.db,
            entity_types::ACTIVITY,
            &activity_id,
            self.workspace_id,
            SyncActionType::Insert,
            None,
        )
        .await
        {
            tracing::warn!(
                error = %e,
                activity_id = %activity_id,
                "Failed to write sync log entry for activity"
            );
        }

        // Broadcast to workspace via WebSocket — best-effort.
        if let Some(ws) = self.ws_manager {
            sync_log_service::broadcast_sync_action(
                ws,
                self.workspace_id,
                entity_types::ACTIVITY,
                &activity_id,
                SyncActionType::Insert,
                None,
            )
            .await;
        }

        Ok(())
    }

    /// Diff label sets and record label_added / label_removed for each difference.
    async fn diff_labels(
        &self,
        issue_id: &str,
        before: &[SnapshotLabel],
        after: &[SnapshotLabel],
    ) -> trakkt_core::Result<()> {
        // Labels removed: present in before but not in after
        for label in before {
            if !after.iter().any(|l| l.label_id == label.label_id) {
                let meta = serde_json::json!({
                    "label_id": label.label_id,
                    "color": label.color,
                });
                self.insert_activity(
                    issue_id,
                    "label_removed",
                    Some("labels"),
                    Some(&label.name),
                    None,
                    Some(&meta),
                )
                .await?;
            }
        }

        // Labels added: present in after but not in before
        for label in after {
            if !before.iter().any(|l| l.label_id == label.label_id) {
                let meta = serde_json::json!({
                    "label_id": label.label_id,
                    "color": label.color,
                });
                self.insert_activity(
                    issue_id,
                    "label_added",
                    Some("labels"),
                    None,
                    Some(&label.name),
                    Some(&meta),
                )
                .await?;
            }
        }

        Ok(())
    }
}

// ─── Query functions ─────────────────────────────────────────────────────────

/// Fetch all activities for an issue, ordered by creation time (oldest first).
pub async fn list_issue_activities(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<Vec<IssueActivity>> {
    let rows: Vec<IssueActivityRow> = trakkt_core::db_fetch_all!(
        db,
        IssueActivityRow,
        "SELECT a.activity_id, a.issue_id, a.workspace_id, a.actor_id, \
                u.name AS actor_name, \
                a.action_type, a.field, a.old_value, a.new_value, \
                CAST(a.metadata AS TEXT) AS metadata, \
                a.action_source, a.action_source_label, \
                CAST(a.created_at AS TEXT) AS created_at \
         FROM issue_activities a \
         LEFT JOIN users u ON u.user_id = a.actor_id \
         WHERE a.issue_id = $1 \
         ORDER BY a.created_at ASC",
        issue_id
    )?;
    Ok(rows.into_iter().map(IssueActivityRow::into_dto).collect())
}

// ─── Workspace-level activity query ────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct WorkspaceActivityRow {
    activity_id: String,
    issue_id: String,
    workspace_id: String,
    actor_id: String,
    actor_name: Option<String>,
    action_type: String,
    field: Option<String>,
    old_value: Option<String>,
    new_value: Option<String>,
    metadata: Option<String>,
    action_source: String,
    action_source_label: Option<String>,
    created_at: String,
    team_key: String,
    issue_number: i32,
    issue_title: String,
}

impl WorkspaceActivityRow {
    fn into_dto(self) -> WorkspaceActivity {
        WorkspaceActivity {
            activity_id: self.activity_id,
            issue_id: self.issue_id,
            workspace_id: self.workspace_id,
            actor_id: self.actor_id,
            actor_name: self.actor_name,
            action_type: self.action_type,
            field: self.field,
            old_value: self.old_value,
            new_value: self.new_value,
            metadata: self.metadata,
            action_source: self.action_source
                .parse::<ActionSource>()
                .unwrap_or_else(|_| {
                    tracing::warn!(raw = %self.action_source, "Unknown action_source value; defaulting to User");
                    ActionSource::User
                }),
            action_source_label: self.action_source_label,
            created_at: self.created_at,
            team_key: self.team_key,
            issue_number: self.issue_number,
            issue_title: self.issue_title,
        }
    }
}

/// Fetch activities across all teams in a workspace, ordered by most recent first.
///
/// Optional filters narrow results by team key or action type. The nullable
/// parameter pattern (`$N IS NULL OR column = $N`) keeps bind positions fixed
/// regardless of which filters are active.
pub async fn list_workspace_activities(
    db: &DbPool,
    workspace_id: &str,
    team_key: Option<&str>,
    action_type: Option<&str>,
    limit: i64,
    offset: i64,
) -> trakkt_core::Result<Vec<WorkspaceActivity>> {
    let is_pg = db.is_postgres();
    let cast_text = if is_pg { "::TEXT" } else { "" };

    // Inline LIMIT/OFFSET per CODING_STANDARDS.md (sanitized i64 values).
    let sql = format!(
        "SELECT a.activity_id, a.issue_id, a.workspace_id, a.actor_id, \
                u.name AS actor_name, \
                a.action_type, a.field, a.old_value, a.new_value, \
                CAST(a.metadata AS TEXT) AS metadata, \
                a.action_source, a.action_source_label, \
                CAST(a.created_at AS TEXT) AS created_at, \
                t.key AS team_key, \
                i.number AS issue_number, \
                i.title AS issue_title \
         FROM issue_activities a \
         LEFT JOIN users u ON u.user_id = a.actor_id \
         JOIN issues i ON i.issue_id = a.issue_id \
         JOIN teams t ON t.team_id = i.team_id \
         WHERE a.workspace_id = $1 \
           AND ($2{cast_text} IS NULL OR t.key = $2) \
           AND ($3{cast_text} IS NULL OR a.action_type = $3) \
         ORDER BY a.created_at DESC \
         LIMIT {limit} OFFSET {offset}"
    );

    let rows: Vec<WorkspaceActivityRow> = trakkt_core::db_fetch_all!(
        db,
        WorkspaceActivityRow,
        &sql,
        workspace_id,
        team_key,
        action_type
    )?;
    Ok(rows.into_iter().map(WorkspaceActivityRow::into_dto).collect())
}
