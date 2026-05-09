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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub team_id: String,
    pub workspace_id: String,
    pub name: String,
    pub key: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub created_at: String,
}

/// A member of an issue-tracker team (team_members join table).
///
/// Distinct from workspace membership — this tracks which users belong to
/// specific teams within a workspace, for defaults and notification routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueTeamMember {
    pub team_id: String,
    pub user_id: String,
    pub user_name: Option<String>,
    pub user_email: String,
    pub role: String,
    pub created_at: String,
}

/// A first-class status with category grouping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub status_id: String,
    pub workspace_id: String,
    pub team_id: Option<String>,
    pub name: String,
    pub category: String,
    pub position: i32,
    pub color: Option<String>,
    pub created_at: String,
}

/// An issue (task / bug / story) within a team.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub issue_id: String,
    pub workspace_id: String,
    pub team_id: String,
    pub number: i32,
    pub title: String,
    pub description: Option<String>,
    pub status_id: String,
    pub priority: i32,
    pub assignee_id: Option<String>,
    pub creator_id: String,
    pub due_date: Option<String>,
    pub project_id: Option<String>,
    pub milestone_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Issue with joined details: team key, assignee/creator names, and labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueWithDetails {
    pub issue_id: String,
    pub workspace_id: String,
    pub team_id: String,
    pub team_key: String,
    pub number: i32,
    pub title: String,
    pub description: Option<String>,
    pub status_id: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: i32,
    pub assignee_id: Option<String>,
    pub assignee_name: Option<String>,
    pub creator_id: String,
    pub creator_name: Option<String>,
    pub due_date: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub milestone_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<Label>,
}

/// A label that can be applied to issues.
///
/// Labels are either workspace-scoped (`team_id = None`, available to all teams)
/// or team-scoped (`team_id = Some(...)`, only available within that team).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub label_id: String,
    pub workspace_id: String,
    pub team_id: Option<String>,
    pub name: String,
    pub color: String,
    pub created_at: String,
}

/// A project within a workspace (e.g. "Q3 Launch", "Mobile App").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub project_id: String,
    pub workspace_id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub status: String,
    pub lead_id: Option<String>,
    pub lead_name: Option<String>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub sort_order: f64,
    pub created_at: String,
    pub updated_at: String,
}

/// A member of a project with a specific role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMember {
    pub project_id: String,
    pub user_id: String,
    pub role: String,
    pub created_at: String,
}

/// A milestone within a project — a target checkpoint with a due date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMilestone {
    pub milestone_id: String,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub target_date: Option<String>,
    pub sort_order: i32,
    pub created_at: String,
}

/// A periodic status update on a project's health.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectUpdate {
    pub update_id: String,
    pub project_id: String,
    pub user_id: String,
    pub health: String,
    pub body: Option<String>,
    pub created_at: String,
}

/// Progress summary for a project — computed from issue status categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectProgress {
    pub total: i64,
    pub completed: i64,
    pub cancelled: i64,
    pub percent_done: f64,
}

/// A comment on an issue, with optional author metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub status_id: Option<String>,
    pub priority: Option<i32>,
    pub assignee_id: Option<String>,
    pub label_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Parameters for creating a new issue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub project_id: Option<String>,
    pub milestone_id: Option<String>,
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
    pub status_id: Option<String>,
    pub priority: Option<i32>,
    pub assignee_id: Option<Option<String>>,
    pub due_date: Option<Option<String>>,
    pub project_id: Option<Option<String>>,
    pub milestone_id: Option<Option<String>>,
}
