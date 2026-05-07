// SPDX-License-Identifier: AGPL-3.0-or-later

//! WASM-safe DTOs for the Trakkt issue tracker.
//!
//! These structs are the wire format shared between server and UI. They use
//! only serde (no sqlx) so the trakkt-ui crate can compile them to WASM.
//!
//! Timestamps and optional foreign keys are `String` / `Option<String>` because
//! both Postgres and SQLite serialize timestamps as text through sqlx, and
//! keeping them as strings avoids pulling in chrono on the WASM side.

use serde::{Deserialize, Serialize};

/// A team within a workspace (e.g. "Engineering", "Design").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub team_id: String,
    pub workspace_id: String,
    pub name: String,
    pub key: String,
    pub created_at: String,
}

/// An issue (task / bug / story) within a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub issue_id: String,
    pub workspace_id: String,
    pub team_id: String,
    pub number: i32,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: i32,
    pub assignee_id: Option<String>,
    pub creator_id: String,
    pub due_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Issue with joined details: team key, assignee/creator names, and labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueWithDetails {
    pub issue_id: String,
    pub workspace_id: String,
    pub team_id: String,
    pub team_key: String,
    pub number: i32,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: i32,
    pub assignee_id: Option<String>,
    pub assignee_name: Option<String>,
    pub creator_id: String,
    pub creator_name: Option<String>,
    pub due_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<Label>,
}

/// A workspace-scoped label that can be applied to issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub label_id: String,
    pub workspace_id: String,
    pub name: String,
    pub color: String,
    pub created_at: String,
}

/// A comment on an issue, with optional author metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub comment_id: String,
    pub issue_id: String,
    pub user_id: String,
    pub body: String,
    pub parent_id: Option<String>,
    pub author_name: Option<String>,
    pub author_avatar: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A notification for a user about an issue event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub notification_id: String,
    pub workspace_id: String,
    pub user_id: String,
    pub issue_id: String,
    pub notification_type: String,
    pub read: bool,
    pub issue_title: Option<String>,
    pub issue_number: Option<i32>,
    pub created_at: String,
}

/// Filter criteria for listing issues.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueFilters {
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub assignee_id: Option<String>,
    pub label_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Parameters for creating a new issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIssueParams {
    pub workspace_id: String,
    pub team_id: String,
    pub creator_id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
    pub assignee_id: Option<String>,
    pub due_date: Option<String>,
    pub label_ids: Vec<String>,
}

/// Fields that can be updated on an issue.
///
/// For clearable fields (description, assignee_id, due_date), use double-Option:
/// - `None` = no change
/// - `Some(None)` = clear the field (set to NULL)
/// - `Some(Some(value))` = set to new value
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueUpdate {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub assignee_id: Option<Option<String>>,
    pub due_date: Option<Option<String>>,
}
