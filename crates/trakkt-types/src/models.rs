// SPDX-License-Identifier: AGPL-3.0-or-later

//! WASM-safe DTOs for the Trakkt issue tracker.
//!
//! These structs are the wire format shared between server and UI. They use
//! only serde (no sqlx) so the trakkt-ui crate can compile them to WASM.
//!
//! Timestamp fields are being migrated from `String` to `chrono::DateTime<Utc>`.
//! `Comment` uses typed timestamps; other structs still use `String` and will
//! be migrated incrementally.

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
    pub icon_type: Option<String>,
    pub icon_name: Option<String>,
    pub icon_color: Option<String>,
    pub member_count: i64,
    pub settings: Option<TeamSettings>,
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
    pub estimate: Option<i32>,
    pub sort_order: Option<f64>,
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
    pub estimate: Option<i32>,
    /// The parent issue's identifier (e.g. "ENG-42"), derived from a subquery
    /// on the `issue_relations` table. `None` if this issue has no parent.
    pub parent_identifier: Option<String>,
    pub parent_title: Option<String>,
    pub sort_order: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
    /// Whether this issue has any child issues (via `issue_relations` parent type).
    pub has_children: bool,
    /// Whether this issue is blocked by another issue (target of a `blocks` relation).
    pub is_blocked: bool,
    /// Whether this issue blocks another issue (source of a `blocks` relation).
    pub is_blocking: bool,
    /// Whether this issue has any relations at all (source or target).
    pub has_relations: bool,
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

/// A user-pinned favorite (team, project, or view) for quick sidebar access.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Favorite {
    pub favorite_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub target_type: String,
    pub target_id: String,
    pub sort_order: f64,
    pub created_at: String,
}

/// A saved view — a named set of filters and display options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub view_id: String,
    pub workspace_id: String,
    pub team_id: Option<String>,
    pub created_by: String,
    pub name: String,
    pub icon: Option<String>,
    pub filters: String,
    pub display_options: String,
    pub sort_order: f64,
    pub position: i32,
    pub is_shared: bool,
    pub created_at: String,
    pub updated_at: String,
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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
    pub team_key: Option<String>,
    pub actor_id: Option<String>,
    pub actor_name: Option<String>,
    pub created_at: String,
}

/// Filter criteria for listing issues.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueFilters {
    pub status_id: Option<String>,
    pub status_categories: Option<Vec<String>>,
    pub exclude_status_categories: Option<Vec<String>>,
    pub priority: Option<i32>,
    pub assignee_id: Option<String>,
    pub creator_id: Option<String>,
    pub label_ids: Option<Vec<String>>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub include_archived: Option<bool>,
    /// When true, return ONLY archived issues (archived_at IS NOT NULL).
    /// Takes precedence over `include_archived`.
    pub only_archived: Option<bool>,
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
    pub estimate: Option<i32>,
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
    pub estimate: Option<Option<i32>>,
    pub sort_order: Option<Option<f64>>,
    pub team_id: Option<String>,
}

/// A relation between two issues (e.g. "blocks").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueRelation {
    pub relation_id: String,
    pub workspace_id: String,
    pub source_issue_id: String,
    pub target_issue_id: String,
    pub relation_type: String,
    pub created_by: Option<String>,
    pub created_at: String,
}

/// A relation with joined issue details for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueRelationWithDetails {
    pub relation_id: String,
    pub relation_type: String,
    pub issue_id: String,
    pub team_key: String,
    pub number: i32,
    pub title: String,
    pub status_category: String,
    pub direction: String,
}

/// A single activity entry for an issue (field change, status transition, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueActivity {
    pub activity_id: String,
    pub issue_id: String,
    pub workspace_id: String,
    pub actor_id: String,
    pub actor_name: Option<String>,
    pub action_type: String,
    pub field: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub metadata: Option<String>,
    pub created_at: String,
}

// ─── Estimate types ────────────────────────────────────────────────────────

/// The scale used for issue point estimation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimateScale {
    Exponential,
    Fibonacci,
    Linear,
    TShirt,
}

/// A single option in an estimate scale (value + display label).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EstimateOption {
    pub value: i32,
    pub label: String,
}

impl EstimateScale {
    /// Generate the list of estimate options for this scale.
    ///
    /// - `extended`: include the extended range (larger values).
    /// - `allow_zero`: prepend a "No estimate" option with value 0.
    pub fn options(&self, extended: bool, allow_zero: bool) -> Vec<EstimateOption> {
        let mut opts = Vec::new();
        if allow_zero {
            opts.push(EstimateOption { value: 0, label: self.format_label(0) });
        }
        let base: Vec<i32> = match self {
            Self::Exponential => vec![1, 2, 4, 8, 16],
            Self::Fibonacci => vec![1, 2, 3, 5, 8],
            Self::Linear => vec![1, 2, 3, 4, 5],
            Self::TShirt => vec![1, 2, 3, 4, 5],
        };
        let ext: Vec<i32> = match self {
            Self::Exponential => vec![32, 64],
            Self::Fibonacci => vec![13, 21],
            Self::Linear => vec![6, 7, 8, 9, 10],
            Self::TShirt => vec![6],
        };
        for v in &base {
            opts.push(EstimateOption { value: *v, label: self.format_label(*v) });
        }
        if extended {
            for v in &ext {
                opts.push(EstimateOption { value: *v, label: self.format_label(*v) });
            }
        }
        opts
    }

    /// Format a numeric value as a display label for this scale.
    pub fn format_label(&self, value: i32) -> String {
        match self {
            Self::TShirt => match value {
                0 => "No estimate",
                1 => "XS",
                2 => "S",
                3 => "M",
                4 => "L",
                5 => "XL",
                6 => "XXL",
                _ => "?",
            }
            .to_string(),
            _ => match value {
                0 => "No estimate".to_string(),
                1 => "1 Point".to_string(),
                n => format!("{n} Points"),
            },
        }
    }

    /// Human-readable name of this scale.
    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Exponential => "Exponential",
            Self::Fibonacci => "Fibonacci",
            Self::Linear => "Linear",
            Self::TShirt => "T-Shirt",
        }
    }

    /// Short preview string showing the scale values.
    pub fn preview(&self) -> &'static str {
        match self {
            Self::Exponential => "1, 2, 4, 8, 16 Points",
            Self::Fibonacci => "1, 2, 3, 5, 8 Points",
            Self::Linear => "1, 2, 3, 4, 5 Points",
            Self::TShirt => "XS, S, M, L, XL",
        }
    }
}

// ─── Team settings ─────────────────────────────────────────────────────────

/// Per-team settings stored in teams.settings JSON column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TeamSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_archive_days: Option<u32>,
    /// Which estimation scale this team uses. `None` means estimates are disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate_scale: Option<EstimateScale>,
    /// Whether to include a "0 / No estimate" option in the picker.
    #[serde(default)]
    pub estimate_allow_zero: bool,
    /// Whether to show extended range values (larger point values).
    #[serde(default)]
    pub estimate_extended: bool,
    /// Whether unestimated issues count toward velocity/capacity totals.
    #[serde(default = "default_true")]
    pub estimate_count_unestimated: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TeamSettings {
    fn default() -> Self {
        Self {
            auto_archive_days: None,
            estimate_scale: None,
            estimate_allow_zero: false,
            estimate_extended: false,
            estimate_count_unestimated: true,
        }
    }
}

/// Workspace-level settings stored in workspaces.settings JSON column.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_auto_archive_days: Option<u32>,
}

/// A file attachment linked to an issue.
///
/// WASM-safe serializable DTO for file attachment metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub attachment_id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub created_at: String,
}
