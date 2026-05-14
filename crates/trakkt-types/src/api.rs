// SPDX-License-Identifier: AGPL-3.0-or-later

//! API parameter structs shared between MCP and REST surfaces.
//!
//! Each operation defines its input parameters here so that schema generation
//! (via `schemars::JsonSchema`) and deserialization work identically regardless
//! of the transport.

use serde::Deserialize;

// ─────────────────────────────────────────────────────────────────────────────
// Issue operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing issues with optional filters.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListIssuesApiParams {
    /// Filter by team key (e.g. 'TRA')
    pub team_key: Option<String>,
    /// Filter by team ID
    pub team_id: Option<String>,
    /// Filter by status ID
    pub status_id: Option<String>,
    /// Comma-separated status categories: backlog, unstarted, started, completed, cancelled
    pub status_category: Option<String>,
    /// If true, include completed and cancelled issues
    pub include_closed: Option<bool>,
    /// Filter by priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low
    pub priority: Option<i32>,
    /// Filter by assignee user ID
    pub assignee: Option<String>,
    /// Filter by label ID(s). Comma-separated for multiple (OR logic)
    pub label: Option<String>,
    /// Search text to match against issue titles
    pub search: Option<String>,
    /// Maximum number of issues to return (default: 50, max: 100)
    pub limit: Option<i64>,
}

/// Parameters for getting a single issue by identifier.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetIssueApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key. Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team
    pub issue_number: Option<i64>,
}

/// Parameters for creating a new issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreateIssueApiParams {
    /// Issue title (required)
    pub title: String,
    /// Team key to assign issue to
    pub team_key: Option<String>,
    /// Team ID to assign issue to
    pub team_id: Option<String>,
    /// Markdown description
    pub description: Option<String>,
    /// Priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low
    pub priority: Option<i32>,
    /// User ID to assign
    pub assignee: Option<String>,
    /// Array of label IDs
    pub labels: Option<Vec<String>>,
    /// Due date in ISO 8601 format (YYYY-MM-DD)
    pub due_date: Option<String>,
    /// Project ID to associate with
    pub project_id: Option<String>,
    /// Milestone ID to associate with
    pub milestone_id: Option<String>,
    /// Parent issue ID for sub-issues
    pub parent_issue_id: Option<String>,
}

/// Parameters for updating an existing issue.
///
/// For clearable fields, use double-Option:
/// - Field absent from JSON = no change (`None`)
/// - Field set to `null` = clear the field (`Some(None)`)
/// - Field set to a value = update the field (`Some(Some(value))`)
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct UpdateIssueApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key. Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team
    pub issue_number: Option<i64>,
    /// New title for the issue
    pub title: Option<String>,
    /// New markdown description, or null to clear
    pub description: Option<Option<String>>,
    /// New status ID
    pub status_id: Option<String>,
    /// New priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low
    pub priority: Option<i32>,
    /// User ID to assign, or null to unassign
    pub assignee: Option<Option<String>>,
    /// Replace all labels with this list of label IDs
    pub labels: Option<Vec<String>>,
    /// Due date in ISO 8601 format, or null to clear
    pub due_date: Option<Option<String>>,
    /// Team key to move the issue to
    pub move_to_team_key: Option<String>,
    /// Team ID to move the issue to
    pub move_to_team_id: Option<String>,
    /// Project ID, or null to clear
    pub project_id: Option<Option<String>>,
    /// Milestone ID, or null to clear
    pub milestone_id: Option<Option<String>>,
    /// Parent issue ID, or null to clear
    pub parent_issue_id: Option<Option<String>>,
    /// Sort order, or null to clear
    pub sort_order: Option<Option<f64>>,
}

/// Parameters for deleting an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DeleteIssueApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key. Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team
    pub issue_number: Option<i64>,
}

/// Parameters for searching issues by text query.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SearchIssuesApiParams {
    /// Search text (required)
    pub query: String,
    /// Filter by team key
    pub team_key: Option<String>,
    /// Filter by team ID
    pub team_id: Option<String>,
    /// If true, include completed and cancelled issues
    pub include_closed: Option<bool>,
    /// Max results (default: 20, max: 100)
    pub limit: Option<i64>,
}
