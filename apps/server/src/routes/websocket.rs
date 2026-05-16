// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket endpoints for real-time communication.
//!
//! One endpoint:
//! - `GET /ws/{user_id}` — Authenticated WebSocket for logged-in users
//!
//! Supports the sync protocol (bootstrap + delta) for real-time entity
//! synchronization. Personal mode bypasses JWT auth (single-user, no login).

use axum::{
    extract::{ws, Path, Query, State},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use trakkt_auth::{jwt, user_service};

use crate::state::AppState;

/// Query parameters for WebSocket authentication.
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    /// JWT access token for authentication.
    token: Option<String>,
}

/// Close codes matching the Python backend.
const CLOSE_AUTH_REQUIRED: u16 = 4001;
const CLOSE_FORBIDDEN: u16 = 4003;
const CLOSE_TOO_MANY_CONNECTIONS: u16 = 4029;

/// Maximum size of a single WebSocket message from the client (64 KB).
/// Prevents DoS via oversized messages. Client->server messages are small
/// JSON payloads (sync_bootstrap, sync_delta, ping), so 64 KB is generous.
const MAX_MESSAGE_SIZE: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// GET /ws/{user_id} -- Authenticated WebSocket
// ---------------------------------------------------------------------------

/// Authenticated WebSocket upgrade handler.
///
/// Authentication flow:
/// 1. In personal mode: skip JWT auth, use hardcoded user/workspace IDs
/// 2. In multi-user mode:
///    a. Extract JWT from `?token=` query parameter
///    b. Validate JWT signature and expiry
///    c. Look up user in database, verify active status
///    d. Verify path user_id matches JWT user_id
/// 3. Register connection with WebSocketManager
/// 4. Spawn inbound/outbound tasks
pub async fn ws_handler(
    ws: ws::WebSocketUpgrade,
    State(state): State<AppState>,
    Path(path_user_id): Path<String>,
    Query(params): Query<WsAuthParams>,
) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_authenticated_ws(socket, state, path_user_id, params))
}

async fn handle_authenticated_ws(
    socket: ws::WebSocket,
    state: AppState,
    path_user_id: String,
    params: WsAuthParams,
) {
    // Resolve user_id and workspace_id -- personal mode bypasses JWT entirely.
    let (user_id, workspace_id) = if state.config.is_personal() {
        ("user-local".to_string(), "workspace-local".to_string())
    } else {
        match authenticate_ws(&state, &path_user_id, &params).await {
            AuthResult::Ok { user_id, workspace_id } => (user_id, workspace_id),
            AuthResult::Err { code, reason } => {
                close_with_code(socket, code, reason).await;
                return;
            }
        }
    };

    // Register with WebSocketManager (heartbeat sent automatically).
    let (connection_id, mut manager_rx) = match state.ws_manager.connect(&user_id) {
        Ok(conn) => conn,
        Err(_) => {
            close_with_code(socket, CLOSE_TOO_MANY_CONNECTIONS, "Too many connections").await;
            return;
        }
    };
    tracing::info!(
        user_id = %user_id,
        connection_id,
        "WebSocket connected"
    );

    // Split socket and run concurrent tasks.
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Outbound task: forwards messages from WebSocketManager + periodic pings.
    // Pings every 45s keep the connection alive through reverse proxies.
    let user_id_for_send = user_id.clone();
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(45));
        ping_interval.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                msg = manager_rx.recv() => {
                    match msg {
                        Some(json) => {
                            if ws_sender.send(ws::Message::text(json)).await.is_err() {
                                break;
                            }
                        }
                        None => break, // channel closed
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_sender.send(ws::Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }

        let _ = ws_sender.close().await;
        tracing::debug!("WS send task ended for user {user_id_for_send}");
    });

    let manager_clone = state.ws_manager.clone();
    let db_clone = state.db.clone();
    let user_id_for_recv = user_id.clone();
    let workspace_id_for_recv = workspace_id.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_client_message(
                        &text,
                        &user_id_for_recv,
                        &workspace_id_for_recv,
                        &manager_clone,
                        &db_clone,
                    )
                    .await;
                }
                ws::Message::Pong(_) => {} // expected response to our pings
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
        tracing::debug!("WS recv task ended for user {user_id_for_recv}");
    });

    // Wait for either side to finish.
    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    // Cleanup.
    state.ws_manager.disconnect(&user_id, connection_id);
    tracing::info!(
        user_id = %user_id,
        connection_id,
        "WebSocket disconnected"
    );
}

/// Result of WebSocket authentication: either success or a close code + reason.
enum AuthResult {
    Ok { user_id: String, workspace_id: String },
    Err { code: u16, reason: &'static str },
}

/// Authenticate a WebSocket connection via JWT.
///
/// Returns `AuthResult::Ok` with user/workspace IDs on success, or
/// `AuthResult::Err` with the appropriate close code on failure.
async fn authenticate_ws(
    state: &AppState,
    path_user_id: &str,
    params: &WsAuthParams,
) -> AuthResult {
    // 1. Extract and validate JWT token.
    let token = match &params.token {
        Some(t) if !t.is_empty() => t.as_str(),
        _ => {
            tracing::warn!("WebSocket connection rejected: no token provided");
            return AuthResult::Err { code: CLOSE_AUTH_REQUIRED, reason: "Authentication required" };
        }
    };

    let claims = match jwt::validate_token(token, &state.config.jwt_secret) {
        Ok(token_data) => token_data.claims,
        Err(e) => {
            tracing::warn!("WebSocket JWT validation failed: {e}");
            return AuthResult::Err { code: CLOSE_AUTH_REQUIRED, reason: "Authentication failed" };
        }
    };

    let jwt_user_id = &claims.sub;

    // 2. Extract user_id and workspace_id from path.
    let actual_user_id = extract_user_id_from_path(path_user_id);
    let workspace_id = extract_workspace_id_from_path(path_user_id).to_string();

    // 3. Verify path user_id matches JWT user_id.
    if actual_user_id != jwt_user_id {
        tracing::warn!(
            "WebSocket user_id mismatch: path={actual_user_id}, jwt={jwt_user_id}"
        );
        return AuthResult::Err { code: CLOSE_FORBIDDEN, reason: "User ID mismatch" };
    }

    // 4. Look up user in database and verify active status.
    let user = match user_service::get_user_by_id(&state.db, jwt_user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::warn!("WebSocket rejected: user not found: {jwt_user_id}");
            return AuthResult::Err { code: CLOSE_FORBIDDEN, reason: "User not found" };
        }
        Err(e) => {
            tracing::error!("WebSocket user lookup failed: {e}");
            return AuthResult::Err { code: CLOSE_AUTH_REQUIRED, reason: "Authentication failed" };
        }
    };

    if !user.active {
        tracing::warn!("WebSocket rejected: user disabled: {jwt_user_id}");
        return AuthResult::Err { code: CLOSE_FORBIDDEN, reason: "Account disabled" };
    }

    AuthResult::Ok {
        user_id: jwt_user_id.clone(),
        workspace_id,
    }
}

/// Extract the workspace_id from a path that is "{workspace_id}_{user_id}".
///
/// Returns the prefix portion before the user_id boundary. Returns an empty
/// string when no workspace prefix is found (plain user_id paths).
fn extract_workspace_id_from_path(path: &str) -> &str {
    if let Some(idx) = path.find("_usr_") {
        return &path[..idx];
    }
    if let Some(idx) = path.find('_') {
        let prefix = &path[..idx];
        if prefix.starts_with("ws-") || prefix.starts_with("workspace-") {
            return &path[..idx];
        }
    }
    ""
}

/// Extract the actual user_id from a path that may be "{workspace_id}_{user_id}".
///
/// The frontend sends `{workspace_id}_{user_id}` as the path parameter.
/// User IDs always start with `usr_`, so we search for `_usr_` first.
/// Legacy fallback: check for known workspace_id prefix formats (`ws-`, `workspace-`).
fn extract_user_id_from_path(path: &str) -> &str {
    if let Some(idx) = path.find("_usr_") {
        return &path[idx + 1..];
    }
    if let Some(idx) = path.find('_') {
        let prefix = &path[..idx];
        if prefix.starts_with("ws-") || prefix.starts_with("workspace-") {
            return &path[idx + 1..];
        }
    }
    path
}

/// Handle a client->server message on the authenticated WebSocket.
async fn handle_client_message(
    text: &str,
    user_id: &str,
    workspace_id: &str,
    manager: &trakkt_auth::websocket::WebSocketManager,
    db: &trakkt_core::DbPool,
) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("WS received invalid JSON from user {user_id}");
            return;
        }
    };

    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "sync_bootstrap" => {
            handle_sync_bootstrap(manager, db, user_id, workspace_id).await;
        }
        "sync_delta" => {
            let last_sync_id = msg
                .get("last_sync_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            handle_sync_delta(manager, db, user_id, workspace_id, last_sync_id).await;
        }
        _ => {
            tracing::debug!(user_id, msg_type, "Received unknown client message type");
        }
    }
}

// --- Sync protocol handlers -------------------------------------------------

/// Handle a `sync_bootstrap` request.
///
/// Streams all issues and labels for the workspace as `SyncAction` messages
/// with `action = Insert`, then closes with a `SyncComplete` carrying the
/// current `latest_sync_id`. Clients should store this ID and use `sync_delta`
/// for subsequent reconnects.
async fn handle_sync_bootstrap(
    manager: &trakkt_auth::websocket::WebSocketManager,
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
) {
    use trakkt_types::models::IssueFilters;
    use trakkt_types::sync::entity_types;

    tracing::debug!(user_id, workspace_id, "Handling sync_bootstrap");

    // 1. Fetch all non-archived issues (archived issues are excluded from bootstrap).
    let issues = trakkt_auth::issue_service::list_issues(
        db,
        workspace_id,
        None,
        &IssueFilters {
            include_archived: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(user_id, workspace_id, error = %e, "list_issues failed during bootstrap");
        vec![]
    });

    // 2. Fetch all labels.
    let labels = trakkt_auth::label_service::list_labels(db, workspace_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_labels failed during bootstrap");
            vec![]
        });

    // 3. Fetch statuses, teams, and projects.
    let statuses = trakkt_auth::status_service::list_statuses(db, workspace_id, None)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_statuses failed during bootstrap");
            vec![]
        });

    let teams = trakkt_auth::team_service::list_teams(db, workspace_id, Some(user_id))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_teams failed during bootstrap");
            vec![]
        });

    let projects = trakkt_auth::project_service::list_projects(db, workspace_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_projects failed during bootstrap");
            vec![]
        });

    let views = trakkt_auth::view_service::list_views(db, workspace_id, user_id, None)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_views failed during bootstrap");
            vec![]
        });

    let favorites = trakkt_auth::favorite_service::list_favorites(db, user_id, workspace_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_favorites failed during bootstrap");
            vec![]
        });

    let notifications = trakkt_auth::notification_service::list_notifications(db, user_id, false)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_notifications failed during bootstrap");
            vec![]
        });

    let comments = trakkt_auth::comment_service::list_comments_for_workspace(db, workspace_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_comments_for_workspace failed during bootstrap");
            vec![]
        });

    // 4. Get the current sync watermark.
    let latest_sync_id =
        trakkt_auth::sync_log_service::get_latest_sync_id(db, workspace_id)
            .await
            .unwrap_or(0);

    // 5. Stream each entity as a SyncAction with action=Insert.
    let issue_values: Vec<serde_json::Value> = issues
        .iter()
        .filter_map(|i| serde_json::to_value(i).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::ISSUE, "issue_id", issue_values).await;

    let label_values: Vec<serde_json::Value> = labels
        .iter()
        .filter_map(|l| serde_json::to_value(l).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::LABEL, "label_id", label_values).await;

    let status_values: Vec<serde_json::Value> = statuses
        .iter()
        .filter_map(|s| serde_json::to_value(s).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::STATUS, "status_id", status_values).await;

    let team_values: Vec<serde_json::Value> = teams
        .iter()
        .filter_map(|t| serde_json::to_value(t).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::TEAM, "team_id", team_values).await;

    let project_values: Vec<serde_json::Value> = projects
        .iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::PROJECT, "project_id", project_values).await;

    let view_values: Vec<serde_json::Value> = views
        .iter()
        .filter_map(|v| serde_json::to_value(v).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::VIEW, "view_id", view_values).await;

    let favorite_values: Vec<serde_json::Value> = favorites
        .iter()
        .filter_map(|f| serde_json::to_value(f).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::FAVORITE, "favorite_id", favorite_values).await;

    let notification_values: Vec<serde_json::Value> = notifications
        .iter()
        .filter_map(|n| serde_json::to_value(n).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::NOTIFICATION, "notification_id", notification_values).await;

    let comment_values: Vec<serde_json::Value> = comments
        .iter()
        .filter_map(|c| serde_json::to_value(c).ok())
        .collect();
    stream_entities(manager, user_id, workspace_id, entity_types::COMMENT, "comment_id", comment_values).await;

    // 6. Signal completion with the current sync watermark.
    send_sync_response(
        manager,
        user_id,
        trakkt_types::sync::SyncResponse::SyncComplete {
            last_sync_id: latest_sync_id,
        },
    )
    .await;

    tracing::debug!(user_id, workspace_id, latest_sync_id, "sync_bootstrap complete");
}

/// Handle a `sync_delta` request.
///
/// Streams all sync log entries with `sync_id > last_sync_id`. If the
/// requested `sync_id` is no longer in the log (pruned), sends `SyncReset`
/// so the client falls back to a full bootstrap.
async fn handle_sync_delta(
    manager: &trakkt_auth::websocket::WebSocketManager,
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    last_sync_id: i64,
) {
    use trakkt_types::sync::SyncResponse;

    tracing::debug!(user_id, workspace_id, last_sync_id, "Handling sync_delta");

    // 1. Verify the requested sync_id is still available (not pruned).
    if last_sync_id > 0 {
        match trakkt_auth::sync_log_service::is_sync_id_available(db, workspace_id, last_sync_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    user_id,
                    workspace_id,
                    last_sync_id,
                    "sync_id pruned -- sending SyncReset"
                );
                send_sync_response(manager, user_id, SyncResponse::SyncReset).await;
                return;
            }
            Err(e) => {
                tracing::error!(
                    user_id,
                    workspace_id,
                    last_sync_id,
                    error = %e,
                    "DB error checking sync_id availability -- sending SyncReset"
                );
                send_sync_response(manager, user_id, SyncResponse::SyncReset).await;
                return;
            }
        }
    }

    // 2. Fetch all entries since last_sync_id (capped at 10,000 rows).
    let entries =
        trakkt_auth::sync_log_service::get_entries_since(db, workspace_id, last_sync_id, 10_000)
            .await
            .unwrap_or_default();

    // 3. Stream each entry as a SyncAction message.
    for entry in &entries {
        send_sync_response(manager, user_id, SyncResponse::SyncAction(entry.clone())).await;
    }

    // 4. Send SyncComplete with the latest sync_id we streamed.
    let latest_id = entries.last().map(|e| e.sync_id).unwrap_or(last_sync_id);
    send_sync_response(
        manager,
        user_id,
        SyncResponse::SyncComplete {
            last_sync_id: latest_id,
        },
    )
    .await;

    tracing::debug!(user_id, workspace_id, latest_id, "sync_delta complete");
}

/// Stream a batch of entities as individual `SyncAction(Insert)` messages.
///
/// Used by `handle_sync_bootstrap` to avoid copy-pasting the same loop for
/// each entity type. `id_field` is the JSON key that holds the entity's
/// primary key (e.g. `"issue_id"`, `"label_id"`).
async fn stream_entities(
    manager: &trakkt_auth::websocket::WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    entity_type: &str,
    id_field: &str,
    items: Vec<serde_json::Value>,
) {
    use trakkt_types::sync::{SyncAction, SyncActionType, SyncResponse};

    for item in items {
        let entity_id = item
            .get(id_field)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let timestamp = item
            .get("updated_at")
            .or_else(|| item.get("created_at"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let action = SyncAction {
            sync_id: 0,
            entity_type: entity_type.to_string(),
            entity_id,
            workspace_id: workspace_id.to_string(),
            action: SyncActionType::Insert,
            data: Some(item),
            timestamp,
        };
        send_sync_response(manager, user_id, SyncResponse::SyncAction(action)).await;
    }
}

/// Send a `SyncResponse` to a specific user over WebSocket.
async fn send_sync_response(
    manager: &trakkt_auth::websocket::WebSocketManager,
    user_id: &str,
    response: trakkt_types::sync::SyncResponse,
) {
    let json = match serde_json::to_string(&response) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("Failed to serialize SyncResponse: {e}");
            return;
        }
    };
    manager.send_to_user_raw(user_id, &json).await;
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Close a WebSocket with a custom close code and reason.
async fn close_with_code(socket: ws::WebSocket, code: u16, reason: &str) {
    let (mut sender, _) = socket.split();
    let close_frame = ws::CloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    let _ = sender.send(ws::Message::Close(Some(close_frame))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_user_id_plain() {
        assert_eq!(extract_user_id_from_path("user-abc123"), "user-abc123");
    }

    #[test]
    fn extract_user_id_with_ws_prefix() {
        assert_eq!(
            extract_user_id_from_path("ws-550e8400-e29b-41d4-a716-446655440000_user-abc123"),
            "user-abc123"
        );
    }

    #[test]
    fn extract_user_id_with_workspace_prefix() {
        assert_eq!(
            extract_user_id_from_path("workspace-99f24d05-673d25b8_user-PHjsNsAj8hqZXOGGM-em1Q"),
            "user-PHjsNsAj8hqZXOGGM-em1Q"
        );
    }

    #[test]
    fn extract_user_id_with_underscore_in_user_id() {
        assert_eq!(
            extract_user_id_from_path("ws-uuid-here_user-abc_123"),
            "user-abc_123"
        );
    }

    #[test]
    fn extract_user_id_no_workspace_prefix_with_underscore() {
        assert_eq!(extract_user_id_from_path("user-abc_123"), "user-abc_123");
    }

    #[test]
    fn extract_user_id_with_non_standard_workspace_prefix() {
        assert_eq!(
            extract_user_id_from_path("e2e-test-workspace-0001_usr_a0bda4c2e7af4be3a29d"),
            "usr_a0bda4c2e7af4be3a29d"
        );
    }

    #[test]
    fn extract_workspace_id_plain() {
        assert_eq!(extract_workspace_id_from_path("user-abc123"), "");
    }

    #[test]
    fn extract_workspace_id_with_ws_prefix() {
        assert_eq!(
            extract_workspace_id_from_path("ws-550e8400-e29b-41d4-a716-446655440000_user-abc123"),
            "ws-550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn extract_workspace_id_with_workspace_prefix() {
        assert_eq!(
            extract_workspace_id_from_path("workspace-99f24d05-673d25b8_user-PHjsNsAj8hqZXOGGM-em1Q"),
            "workspace-99f24d05-673d25b8"
        );
    }

    #[test]
    fn extract_workspace_id_e2e_format() {
        assert_eq!(
            extract_workspace_id_from_path("e2e-test-workspace-0001_usr_a0bda4c2e7af4be3a29d"),
            "e2e-test-workspace-0001"
        );
    }
}
