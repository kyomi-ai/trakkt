// SPDX-License-Identifier: AGPL-3.0-or-later

//! Activity service — records and retrieves issue activity history.
//!
//! Every field change, status transition, or significant action on an issue
//! is recorded as an `IssueActivity` row. The `ActivityRecorder` struct
//! provides a convenient API for recording individual actions or diffing
//! entire issue snapshots to detect all changes at once.

use std::hash::{DefaultHasher, Hash, Hasher};

use trakkt_core::db::DbTx;
use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;
use trakkt_types::models::{IssueActivity, IssueWithDetails, WorkspaceActivity};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row type ────────────────────────────────────────────────────────────────

const DESCRIPTION_COALESCE_WINDOW_SECS: i64 = 60;

#[derive(sqlx::FromRow)]
struct CoalesceRow {
    activity_id: String,
}

#[derive(sqlx::FromRow)]
struct IssueActivityRow {
    activity_id: String,
    issue_id: String,
    workspace_id: String,
    actor_id: Option<String>,
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
///
/// # Why the recorder owns its transaction
///
/// TRA-9923 requires a `sync_log` row to commit atomically with the mutation it
/// describes. For an activity that mutation is *the activity row itself*, so
/// each activity gets its own transaction: the row and its `sync_log` entry are
/// written on it, it commits, and only then is the change broadcast. A method
/// that records several — [`ActivityRecorder::record_issue_diff`] — runs one
/// such transaction per activity.
///
/// It does not instead take the caller's transaction, because there is no
/// caller transaction to take. Every construction of an `ActivityRecorder`
/// lives in `trakkt-api` or `trakkt-github` (`comments.rs`, `issues.rs`,
/// `relations.rs`, `events.rs`), and `grep -rn '\.begin()'` over
/// `crates/trakkt-api/src/` and `crates/trakkt-github/src/` matches nothing —
/// neither crate opens a transaction anywhere. Each caller calls a service
/// function that opens and commits its own transaction and returns, and only
/// then records the activity describing what already committed.
///
/// The remaining non-atomicity is activity-vs-issue-update, and that is the
/// intended trade. An activity is a derived audit record; a failed activity
/// write must not roll back a legitimate issue update, and every call site
/// already treats recording as best-effort (`if let Err(e) = recorder.record(…)`
/// followed by a log and a continue).
///
/// # Coalescing read visibility
///
/// [`ActivityRecorder::coalesce_or_insert_activity`] runs its lookup on its own
/// transaction, so it sees committed rows only. That is the right set here: a
/// row it could coalesce onto was written by an earlier, separate recorder call
/// that has already committed. Rows written earlier in the *same* logical
/// mutation would additionally be visible if this joined the caller's
/// transaction, but none of them could match. The coalesce predicate matches on
/// `action_type`; the only caller of the coalescing path is the
/// `description_changed` branch of [`ActivityRecorder::record_issue_diff`],
/// which runs at most once per diff; and no other branch of that diff writes
/// `description_changed`.
pub struct ActivityRecorder<'a> {
    db: &'a DbPool,
    workspace_id: &'a str,
    actor_id: Option<&'a str>,
    action_source: ActionSource,
    action_source_label: Option<String>,
    ws_manager: Option<&'a WebSocketManager>,
}

impl<'a> ActivityRecorder<'a> {
    /// Create a new recorder bound to a workspace and a known actor.
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
            actor_id: Some(actor_id),
            action_source,
            action_source_label,
            ws_manager,
        }
    }

    /// Create a new recorder where the actor may be unknown.
    ///
    /// Used for activities sourced from external systems (e.g. GitHub webhook
    /// commits and pull requests) where the author frequently has no matching
    /// Trakkt user. A `None` actor results in a NULL `actor_id` column.
    pub fn new_with_optional_actor(
        db: &'a DbPool,
        workspace_id: &'a str,
        actor_id: Option<&'a str>,
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

        // Description (hash-based detection, no content stored).
        // Uses coalescing to avoid duplicate activity entries from
        // rapid auto-saves (debounce fires every 500ms).
        if before.description_hash != after.description_hash {
            self.coalesce_or_insert_activity(
                issue_id,
                "description_changed",
                Some("description"),
                None,
                None,
                None,
                DESCRIPTION_COALESCE_WINDOW_SECS,
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

    /// Insert a new activity, or update an existing recent one if a matching
    /// activity from the same actor on the same issue already exists within
    /// `coalesce_window_secs`.
    ///
    /// This prevents flooding the activity feed when a field is saved
    /// repeatedly in quick succession (e.g. description auto-save).
    ///
    /// The lookup runs on the same transaction as the write it decides, so the
    /// row it found cannot be deleted, nor a newer one inserted, between the two
    /// — previously they were separate pool round trips. Which branch is taken
    /// is therefore a property of one consistent read, not of whatever the
    /// database happened to hold at two different moments.
    async fn coalesce_or_insert_activity(
        &self,
        issue_id: &str,
        action_type: &str,
        field: Option<&str>,
        old_value: Option<&str>,
        new_value: Option<&str>,
        metadata: Option<&serde_json::Value>,
        coalesce_window_secs: i64,
    ) -> trakkt_core::Result<()> {
        let is_pg = self.db.is_postgres();
        let recent_predicate =
            sql_compat::within_seconds(is_pg, "created_at", coalesce_window_secs);

        let field_predicate = if is_pg {
            "field IS NOT DISTINCT FROM $4"
        } else {
            "field IS $4"
        };

        let find_sql = format!(
            "SELECT activity_id \
             FROM issue_activities \
             WHERE issue_id = $1 \
               AND actor_id = $2 \
               AND action_type = $3 \
               AND {field_predicate} \
               AND {recent_predicate} \
             ORDER BY created_at DESC \
             LIMIT 1"
        );

        let mut tx = self.db.begin().await?;

        let existing: Option<CoalesceRow> = trakkt_core::tx_fetch_optional!(
            &mut tx,
            CoalesceRow,
            &find_sql,
            issue_id,
            self.actor_id,
            action_type,
            field
        )?;

        let (activity_id, action) = match existing {
            Some(row) => {
                // Update the existing row's timestamp to "now".
                let now = sql_compat::now(is_pg);
                let update_sql = format!(
                    "UPDATE issue_activities SET created_at = {now} WHERE activity_id = $1"
                );
                trakkt_core::tx_execute!(&mut tx, &update_sql, &row.activity_id)?;

                (row.activity_id, SyncActionType::Update)
            }
            None => {
                let activity_id = self
                    .insert_activity_row(
                        &mut tx, issue_id, action_type, field, old_value, new_value, metadata,
                    )
                    .await?;

                (activity_id, SyncActionType::Insert)
            }
        };

        // Logs the entry on `tx`, commits, and only then broadcasts — the
        // broadcast resolves its recipients from the pool, which this
        // transaction is holding on SQLite (see `DbTx`).
        sync_log_service::commit_and_deliver(
            tx,
            entity_types::ACTIVITY,
            &activity_id,
            self.workspace_id,
            sync_log_service::SyncAudience::Workspace,
            action,
            None,
            self.ws_manager,
        )
        .await
    }

    /// Insert a single activity row, log it, and broadcast it once committed.
    async fn insert_activity(
        &self,
        issue_id: &str,
        action_type: &str,
        field: Option<&str>,
        old_value: Option<&str>,
        new_value: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> trakkt_core::Result<()> {
        let mut tx = self.db.begin().await?;

        let activity_id = self
            .insert_activity_row(
                &mut tx, issue_id, action_type, field, old_value, new_value, metadata,
            )
            .await?;

        sync_log_service::commit_and_deliver(
            tx,
            entity_types::ACTIVITY,
            &activity_id,
            self.workspace_id,
            sync_log_service::SyncAudience::Workspace,
            SyncActionType::Insert,
            None,
            self.ws_manager,
        )
        .await
    }

    /// Write one `issue_activities` row on an open transaction and return its
    /// generated id.
    ///
    /// The `sync_log` entry, the commit and the broadcast are the caller's:
    /// both callers finish through
    /// [`sync_log_service::commit_and_deliver`], which is what keeps the
    /// delivery strictly after the commit.
    async fn insert_activity_row(
        &self,
        tx: &mut DbTx,
        issue_id: &str,
        action_type: &str,
        field: Option<&str>,
        old_value: Option<&str>,
        new_value: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> trakkt_core::Result<String> {
        let is_pg = tx.is_postgres();
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

        trakkt_core::tx_execute!(
            &mut *tx,
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

        Ok(activity_id)
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
    actor_id: Option<String>,
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
    actor_id: Option<&str>,
    action_source: Option<&str>,
    created_after: Option<&str>,
    created_before: Option<&str>,
    limit: i64,
    offset: i64,
) -> trakkt_core::Result<Vec<WorkspaceActivity>> {
    let is_pg = db.is_postgres();
    let cast_text = if is_pg { "::TEXT" } else { "" };
    let cast_ts = |n: u8| {
        if is_pg {
            format!("${n}::TEXT::TIMESTAMPTZ")
        } else {
            // Normalize to ISO 8601 matching the stored strftime format
            format!("strftime('%Y-%m-%dT%H:%M:%SZ', ${n})")
        }
    };

    // Inline LIMIT/OFFSET per CODING_STANDARDS.md (sanitized i64 values).
    let cast_after = cast_ts(6);
    let cast_before = cast_ts(7);
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
           AND ($4{cast_text} IS NULL OR a.actor_id = $4) \
           AND ($5{cast_text} IS NULL OR a.action_source = $5) \
           AND ($6{cast_text} IS NULL OR a.created_at >= {cast_after}) \
           AND ($7{cast_text} IS NULL OR a.created_at <= {cast_before}) \
         ORDER BY a.created_at DESC \
         LIMIT {limit} OFFSET {offset}"
    );

    let rows: Vec<WorkspaceActivityRow> = trakkt_core::db_fetch_all!(
        db,
        WorkspaceActivityRow,
        &sql,
        workspace_id,
        team_key,
        action_type,
        actor_id,
        action_source,
        created_after,
        created_before
    )?;
    Ok(rows.into_iter().map(WorkspaceActivityRow::into_dto).collect())
}
