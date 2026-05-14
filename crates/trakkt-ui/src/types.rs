// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared types that cross the server/client boundary.
//!
//! All types here must be `Serialize + Deserialize + Clone` since they are
//! sent over the wire between server functions and WASM client code.

use serde::{Deserialize, Serialize};

/// User profile data returned by the get_profile server function.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileData {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub theme: String,
    pub landing_page: String,
    pub is_personal_mode: bool,
    pub is_self_hosted: bool,
}

/// Pending workspace invitation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvitationData {
    pub invitation_id: String,
    pub workspace_id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
    pub expires_at: String,
}

/// Workspace settings data returned by the get_workspace_settings server function.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSettingsData {
    pub workspace_name: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Team management types
// ─────────────────────────────────────────────────────────────────────────────

/// A workspace member with user details.
///
/// Mirrors the JSON shape returned by `GET /api/v1/workspaces/members`.
/// Named `WorkspaceMember` to distinguish from `IssueTeamMember` which
/// represents membership in an issue-tracker team.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceMember {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub is_owner: bool,
    pub joined_at: String,
}

/// A pending workspace invitation (admin view).
///
/// Mirrors the JSON shape returned by `GET /api/v1/workspaces/invitations`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamInvitation {
    pub invitation_id: String,
    pub email: String,
    pub role: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue navigation state (browser history state for back button)
// ─────────────────────────────────────────────────────────────────────────────

/// State passed via the browser History API when navigating to an issue.
///
/// Allows the issue detail back button to return to the originating view
/// (team issues, board, my issues, project view) instead of a hardcoded path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueNavState {
    pub back_path: String,
    pub back_label: String,
}

impl IssueNavState {
    /// Build nav state from the current router location.
    ///
    /// Combines `path` and `search` into `back_path` and derives a
    /// human-readable `back_label` for the back button tooltip.
    pub fn from_current_path(path: &str, search: &str) -> Self {
        let back_path = if search.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{search}")
        };
        let back_label = derive_back_label(path);
        Self { back_path, back_label }
    }

    pub fn to_json(&self) -> String {
        match serde_json::to_string(self) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("Failed to serialize IssueNavState: {e}");
                String::new()
            }
        }
    }
}

/// Derive a human-readable label from a URL path for the back button.
fn derive_back_label(path: &str) -> String {
    if path == "/my-issues" {
        return "My Issues".to_string();
    }
    // /teams/TRA/issues → "TRA Issues"
    if let Some(rest) = path.strip_prefix("/teams/")
        && let Some(key) = rest.split('/').next()
    {
        return format!("{key} Issues");
    }
    // /projects/:id → "Project Issues"
    if path.starts_with("/projects/") {
        return "Project Issues".to_string();
    }
    // /inbox → "Inbox"
    if path == "/inbox" {
        return "Inbox".to_string();
    }
    "Issues".to_string()
}

/// A pending ownership transfer.
///
/// Mirrors the JSON shape returned by `GET /api/v1/workspaces/ownership/transfers`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnershipTransferData {
    pub transfer_id: String,
    pub from_user_id: String,
    pub from_user_email: String,
    pub to_user_id: String,
    pub to_user_email: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
    pub is_initiator: bool,
    pub is_recipient: bool,
}
