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

use crate::enums::ActionSource;

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
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub released_at: Option<String>,
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
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub released_at: Option<String>,
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
    pub archived_at: Option<String>,
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

/// A user-pinned issue, project, team or view, for quick sidebar access.
///
/// `target_type` is a `String` and not a [`crate::enums::FavoriteTarget`] on
/// purpose, and the asymmetry is deliberate: writes are strict, reads are not.
/// `favorite_service::add_favorite` takes the enum, so nothing new can be stored
/// outside the closed set — but this type also decodes rows that predate
/// TRA-10025, when the column took whatever string an HTTP caller sent. Parsing
/// here would turn one such legacy row into a failed bootstrap for its owner
/// rather than a favorite that renders as nothing.
/// `migrations/20260807000000_prune_dangling_favorites.sql` is what removes
/// them; until it has run, this field has to be able to hold one.
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
    pub action_source: ActionSource,
    pub action_source_label: Option<String>,
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
    pub action_source: ActionSource,
    pub action_source_label: Option<String>,
    pub created_at: String,
    pub deleted_at: Option<String>,
    /// Optional context ID for deep-linking (e.g. comment_id for "commented" notifications).
    #[serde(default)]
    pub context_id: Option<String>,
}

impl Notification {
    /// Whether the inbox still lists this notification *and* the user has not
    /// read it — what the sidebar's unread badge counts.
    ///
    /// Both halves are load-bearing. Deleting from the inbox is a soft delete:
    /// `notification_service::bulk_delete_notifications` stamps `deleted_at` and
    /// leaves the row in place, and it reaches clients as an `Update` carrying
    /// the stamped row rather than as a `Delete`, so a dismissed notification is
    /// still sitting in the client's cache with `read == false`. Counting `!read`
    /// alone therefore counts rows the inbox no longer shows.
    ///
    /// This is the client-side statement of the predicate
    /// `notification_service::count_unread` runs as SQL —
    /// `read = false AND deleted_at IS NULL`, the same one
    /// `list_notifications` applies when it builds the inbox's rows. The two are
    /// written in different languages and cannot be shared, so the point of
    /// naming this once here is that the next place needing "unread, as the
    /// inbox means it" reads it rather than restating it and drifting.
    pub fn is_unread_in_inbox(&self) -> bool {
        !self.read && self.deleted_at.is_none()
    }
}

/// User-level notification preferences for a workspace.
///
/// Controls which event types generate notifications and whether
/// self-initiated agent/API actions should notify the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationPreferences {
    pub preference_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub notify_status_changes: bool,
    pub notify_comments: bool,
    pub notify_assignments: bool,
    pub notify_priority_changes: bool,
    pub notify_label_changes: bool,
    pub notify_due_date_changes: bool,
    pub notify_estimate_changes: bool,
    pub notify_milestone_changes: bool,
    pub notify_project_changes: bool,
    pub notify_team_changes: bool,
    pub notify_relation_changes: bool,
    pub notify_own_agent_actions: bool,
    pub notify_own_api_actions: bool,
    pub delivery_channel: String,
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
    pub status_name: String,
    pub direction: String,
}

/// A single activity entry for an issue (field change, status transition, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueActivity {
    pub activity_id: String,
    pub issue_id: String,
    pub workspace_id: String,
    pub actor_id: Option<String>,
    pub actor_name: Option<String>,
    pub action_type: String,
    pub field: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub metadata: Option<String>,
    pub action_source: ActionSource,
    pub action_source_label: Option<String>,
    pub created_at: String,
}

/// An activity entry with issue context for workspace-level activity feeds.
///
/// Extends [`IssueActivity`] with team key, issue number, and title so that
/// cross-team activity feeds can display meaningful issue identifiers without
/// additional lookups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceActivity {
    pub activity_id: String,
    pub issue_id: String,
    pub workspace_id: String,
    pub actor_id: Option<String>,
    pub actor_name: Option<String>,
    pub action_type: String,
    pub field: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub metadata: Option<String>,
    pub action_source: ActionSource,
    pub action_source_label: Option<String>,
    pub created_at: String,
    // Issue context
    pub team_key: String,
    pub issue_number: i32,
    pub issue_title: String,
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

/// The `workspace_settings` sync entity: the workspace-level fields clients
/// cache, as one addressable row.
///
/// This is the only entity the bootstrap streams that is not a table row of its
/// own — it is a projection of the `workspaces` row, assembled by
/// `workspace_service::WorkspaceSnapshotRow::into_snapshot`. It was also the
/// only one with no Rust type at all: the projection was a hand-built
/// `serde_json::json!` literal, so it was the one entity whose id could not be
/// derived from a type and the one place the next `"workspace_id"` typo would
/// have landed. Giving it a struct is what lets it implement [`SyncEntity`]
/// alongside the other ten.
///
/// [`SyncEntity`]: crate::sync::SyncEntity
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSettingsSnapshot {
    pub workspace_id: String,
    pub name: Option<String>,
    /// The `workspaces.settings` column, carried as parsed JSON rather than as
    /// [`WorkspaceSettings`].
    ///
    /// Deliberately untyped: `WorkspaceSettings` does not deny unknown fields,
    /// so decoding into it and re-encoding would silently drop any key it does
    /// not declare — a lossy round-trip for a column written as free-form JSON
    /// by older versions and by `update_workspace_settings`, which takes a
    /// `serde_json::Value` from its caller. `None` is a NULL column.
    pub settings: Option<serde_json::Value>,
    pub default_team_id: Option<String>,
    pub updated_at: String,
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

/// The link between one issue and one attachment — a row of the
/// `issue_attachments` junction table.
///
/// Distinct from [`Attachment`], which is the file itself. An upload creates an
/// attachment and links it in one request, so both frames go out together; but a
/// link can also be made against a file that already exists, and that change is
/// this row and nothing else. Without a type of its own there is no value
/// `attachment_service::attach_to_issue` could put on the wire, and
/// `cache/apply.rs` drops an insert frame that carries no payload before it
/// reaches any entity arm — so the link would reach no other client at all.
///
/// The junction has a composite primary key and no surrogate id; the sync entity
/// id is `issue_id:attachment_id`, assembled by the service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueAttachment {
    pub issue_id: String,
    pub attachment_id: String,
    pub created_at: String,
}

/// User-submitted feedback (bug report, feature request, or question).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub feedback_type: String,
    pub description: String,
    pub screenshot_url: Option<String>,
    pub include_context: bool,
    pub context: Option<String>,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub resolution_notes: Option<String>,
    pub resolved_by: Option<String>,
}

/// A release — a tagged set of issues shipped together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub release_id: String,
    pub workspace_id: String,
    pub team_key: String,
    pub tag_name: String,
    pub previous_tag: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub issue_count: i64,
}

/// A release with its full list of linked issues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseWithIssues {
    pub release_id: String,
    pub workspace_id: String,
    pub team_key: String,
    pub tag_name: String,
    pub previous_tag: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub issues: Vec<ReleaseIssue>,
}

/// A lightweight issue reference within a release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseIssue {
    pub issue_id: String,
    pub team_key: String,
    pub number: i32,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
}

// ---------------------------------------------------------------------------
// Sync addressing
// ---------------------------------------------------------------------------

/// Implement [`SyncEntity`] for the models the sync bootstrap streams.
///
/// One row per entity: the model, the [`entity_types`] constant its frames are
/// tagged with, and the field holding its primary key. This table is the whole
/// of what `handle_sync_bootstrap` used to restate — an `entity_type` constant
/// and an id-field *string literal* — once per entity at its own call site, with
/// nothing checking either against the model it was written beside.
///
/// What the compiler now checks, and what it still does not:
///
/// - `$id_field` is a field access. A field that does not exist, or that is not
///   a `String`, does not compile. There is no string to mistype.
/// - `$entity_type` is a path into [`entity_types`], so a type outside the
///   declared set does not compile either.
/// - The **pairing** of a model with its constant is still a statement, not a
///   deduction: `Label => STATUS, label_id` would compile. Nothing in Rust can
///   derive a wire string from a type, so this has to be said once somewhere.
///   Said here, it is eleven adjacent rows that read as a table; said at the
///   call sites, it was eleven separate lines scattered through a handler. A
///   wrong pairing here is also not silent the way a wrong id literal was — the
///   client would cache the payload under another type's store and misrender
///   it, rather than accept it, file it under `""`, and go quietly stale.
///
/// [`SyncEntity`]: crate::sync::SyncEntity
/// [`entity_types`]: crate::sync::entity_types
macro_rules! impl_sync_entity {
    ($($model:ident => $entity_type:ident, $id_field:ident;)+) => {
        $(
            impl crate::sync::SyncEntity for $model {
                const ENTITY_TYPE: &'static str = crate::sync::entity_types::$entity_type;

                fn entity_id(&self) -> &str {
                    &self.$id_field
                }
            }
        )+
    };
}

impl_sync_entity! {
    IssueWithDetails => ISSUE, issue_id;
    Label => LABEL, label_id;
    Status => STATUS, status_id;
    Team => TEAM, team_id;
    Project => PROJECT, project_id;
    View => VIEW, view_id;
    Favorite => FAVORITE, favorite_id;
    Notification => NOTIFICATION, notification_id;
    Comment => COMMENT, comment_id;
    ProjectMilestone => PROJECT_MILESTONE, milestone_id;
    WorkspaceSettingsSnapshot => WORKSPACE_SETTINGS, workspace_id;
}
