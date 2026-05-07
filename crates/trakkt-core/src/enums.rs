// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared enums used across Tane crates.

use std::fmt;
use serde::{Deserialize, Serialize};

/// Workspace membership role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum WorkspaceRole {
    #[sqlx(rename = "workspace_admin")]
    WorkspaceAdmin,
    #[sqlx(rename = "workspace_user")]
    WorkspaceUser,
}

impl fmt::Display for WorkspaceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceAdmin => write!(f, "workspace_admin"),
            Self::WorkspaceUser => write!(f, "workspace_user"),
        }
    }
}

/// Workspace status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum WorkspaceStatus {
    #[sqlx(rename = "active")]
    Active,
    #[sqlx(rename = "trial")]
    Trial,
    #[sqlx(rename = "suspended")]
    Suspended,
}

/// Invitation status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum InvitationStatus {
    #[sqlx(rename = "pending")]
    Pending,
    #[sqlx(rename = "accepted")]
    Accepted,
    #[sqlx(rename = "declined")]
    Declined,
    #[sqlx(rename = "cancelled")]
    Cancelled,
    #[sqlx(rename = "expired")]
    Expired,
}

impl fmt::Display for InvitationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Accepted => write!(f, "accepted"),
            Self::Declined => write!(f, "declined"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

/// Ownership transfer status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum TransferStatus {
    #[sqlx(rename = "pending")]
    Pending,
    #[sqlx(rename = "accepted")]
    Accepted,
    #[sqlx(rename = "declined")]
    Declined,
    #[sqlx(rename = "cancelled")]
    Cancelled,
    #[sqlx(rename = "expired")]
    Expired,
}

impl fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Accepted => write!(f, "accepted"),
            Self::Declined => write!(f, "declined"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Expired => write!(f, "expired"),
        }
    }
}
