// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync protocol types shared between trakkt-auth (service layer) and
//! trakkt-ui (WebSocket client).

use serde::{Deserialize, Serialize};

/// Action type for sync log entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Server->client sync response envelope.
///
/// Tagged enum serialized as `{"type": "sync_action", ...}` etc.
/// Used by the WebSocket handler to stream bootstrap and delta data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncResponse {
    SyncAction(SyncAction),
    SyncComplete { last_sync_id: i64 },
    SyncReset,
}

/// Well-known entity type constants for sync log entries.
pub mod entity_types {
    pub const WORKSPACE_SETTINGS: &str = "workspace_settings";
    pub const ISSUE: &str = "issue";
    pub const COMMENT: &str = "comment";
    pub const LABEL: &str = "label";
    pub const NOTIFICATION: &str = "notification";
    pub const TEAM: &str = "team";
    pub const STATUS: &str = "status";
    pub const PROJECT: &str = "project";
    pub const PROJECT_MILESTONE: &str = "project_milestone";
}
