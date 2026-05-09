// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database model types — minimal structs that match the Tane schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::*;

/// User record from the `users` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub verified: bool,
    pub active: bool,
    pub last_login: Option<DateTime<Utc>>,
    pub last_workspace_id: Option<String>,
    pub oauth_data: Option<String>,
    pub extra_metadata: Option<serde_json::Value>,
    pub terms_accepted_at: Option<DateTime<Utc>>,
    pub terms_accepted_version: Option<String>,
    pub marketing_consent: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    /// Extract roles from extra_metadata JSON.
    pub fn roles(&self) -> Vec<String> {
        self.extra_metadata
            .as_ref()
            .and_then(|m| m.get("roles"))
            .and_then(|r| serde_json::from_value::<Vec<String>>(r.clone()).ok())
            .unwrap_or_else(|| vec!["user".to_string()])
    }
}

/// User auth method record from the `user_auth_methods` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserAuthMethod {
    pub id: i32,
    pub user_id: String,
    pub auth_type: String,
    pub auth_data: serde_json::Value,
    pub active: bool,
    pub last_used: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Workspace record from the `workspaces` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Workspace {
    pub workspace_id: String,
    pub name: Option<String>,
    pub admin_email: Option<String>,
    pub owner_user_id: String,
    pub status: WorkspaceStatus,
    pub user_limit: Option<i32>,
    pub settings: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Workspace user membership record from the `workspace_users` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WorkspaceUser {
    pub id: i32,
    pub workspace_id: String,
    pub user_id: String,
    pub role: WorkspaceRole,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

/// Workspace invitation record from the `workspace_invitations` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WorkspaceInvitation {
    pub invitation_id: String,
    pub workspace_id: String,
    pub email: String,
    pub role: String,
    pub invited_by_user_id: String,
    pub status: InvitationStatus,
    pub accepted_at: Option<DateTime<Utc>>,
    pub accepted_by_user_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Ownership transfer record from the `ownership_transfers` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OwnershipTransfer {
    pub transfer_id: String,
    pub workspace_id: String,
    pub from_user_id: String,
    pub to_user_id: String,
    pub status: TransferStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Verification token record from the `verification_tokens` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct VerificationToken {
    pub token_id: String,
    pub email: String,
    pub token_hash: String,
    pub token_type: String,
    pub used: bool,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// API token record from the `api_tokens` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiToken {
    pub token_id: String,
    pub user_id: String,
    pub name: String,
    pub token_hash: String,
    pub active: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<String>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<String>,
    pub last_used: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// OAuth client record from the `oauth_clients` table.
///
/// Used for MCP dynamic client registration (RFC 7591). Clients register
/// themselves with redirect URIs and receive a `client_id` used throughout
/// the OAuth 2.0 authorization code flow.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OAuthClient {
    pub id: String,
    pub client_id: String,
    pub client_secret_hash: Option<String>,
    pub name: String,
    pub redirect_uris: serde_json::Value,
    pub scopes: serde_json::Value,
    pub client_type: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}
