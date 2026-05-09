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
