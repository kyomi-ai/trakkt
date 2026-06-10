// SPDX-License-Identifier: AGPL-3.0-or-later

//! API parameter structs shared between MCP and REST surfaces.
//!
//! Each operation defines its input parameters here so that schema generation
//! (via `schemars::JsonSchema`) and deserialization work identically regardless
//! of the transport.

use serde::{Deserialize, Serialize};

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
    /// JSON array of composable filter clauses, AND-ed together. Each clause is
    /// `{"field","operator","values"}`.
    /// Fields: status, priority, label, project, is_sub_issue, is_parent,
    /// is_blocked, is_blocking, has_relations.
    /// Operators: any_of, none_of, all_of, not_any_of, not_all_of.
    /// Example: `[{"field":"label","operator":"none_of","values":["label-id-1"]}]`
    pub filters: Option<String>,
}

/// A single composable filter clause: a `(field, operator, values)` triple.
///
/// Used by `list_issues` for post-fetch filtering. Boolean fields (e.g.
/// `is_sub_issue`) ignore `values` — the operator alone determines the match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FilterClause {
    pub field: String,
    pub operator: String,
    #[serde(default)]
    pub values: Vec<String>,
}

/// Wrapper response for `list_issues` that includes truncation metadata.
///
/// When composable filter clauses reduce the result set below the raw DB fetch,
/// callers need to know whether more results exist beyond the returned page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListIssuesResponse {
    pub issues: Vec<crate::models::IssueWithDetails>,
    /// Number of issues that passed all filters within the server's fetch window
    /// (approximately 5× the requested limit). When `truncated` is true, the true
    /// total may exceed this count.
    pub matched_count: usize,
    /// Number of issues actually returned (after limit).
    pub returned_count: usize,
    /// Whether more matching issues exist beyond the returned page.
    pub truncated: bool,
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
    /// Estimate points value (integer)
    pub estimate: Option<i32>,
    /// Relations to create after issue creation. Each entry links the new issue
    /// to an existing issue. Supports directional sugar: "blocked_by" creates a
    /// "blocks" relation with the referenced issue as the blocker.
    pub relations: Option<Vec<InlineRelation>>,
}

/// An inline relation to create alongside a new issue.
///
/// The `issue` field is the target issue identifier (e.g. "TRA-130").
/// The `relation_type` is validated by the service layer against configured types.
/// Directional sugar: "blocked_by" creates a "blocks" relation with the target as blocker.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct InlineRelation {
    /// Target issue identifier (e.g. "TRA-130")
    pub issue: String,
    /// Relation type (e.g. "blocks", "blocked_by", "parent", "duplicate", "relates_to")
    pub relation_type: String,
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
    /// Estimate points value, or null to clear
    pub estimate: Option<Option<i32>>,
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
    /// Include archived issues in results (default: false)
    pub include_archived: Option<bool>,
    /// Also search comment bodies (default: true)
    pub include_comments: Option<bool>,
    /// Max results (default: 20, max: 100)
    pub limit: Option<i64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Comment operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for adding a comment to an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct AddCommentApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team. Required if issue_identifier is not provided
    pub issue_number: Option<i64>,
    /// Markdown body of the comment (required)
    pub body: String,
    /// Parent comment ID for threaded replies
    pub parent_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Label operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing labels in the workspace.
///
/// When `team_id` or `team_key` is provided, returns workspace-level labels
/// (team_id IS NULL) plus labels scoped to that team. When neither is
/// provided, returns all labels in the workspace (current behaviour).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListLabelsApiParams {
    /// Filter by team ID — returns workspace-level + team-scoped labels
    pub team_id: Option<String>,
    /// Filter by team key (e.g. 'TRA') — resolved to team_id server-side
    pub team_key: Option<String>,
}

/// Parameters for creating a new label.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreateLabelApiParams {
    /// Label name (must be unique within the workspace)
    pub name: String,
    /// Hex color code (e.g. '#FF5733' or 'FF5733')
    pub color: String,
    /// Team key to scope the label to a specific team
    pub team_key: Option<String>,
    /// Team ID to scope the label to a specific team
    pub team_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Team operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing teams the authenticated user belongs to.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListTeamsApiParams {}

/// Parameters for updating a team's settings.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct UpdateTeamSettingsApiParams {
    /// Team key (e.g. 'TRA')
    pub team_key: Option<String>,
    /// Team ID
    pub team_id: Option<String>,
    /// New team settings (full replace)
    pub settings: crate::models::TeamSettings,
}

// ─────────────────────────────────────────────────────────────────────────────
// Status operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing statuses in the workspace.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListStatusesApiParams {
    /// Team ID to include team-specific statuses
    pub team_id: Option<String>,
    /// Team key (e.g. 'TRA') as alternative to team_id
    pub team_key: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Relation operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for adding a relation between two issues.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct AddRelationApiParams {
    /// Source issue identifier in 'TRA-35' format. For 'blocks': the blocker. For 'parent': the parent issue. For 'duplicate': the duplicate issue. For 'relates_to': either issue (symmetric).
    /// Optional so REST can inject it from the path parameter.
    pub source_issue: Option<String>,
    /// Target issue identifier in 'TRA-35' format. For 'blocks': the blocked issue. For 'parent': the child issue. For 'duplicate': the original issue. For 'relates_to': either issue (symmetric).
    pub target_issue: String,
    /// Relation type: 'blocks', 'parent', 'duplicate', or 'relates_to'
    pub relation_type: String,
}

/// Parameters for removing a relation by its ID.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct RemoveRelationApiParams {
    /// The relation ID to remove
    pub relation_id: String,
}

/// Parameters for listing all relations for an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListRelationsApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team. Required if issue_identifier is not provided
    pub issue_number: Option<i64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Project operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing all projects in the workspace.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListProjectsApiParams {}

/// Parameters for getting a single project by ID.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetProjectApiParams {
    /// The project ID
    pub project_id: String,
}

/// Parameters for creating a new project.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreateProjectApiParams {
    /// Project name (required)
    pub name: String,
    /// Markdown description of the project
    pub description: Option<String>,
    /// Icon identifier for the project
    pub icon: Option<String>,
    /// Hex color code (e.g. '#0D9488')
    pub color: Option<String>,
    /// User ID to set as project lead
    pub lead_id: Option<String>,
    /// Start date in ISO 8601 format (YYYY-MM-DD)
    pub start_date: Option<String>,
    /// Target completion date in ISO 8601 format (YYYY-MM-DD)
    pub target_date: Option<String>,
}

/// Parameters for updating an existing project.
///
/// For clearable fields, use double-Option:
/// - Field absent from JSON = no change (`None`)
/// - Field set to `null` = clear the field (`Some(None)`)
/// - Field set to a value = update the field (`Some(Some(value))`)
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct UpdateProjectApiParams {
    /// The project ID. Optional so REST can inject it from the path parameter.
    pub project_id: Option<String>,
    /// New project name
    pub name: Option<String>,
    /// New markdown description
    pub description: Option<String>,
    /// New icon identifier
    pub icon: Option<String>,
    /// New hex color code
    pub color: Option<String>,
    /// New project status (e.g. 'planned', 'in_progress', 'paused', 'completed', 'cancelled')
    pub status: Option<String>,
    /// User ID to set as project lead, or null to clear
    pub lead_id: Option<Option<String>>,
    /// Start date in ISO 8601 format, or null to clear
    pub start_date: Option<Option<String>>,
    /// Target date in ISO 8601 format, or null to clear
    pub target_date: Option<Option<String>>,
}

/// Parameters for deleting a project.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DeleteProjectApiParams {
    /// The project ID to delete
    pub project_id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Milestone operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing milestones in a project.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListMilestonesApiParams {
    /// The project ID to list milestones for
    pub project_id: String,
}

/// Parameters for creating a new milestone.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreateMilestoneApiParams {
    /// The project ID to create the milestone in. Optional so REST can inject from path.
    pub project_id: Option<String>,
    /// Milestone name (required)
    pub name: String,
    /// Markdown description of the milestone
    pub description: Option<String>,
    /// Target date in ISO 8601 format (YYYY-MM-DD)
    pub target_date: Option<String>,
}

/// Parameters for updating an existing milestone.
///
/// For clearable fields, use double-Option:
/// - Field absent from JSON = no change (`None`)
/// - Field set to `null` = clear the field (`Some(None)`)
/// - Field set to a value = update the field (`Some(Some(value))`)
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct UpdateMilestoneApiParams {
    /// The milestone ID. Optional so REST can inject from path.
    pub milestone_id: Option<String>,
    /// New milestone name
    pub name: Option<String>,
    /// New markdown description
    pub description: Option<String>,
    /// Target date in ISO 8601 format, or null to clear
    pub target_date: Option<Option<String>>,
}

/// Parameters for deleting a milestone.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DeleteMilestoneApiParams {
    /// The milestone ID to delete
    pub milestone_id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Attachment operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for uploading an attachment.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct UploadAttachmentApiParams {
    /// Base64-encoded file content
    pub content_base64: String,
    /// Original filename (e.g. "screenshot.png")
    pub filename: String,
    /// MIME content type (e.g. "image/png")
    pub content_type: String,
    /// Optional issue ID to auto-link the attachment to an issue after upload.
    #[serde(default)]
    pub issue_id: Option<String>,
}

/// Parameters for downloading an attachment.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DownloadAttachmentApiParams {
    /// The attachment ID to download
    pub attachment_id: String,
}

/// Parameters for deleting an attachment.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DeleteAttachmentApiParams {
    /// The attachment ID to delete
    pub attachment_id: String,
}

/// Parameters for listing attachments.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListAttachmentsApiParams {}

// ─────────────────────────────────────────────────────────────────────────────
// Issue attachment operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing attachments linked to an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListIssueAttachmentsApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team. Required if issue_identifier is not provided
    pub issue_number: Option<i64>,
}

/// Parameters for attaching an existing attachment to an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct AttachToIssueApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team. Required if issue_identifier is not provided
    pub issue_number: Option<i64>,
    /// The attachment ID to link to the issue
    pub attachment_id: String,
}

/// Parameters for detaching an attachment from an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct DetachFromIssueApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team. Required if issue_identifier is not provided
    pub issue_number: Option<i64>,
    /// The attachment ID to unlink from the issue
    pub attachment_id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Activity operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing issue activities.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListIssueActivitiesApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Issue number within the team
    pub issue_number: Option<i64>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
}

/// Parameters for listing workspace-level activities across all teams.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListWorkspaceActivitiesApiParams {
    /// Filter by team key (e.g. "TRA")
    pub team_key: Option<String>,
    /// Filter by action type (e.g. "status_changed", "comment_added")
    pub action_type: Option<String>,
    /// Filter by actor user ID
    pub actor_id: Option<String>,
    /// Filter by action source: "user", "agent", or "api"
    pub action_source: Option<String>,
    /// Filter to activities created at or after this ISO 8601 datetime
    pub created_after: Option<String>,
    /// Filter to activities created at or before this ISO 8601 datetime
    pub created_before: Option<String>,
    /// Maximum number of activities to return (default: 50, max: 200)
    pub limit: Option<i64>,
    /// Offset for pagination
    pub offset: Option<i64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// GitHub link operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing GitHub links associated with an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListGitHubLinksApiParams {
    /// Issue identifier in 'TRA-35' format
    pub issue_identifier: Option<String>,
    /// Team key (e.g. 'TRA'). Required if issue_identifier is not provided
    pub team_key: Option<String>,
    /// Issue number within the team. Required if issue_identifier is not provided
    pub issue_number: Option<i64>,
}

/// Parameters for looking up issues by commit SHA.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct LookupCommitApiParams {
    /// Commit SHA (full or abbreviated, minimum 7 characters). Prefix match is used.
    pub sha: String,
}

/// Parameters for looking up issues by branch name.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct LookupBranchApiParams {
    /// Branch name (exact match)
    pub branch: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Release operations
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for listing releases in the workspace.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListReleasesApiParams {
    /// Filter by team key (e.g. 'TRA')
    pub team_key: Option<String>,
}

/// Parameters for getting a single release by ID.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetReleaseApiParams {
    /// The release ID
    pub release_id: String,
}

/// Parameters for creating a new release.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreateReleaseApiParams {
    /// Team key this release belongs to (e.g. 'TRA')
    pub team_key: String,
    /// Git tag name (e.g. 'v2026.05.20.1')
    pub tag_name: String,
    /// Previous tag (for commit range context)
    pub previous_tag: Option<String>,
    /// Optional human-readable title
    pub title: Option<String>,
    /// Release notes / changelog markdown
    pub notes: Option<String>,
    /// List of full commit SHAs included in this release. Used to auto-link issues via github_links.
    pub commit_shas: Vec<String>,
}

/// Parameters for listing unreleased issues (completed but not yet shipped).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListUnreleasedIssuesApiParams {
    /// Filter by team key (e.g. 'TRA')
    pub team_key: Option<String>,
}

// ─── Star operations ─────────────────────────────────────────────────────────

/// Parameters for starring an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct StarIssueApiParams {
    /// The issue ID to star (e.g. 'iss_abc123')
    pub issue_id: String,
}

/// Parameters for unstarring an issue.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct UnstarIssueApiParams {
    /// The issue ID to unstar (e.g. 'iss_abc123')
    pub issue_id: String,
}

/// Parameters for listing starred issues (no params needed — scoped to current user/workspace).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListStarredIssuesApiParams {}
