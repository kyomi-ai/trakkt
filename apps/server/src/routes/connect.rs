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
    extract::{ws, Query, State},
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
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
        ping_interval.tick().await; // consume immediate tick

        loop {
            tokio::select! {
                msg = agent_rx.recv() => {
                    match msg {
                        Some(json) => {
                            if ws_sender.send(ws::Message::text(json)).await.is_err() {
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
                            if ws_sender.send(ws::Message::text(json)).await.is_err() {
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

        let _ = ws_sender.close().await;
        tracing::debug!(agent_id = %agent_id_for_send, "Agent WS send task ended");
    });

    // Inbound task: parse AgentMessage JSON from agent, handle routing.
    let connect_mgr = state.connect_manager.clone();
    let agent_id_for_recv = agent_id.clone();
    let recv_task = tokio::spawn(async move {
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
        _ = send_task => {}
        _ = recv_task => {}
    }

    // Cleanup: unregister agent and all its sessions.
    state.connect_manager.unregister_agent(&agent_id);
    tracing::info!(agent_id = %agent_id, "Connect agent WebSocket disconnected");
}

/// Handle an inbound message from the agent.
///
/// Routes agent output to the appropriate browser subscribers.
fn handle_agent_message(text: &str, agent_id: &str, connect_mgr: &trakkt_auth::connect_manager::ConnectManager) {
    let msg: AgentMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(agent_id, error = %e, "Failed to parse AgentMessage");
            return;
        }
    };

    match &msg {
        AgentMessage::SessionOutput { session_id, .. } => {
            // Broadcast terminal output to all watching browsers.
            connect_mgr.broadcast_to_browsers(session_id, text);
        }
        AgentMessage::SessionEvent { session_id, event } => {
            // Broadcast lifecycle event to browsers.
            connect_mgr.broadcast_to_browsers(session_id, text);

            // If the session ended, clean up the mapping.
            match event {
                trakkt_connect_protocol::wire::SessionEventKind::Exited { .. }
                | trakkt_connect_protocol::wire::SessionEventKind::Killed
                | trakkt_connect_protocol::wire::SessionEventKind::SpawnFailed { .. } => {
                    connect_mgr.unregister_session(session_id);
                }
                trakkt_connect_protocol::wire::SessionEventKind::Started => {}
            }
        }
        AgentMessage::ScrollbackDump { session_id, .. } => {
            // Broadcast scrollback to all watching browsers.
            connect_mgr.broadcast_to_browsers(session_id, text);
        }
        AgentMessage::SessionList { sessions } => {
            // Reconcile the session registry with the agent's reported sessions.
            for info in sessions {
                connect_mgr.register_session(&info.session_id, agent_id);
            }
        }
        AgentMessage::Ready {
            agent_version,
            hostname,
            os,
        } => {
            tracing::info!(
                agent_id,
                agent_version,
                hostname,
                os,
                "Agent reported ready"
            );
        }
        AgentMessage::Pong { .. } => {
            // Keepalive acknowledged. Could update last-seen timestamp in the
            // future for agent health monitoring.
        }
    }
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

    // Track which sessions this browser is subscribed to, so we can clean
    // up on disconnect.
    let subscribed_sessions: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let connect_mgr = state.connect_manager.clone();
    let subscribed_for_recv = subscribed_sessions.clone();
    let workspace_for_recv = workspace_id.clone();

    // We need a way to forward session output from the ConnectManager to
    // the browser. Each time the browser subscribes to a session, we get
    // an mpsc::Receiver. We merge all of these into a single stream using
    // an mpsc channel that aggregates output from all subscribed sessions.
    let (aggregate_tx, mut aggregate_rx) = tokio::sync::mpsc::channel::<String>(2048);

    // Outbound task: forward aggregated session output to browser WebSocket.
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(45));
        ping_interval.tick().await;

        loop {
            tokio::select! {
                msg = aggregate_rx.recv() => {
                    match msg {
                        Some(json) => {
                            if ws_sender.send(ws::Message::text(json)).await.is_err() {
                                break;
                            }
                        }
                        None => break,
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
    });

    // Inbound task: parse browser commands, relay to agents.
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_browser_message(
                        &text,
                        &workspace_for_recv,
                        browser_conn_id,
                        &connect_mgr,
                        &subscribed_for_recv,
                        &aggregate_tx,
                    );
                }
                ws::Message::Pong(_) => {}
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    // Cleanup: unsubscribe from all sessions.
    let sessions = subscribed_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    for session_id in &sessions {
        state
            .connect_manager
            .unsubscribe_browser(session_id, browser_conn_id);
    }
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

    Some((user_id, workspace_id))
}

/// Handle an inbound message from the browser.
///
/// The browser sends `ServerMessage` JSON. The server validates workspace
/// permissions and relays to the correct agent.
fn handle_browser_message(
    text: &str,
    workspace_id: &str,
    browser_conn_id: u64,
    connect_mgr: &trakkt_auth::connect_manager::ConnectManager,
    subscribed_sessions: &std::sync::Mutex<Vec<String>>,
    aggregate_tx: &tokio::sync::mpsc::Sender<String>,
) {
    let msg: ServerMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(browser_conn_id, error = %e, "Failed to parse browser ServerMessage");
            return;
        }
    };

    match msg {
        ServerMessage::SpawnSession { ref session_id, .. } => {
            // Find an agent in this workspace to handle the spawn.
            let agent_id = match connect_mgr.find_agent_for_workspace(workspace_id) {
                Some(id) => id,
                None => {
                    tracing::warn!(
                        workspace_id,
                        browser_conn_id,
                        "No agent connected for workspace"
                    );
                    return;
                }
            };

            // Register the session mapping before sending to agent.
            connect_mgr.register_session(session_id, &agent_id);

            // Subscribe this browser to the session output.
            let session_rx = connect_mgr.subscribe_browser(session_id, browser_conn_id);
            subscribed_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(session_id.clone());

            spawn_session_forwarder(session_rx, aggregate_tx.clone(), session_id.clone());

            // Relay the spawn command to the agent.
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to serialize SpawnSession for relay");
                    return;
                }
            };
            if !connect_mgr.send_to_agent(session_id, &json) {
                tracing::warn!(session_id, "Failed to relay SpawnSession to agent");
            }
        }

        ServerMessage::SessionInput { ref session_id, .. }
        | ServerMessage::SessionResize { ref session_id, .. }
        | ServerMessage::SessionKill { ref session_id, .. }
        | ServerMessage::ScrollbackRequest { ref session_id } => {
            // Verify the session belongs to this browser's workspace.
            match connect_mgr.get_session_workspace(session_id) {
                Some(ref session_ws) if session_ws == workspace_id => {}
                Some(_) => {
                    tracing::warn!(
                        browser_conn_id,
                        session_id,
                        workspace_id,
                        "Cross-workspace session access denied"
                    );
                    return;
                }
                None => {
                    tracing::warn!(browser_conn_id, session_id, "Unknown session");
                    return;
                }
            }

            // For ScrollbackRequest, also subscribe the browser if not already.
            if matches!(msg, ServerMessage::ScrollbackRequest { .. }) {
                let already_subscribed = subscribed_sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(session_id);

                if !already_subscribed {
                    let session_rx = connect_mgr.subscribe_browser(session_id, browser_conn_id);
                    subscribed_sessions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(session_id.clone());
                    spawn_session_forwarder(session_rx, aggregate_tx.clone(), session_id.clone());
                }
            }

            // Relay to the agent owning this session.
            // Re-serialize to ensure clean JSON (text may have extra whitespace etc).
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to serialize message for relay");
                    return;
                }
            };
            if !connect_mgr.send_to_agent(session_id, &json) {
                tracing::warn!(session_id = %session_id, "Failed to relay message to agent");
            }
        }

        ServerMessage::ListSessions => {
            // Find the agent for this workspace and send directly.
            let agent_id = match connect_mgr.find_agent_for_workspace(workspace_id) {
                Some(id) => id,
                None => {
                    tracing::warn!(workspace_id, "No agent connected for ListSessions");
                    return;
                }
            };

            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to serialize ListSessions");
                    return;
                }
            };

            if let Some(sender) = connect_mgr.get_agent_sender(&agent_id) {
                if let Err(e) = sender.try_send(json) {
                    tracing::warn!(agent_id, error = %e, "Failed to send ListSessions to agent");
                }
            }
        }

        ServerMessage::Ping { .. } => {
            // Browser shouldn't send pings, but harmless to ignore.
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a task that forwards session output from a per-session receiver to the
/// browser's aggregate output channel.
fn spawn_session_forwarder(
    session_rx: tokio::sync::mpsc::Receiver<String>,
    agg_tx: tokio::sync::mpsc::Sender<String>,
    session_id: String,
) {
    tokio::spawn(async move {
        let mut rx = session_rx;
        while let Some(json) = rx.recv().await {
            if agg_tx.send(json).await.is_err() {
                break;
            }
        }
        tracing::debug!(session_id = %session_id, "Session output forwarder ended");
    });
}

/// Close a WebSocket with a custom close code and reason.
async fn close_with_code(socket: ws::WebSocket, code: u16, reason: &str) {
    let (mut sender, _) = socket.split();
    let close_frame = ws::CloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    let _ = sender.send(ws::Message::Close(Some(close_frame))).await;
}
