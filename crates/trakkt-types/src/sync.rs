// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync protocol types shared between trakkt-auth (service layer) and
//! trakkt-ui (WebSocket client).

use serde::{Deserialize, Serialize};

/// Action type for sync log entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncActionType {
    Insert,
    Update,
    Delete,
}

/// A single sync log entry broadcast to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAction {
    pub sync_id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub workspace_id: String,
    pub action: SyncActionType,
    pub data: Option<serde_json::Value>,
    pub timestamp: String,
}

/// Well-known entity type constants for sync log entries.
pub mod entity_types {
    pub const WORKSPACE_SETTINGS: &str = "workspace_settings";
    pub const ISSUE: &str = "issue";
    pub const COMMENT: &str = "comment";
    pub const LABEL: &str = "label";
    pub const NOTIFICATION: &str = "notification";
    pub const TEAM: &str = "team";
}
