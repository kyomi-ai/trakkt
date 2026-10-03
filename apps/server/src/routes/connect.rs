// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket endpoints for the Trakkt Connect terminal session relay.
//!
//! Two endpoints:
//!
//! - `GET /ws/connect/agent` — Agent WebSocket (authenticated via Bearer token).
//!   Agents connect and register with the [`ConnectManager`]. They receive
//!   [`ServerMessage`] commands and send [`AgentMessage`] events.
//!
//! - `GET /ws/connect/terminal` — Browser terminal WebSocket (authenticated via
//!   JWT query parameter). Browsers send [`ServerMessage`] commands (spawn,
//!   input, resize, kill) and receive relayed [`AgentMessage`] events.
//!
//! The server never executes commands — it is purely a relay. Session routing
//! maps each `session_id` to the owning agent, and fan-out broadcasts agent
//! output to all subscribed browsers.

use axum::{
    extract::{Query, State, ws},
    http::HeaderMap,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use uuid::Uuid;

use trakkt_connect_protocol::wire::{AgentMessage, ServerMessage};

use super::auth_shared;
use crate::state::AppState;

/// Maximum size of a single WebSocket message (256 KB).
/// Terminal output can be bursty (large scrollback dumps), so we allow
/// more than the default WS endpoint.
const MAX_MESSAGE_SIZE: usize = 256 * 1024;

/// Close codes for Connect WebSocket errors.
const CLOSE_AUTH_REQUIRED: u16 = 4001;

// ---------------------------------------------------------------------------
// Agent endpoint: GET /ws/connect/agent
// ---------------------------------------------------------------------------

/// Agent WebSocket upgrade handler.
///
/// Authentication: Bearer token (`Authorization: Bearer trakkt-...` or JWT)
/// from the HTTP headers during the WebSocket handshake.
pub async fn agent_ws_handler(
    ws: ws::WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_agent_ws(socket, state, headers))
}

async fn handle_agent_ws(socket: ws::WebSocket, state: AppState, headers: HeaderMap) {
    // Authenticate via Bearer token (same path as MCP).
    let auth = if state.config.is_personal() {
        auth_shared::ResolvedAuth {
            workspace_id: "workspace-local".to_string(),
            user_id: "user-local".to_string(),
            scopes: vec![],
            action_source: trakkt_types::enums::ActionSource::Api,
            action_source_label: Some("connect-agent".to_string()),
        }
    } else {
        match auth_shared::resolve_auth(&headers, &state).await {
            Some(auth) => auth,
            None => {
                close_with_code(socket, CLOSE_AUTH_REQUIRED, "Authentication required").await;
                return;
            }
        }
    };

    if !state.config.is_personal()
        && (!auth.has_scope("write")
            || !active_membership(&state, &auth.workspace_id, &auth.user_id).await)
    {
        close_with_code(socket, CLOSE_AUTH_REQUIRED, "Workspace access denied").await;
        return;
    }
    let agent_id = Uuid::new_v4().to_string();

    // Register agent and get the outbound channel receiver.
    let mut agent_rx =
        state
            .connect_manager
            .register_agent(&agent_id, &auth.workspace_id, &auth.user_id);

    tracing::info!(
        agent_id = %agent_id,
        workspace_id = %auth.workspace_id,
        user_id = %auth.user_id,
        "Connect agent WebSocket connected"
    );

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Outbound task: drain mpsc receiver, send to WebSocket.
    let agent_id_for_send = agent_id.clone();
    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
        ping_interval.tick().await; // consume immediate tick

        loop {
            tokio::select! {
                msg = agent_rx.recv() => {
                    match msg {
                        Some(json) => {
                            if !matches!(tokio::time::timeout(std::time::Duration::from_secs(10), ws_sender.send(ws::Message::text(json))).await, Ok(Ok(()))) {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    // Send a protocol-level Ping to keep the connection alive
                    // through reverse proxies.
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_else(|e| {
                            tracing::warn!(error = %e, "SystemTime before UNIX epoch");
                            std::time::Duration::ZERO
                        })
                        .as_millis() as u64;
                    let ping_msg = ServerMessage::Ping { ts };
                    match serde_json::to_string(&ping_msg) {
                        Ok(json) => {
                            if !matches!(tokio::time::timeout(std::time::Duration::from_secs(10), ws_sender.send(ws::Message::text(json))).await, Ok(Ok(()))) {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to serialize Ping");
                        }
                    }
                }
            }
        }

        if !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(10), ws_sender.close()).await,
            Ok(Ok(()))
        ) {
            tracing::debug!("Connect WebSocket close failed or timed out");
        }
        tracing::debug!(agent_id = %agent_id_for_send, "Agent WS send task ended");
    });

    // Inbound task: parse AgentMessage JSON from agent, handle routing.
    let connect_mgr = state.connect_manager.clone();
    let agent_id_for_recv = agent_id.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_agent_message(&text, &agent_id_for_recv, &connect_mgr);
                }
                ws::Message::Pong(_) => {}
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
        tracing::debug!(agent_id = %agent_id_for_recv, "Agent WS recv task ended");
    });

    // Wait for either side to finish.
    tokio::select! {
        _ = &mut send_task => {}
        _ = &mut recv_task => {}
    }

    send_task.abort();
    recv_task.abort();

    // Cleanup: unregister agent and all its sessions.
    state.connect_manager.unregister_agent(&agent_id);
    tracing::info!(agent_id = %agent_id, "Connect agent WebSocket disconnected");
}

/// Handle an inbound message from the agent.
///
/// Routes agent output to the appropriate browser subscribers.
fn handle_agent_message(
    text: &str,
    agent_id: &str,
    connect_mgr: &trakkt_auth::connect_manager::ConnectManager,
) {
    let msg: AgentMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(agent_id, error = %e, "Failed to parse AgentMessage");
            return;
        }
    };

    connect_mgr.agent_message(agent_id, &msg);
}

// ---------------------------------------------------------------------------
// Browser terminal endpoint: GET /ws/connect/terminal
// ---------------------------------------------------------------------------

/// Query parameters for browser terminal WebSocket authentication.
#[derive(Debug, Deserialize)]
pub struct TerminalWsParams {
    /// JWT access token for authentication.
    token: Option<String>,
}

/// Browser terminal WebSocket upgrade handler.
///
/// Authentication: JWT in `?token=` query parameter (same pattern as the
/// main `/ws/{user_id}` endpoint).
pub async fn terminal_ws_handler(
    ws: ws::WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<TerminalWsParams>,
) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_terminal_ws(socket, state, params))
}

async fn handle_terminal_ws(socket: ws::WebSocket, state: AppState, params: TerminalWsParams) {
    // Authenticate via JWT.
    let (user_id, workspace_id) = if state.config.is_personal() {
        ("user-local".to_string(), "workspace-local".to_string())
    } else {
        match authenticate_terminal_ws(&state, &params).await {
            Some((user_id, workspace_id)) => (user_id, workspace_id),
            None => {
                close_with_code(socket, CLOSE_AUTH_REQUIRED, "Authentication required").await;
                return;
            }
        }
    };

    let browser_conn_id = trakkt_auth::connect_manager::next_browser_connection_id();

    tracing::info!(
        browser_conn_id,
        user_id = %user_id,
        workspace_id = %workspace_id,
        "Terminal browser WebSocket connected"
    );

    let (mut ws_sender, mut ws_receiver) = socket.split();

    let connect_mgr = state.connect_manager.clone();
    let mut aggregate_rx = connect_mgr.register_browser(browser_conn_id, &workspace_id, &user_id);

    // Outbound task: forward aggregated session output to browser WebSocket.
    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(45));
        ping_interval.tick().await;

        loop {
            tokio::select! {
                msg = aggregate_rx.recv() => {
                    match msg {
                        Some(json) => {
                            if !matches!(tokio::time::timeout(std::time::Duration::from_secs(10), ws_sender.send(ws::Message::text(json))).await, Ok(Ok(()))) {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if !matches!(tokio::time::timeout(std::time::Duration::from_secs(10), ws_sender.send(ws::Message::Ping(vec![].into()))).await, Ok(Ok(()))) {
                        break;
                    }
                }
            }
        }
        if !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(10), ws_sender.close()).await,
            Ok(Ok(()))
        ) {
            tracing::debug!("Connect WebSocket close failed or timed out");
        }
    });

    // Inbound task: parse browser commands, relay to agents.
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_browser_message(&text, browser_conn_id, &connect_mgr);
                }
                ws::Message::Pong(_) => {}
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => {}
        _ = &mut recv_task => {}
    }

    send_task.abort();
    recv_task.abort();
    state.connect_manager.unregister_browser(browser_conn_id);
    tracing::info!(
        browser_conn_id,
        user_id = %user_id,
        "Terminal browser WebSocket disconnected"
    );
}

/// Authenticate a browser terminal WebSocket connection via JWT.
///
/// Returns `(user_id, workspace_id)` on success, `None` on failure.
async fn authenticate_terminal_ws(
    state: &AppState,
    params: &TerminalWsParams,
) -> Option<(String, String)> {
    let token = params.token.as_deref().filter(|t| !t.is_empty())?;

    let claims = trakkt_auth::jwt::validate_token(token, &state.config.jwt_secret)
        .ok()?
        .claims;

    let user_id = claims
        .extra
        .get("user_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| claims.sub.clone());

    // Verify user exists and is active.
    let user = trakkt_auth::user_service::get_user_by_id(&state.db, &user_id)
        .await
        .ok()??;

    if !user.active {
        tracing::warn!(user_id = %user_id, "Terminal WS rejected: user disabled");
        return None;
    }

    // Get workspace_id from JWT claims, fall back to user's workspace context.
    let workspace_id = match claims
        .extra
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        Some(ws_id) => ws_id,
        None => {
            let ctx = trakkt_auth::user_service::get_user_workspace_context(&state.db, &user_id)
                .await
                .ok()??;
            ctx.0.workspace_id
        }
    };

    if !active_membership(state, &workspace_id, &user_id).await {
        return None;
    }
    Some((user_id, workspace_id))
}

/// Handle an inbound message from the browser.
///
/// The browser sends `ServerMessage` JSON. The server validates workspace
/// permissions and relays to the correct agent.
fn handle_browser_message(
    text: &str,
    browser_conn_id: u64,
    manager: &trakkt_auth::connect_manager::ConnectManager,
) {
    let msg: ServerMessage = match serde_json::from_str(text) {
        Ok(msg) => msg,
        Err(error) => {
            tracing::warn!(%error, "Invalid Connect command");
            return;
        }
    };
    if let Err(error) = manager.browser_command(browser_conn_id, &msg) {
        let session_id = match &msg {
            ServerMessage::SpawnSession { session_id, .. }
            | ServerMessage::SessionInput { session_id, .. }
            | ServerMessage::SessionResize { session_id, .. }
            | ServerMessage::SessionKill { session_id, .. }
            | ServerMessage::ScrollbackRequest { session_id } => session_id.as_str(),
            ServerMessage::ListSessions | ServerMessage::Ping { .. } => "",
        };
        manager.browser_error(browser_conn_id, session_id, error);
    }
}

/// Close a WebSocket with a custom close code and reason.
async fn close_with_code(socket: ws::WebSocket, code: u16, reason: &str) {
    let (mut sender, _) = socket.split();
    let close_frame = ws::CloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    if !matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sender.send(ws::Message::Close(Some(close_frame)))
        )
        .await,
        Ok(Ok(()))
    ) {
        tracing::debug!("Connect authentication close failed or timed out");
    }
}

async fn active_membership(state: &AppState, workspace: &str, user: &str) -> bool {
    if !matches!(trakkt_auth::user_service::get_user_by_id(&state.db, user).await, Ok(Some(record)) if record.active)
    {
        return false;
    }
    matches!(trakkt_auth::user_service::get_workspace_user(&state.db, workspace, user).await, Ok(Some(member)) if member.active)
}
