// SPDX-License-Identifier: AGPL-3.0-or-later

//! Convenience functions for sending typed WebSocket messages.
//!
//! One function per message type — constructs `WebSocketMessage` with the correct
//! `MessageType` and data payload, then calls `send_to_user` or `broadcast_to_workspace`.

use trakkt_core::{MessageType, WebSocketMessage};

use super::WebSocketManager;

// ---------------------------------------------------------------------------
// Session events
// ---------------------------------------------------------------------------

/// Send a session_created notification.
pub async fn send_session_created(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    session_data: serde_json::Value,
) {
    let msg = WebSocketMessage::new(MessageType::SessionCreated)
        .with_session(session_id)
        .with_data(session_data);

    manager.send_to_user(user_id, msg).await;
}

/// Send a title_update notification.
pub async fn send_title_update(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    title: &str,
) {
    let msg = WebSocketMessage::new(MessageType::TitleUpdate)
        .with_session(session_id)
        .with_data(serde_json::json!({"title": title}));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Send an error message to a user.
pub async fn send_error(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: Option<&str>,
    error_message: &str,
    error_code: Option<&str>,
    context_type: Option<&str>,
) {
    let mut data = serde_json::json!({
        "error": error_message,
    });
    if let Some(code) = error_code {
        data["error_code"] = serde_json::Value::String(code.to_string());
    }
    if let Some(ct) = context_type {
        data["context_type"] = serde_json::Value::String(ct.to_string());
    }

    let mut msg = WebSocketMessage::new(MessageType::Error).with_data(data);
    if let Some(sid) = session_id {
        msg = msg.with_session(sid);
    }

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Workspace events
// ---------------------------------------------------------------------------

/// Parameters for [`send_workspace_invitation`].
pub struct WorkspaceInvitationParams<'a> {
    pub manager: &'a WebSocketManager,
    pub user_id: &'a str,
    pub invitation_id: &'a str,
    pub workspace_id: &'a str,
    pub workspace_name: &'a str,
    pub invited_by_name: &'a str,
    pub role: &'a str,
    pub message: &'a str,
}

/// Send a workspace_invitation notification to an invitee.
pub async fn send_workspace_invitation(params: WorkspaceInvitationParams<'_>) {
    let WorkspaceInvitationParams {
        manager,
        user_id,
        invitation_id,
        workspace_id,
        workspace_name,
        invited_by_name,
        role,
        message,
    } = params;

    let msg = WebSocketMessage::new(MessageType::WorkspaceInvitation)
        .with_data(serde_json::json!({
            "invitation_id": invitation_id,
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "invited_by_name": invited_by_name,
            "role": role,
            "message": message,
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send a workspace_removed notification to a removed user.
pub async fn send_workspace_removed(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    message: &str,
) {
    let msg = WebSocketMessage::new(MessageType::WorkspaceRemoved)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "message": message,
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send an ownership_transfer_offered notification.
pub async fn send_ownership_transfer_offered(
    manager: &WebSocketManager,
    user_id: &str,
    transfer_id: &str,
    workspace_name: &str,
    from_user_email: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OwnershipTransferOffered)
        .with_data(serde_json::json!({
            "transfer_id": transfer_id,
            "workspace_name": workspace_name,
            "from_user_email": from_user_email,
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Member events
// ---------------------------------------------------------------------------

/// Send a member_role_changed notification to the affected member.
pub async fn send_member_role_changed(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    new_role: &str,
) {
    let msg = WebSocketMessage::new(MessageType::MemberRoleChanged)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "new_role": new_role,
            "message": format!("Your role has been changed to {new_role}"),
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Broadcast a member_joined notification to all workspace members.
pub async fn send_member_joined(
    manager: &WebSocketManager,
    workspace_id: &str,
    user_name: &str,
    role: &str,
) {
    let msg = WebSocketMessage::new(MessageType::MemberJoined)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "user_name": user_name,
            "role": role,
            "message": format!("{user_name} joined the workspace"),
        }));

    manager.broadcast_to_workspace(workspace_id, msg, None).await;
}

/// Send an ownership_transfer_completed notification to the previous owner.
pub async fn send_ownership_transfer_completed(
    manager: &WebSocketManager,
    user_id: &str,
    transfer_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    new_owner_name: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OwnershipTransferCompleted)
        .with_data(serde_json::json!({
            "transfer_id": transfer_id,
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "new_owner_name": new_owner_name,
            "message": format!("{new_owner_name} accepted ownership of {workspace_name}"),
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send an ownership_transfer_declined notification to the original owner.
pub async fn send_ownership_transfer_declined(
    manager: &WebSocketManager,
    user_id: &str,
    transfer_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    declined_by_name: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OwnershipTransferDeclined)
        .with_data(serde_json::json!({
            "transfer_id": transfer_id,
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "declined_by_name": declined_by_name,
            "message": format!("{declined_by_name} declined ownership of {workspace_name}"),
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Live sync broadcasts
// ---------------------------------------------------------------------------

/// Broadcast a SyncAction to all connected workspace members.
/// Used for live sync — clients receive these to update their local cache.
pub async fn send_sync_action(
    manager: &WebSocketManager,
    workspace_id: &str,
    sync_action: &trakkt_types::sync::SyncAction,
    exclude_user_id: Option<&str>,
) {
    let msg = WebSocketMessage::new(MessageType::SyncAction)
        .with_data(serde_json::to_value(sync_action).unwrap_or_default());

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}
