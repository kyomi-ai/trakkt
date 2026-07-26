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

use trakkt_auth::websocket::manager::{CatchUpFlag, CatchUpGuard, ConnectionHandle, WsSender};
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

/// Rows fetched per `sync_delta` page.
///
/// Bounds the memory a single query materialises; `handle_sync_delta` keeps
/// paging until the backlog is drained, so this is not a limit on how much a
/// client can catch up on.
const DELTA_BATCH_SIZE: i64 = 10_000;

/// Hard ceiling on pages per `sync_delta` request (500,000 rows).
///
/// A backlog that large means the workspace is producing sync entries faster
/// than its clients consume them; keeping one connection streaming forever
/// would starve the rest. The client resumes from the watermark it was given.
const MAX_DELTA_BATCHES: usize = 50;

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
    let ConnectionHandle {
        id: connection_id,
        rx: mut manager_rx,
        tx: conn_tx,
        kill,
        catching_up,
    } = match state.ws_manager.connect(&user_id) {
        Ok(handle) => handle,
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
    let mut send_task = tokio::spawn(async move {
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

    let db_clone = state.db.clone();
    let user_id_for_recv = user_id.clone();
    let workspace_id_for_recv = workspace_id.clone();
    // `conn_tx` is moved into this task and lives as long as it does, so the
    // outbound channel stays open for the connection's whole lifetime. That is
    // why closing this socket takes the explicit `kill` signal plus the aborts
    // below — neither `disconnect()` nor a dropped sender can end it.
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_client_message(
                        &text,
                        &user_id_for_recv,
                        &workspace_id_for_recv,
                        &conn_tx,
                        &catching_up,
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

    // Wait for either side to finish, or for the manager to kill this
    // connection because it stopped draining its outbound buffer.
    // Select on `&mut` so both handles survive the select: a dropped
    // `JoinHandle` detaches its task instead of stopping it, which would leave
    // the socket open after the loser is discarded.
    tokio::select! {
        _ = &mut send_task => {}
        _ = &mut recv_task => {}
        _ = kill.notified() => {
            tracing::warn!(
                user_id = %user_id,
                connection_id,
                "WebSocket terminated by manager: client was not draining its sync stream"
            );
        }
    }

    // Stop both halves. Aborting drops the socket sink and stream, which is
    // what actually closes the connection; aborting a task that already
    // finished is a no-op.
    //
    // Trade-off: on a normal client-initiated close this pre-empts the outbound
    // task's courtesy close frame. The client is already gone in that path, and
    // trakkt-ui branches on its own `intentional_close` flag rather than the
    // close code, so nothing observes the difference today — but close-code
    // aware tooling would, hence this note.
    send_task.abort();
    recv_task.abort();

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
///
/// `conn_tx` addresses the connection that sent `text`. Every response belongs
/// to that connection alone — routing sync responses to the user would deliver
/// one browser's bootstrap stream (and its watermark) to the user's other
/// browsers, which would then skip changes they never received.
async fn handle_client_message(
    text: &str,
    user_id: &str,
    workspace_id: &str,
    conn_tx: &WsSender,
    catching_up: &CatchUpFlag,
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
            handle_sync_bootstrap(conn_tx, catching_up, db, user_id, workspace_id).await;
        }
        "sync_delta" => {
            let last_sync_id = msg
                .get("last_sync_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            handle_sync_delta(conn_tx, catching_up, db, user_id, workspace_id, last_sync_id).await;
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
///
/// The whole stream goes to `conn_tx` — the connection that asked for it — so
/// the `SyncComplete` watermark is only ever adopted by the client that
/// received the entities it covers.
async fn handle_sync_bootstrap(
    conn_tx: &WsSender,
    catching_up: &CatchUpFlag,
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
) {
    use trakkt_types::models::IssueFilters;
    use trakkt_types::sync::entity_types;

    // Held for the whole handler, so a live edit arriving while this stream
    // saturates the outbound channel drops its frame instead of killing a
    // connection that is mid-load. Dropped on every return below.
    let _catching_up = CatchUpGuard::new(catching_up);

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

    let notifications = trakkt_auth::notification_service::list_notifications(db, user_id, false, false, None, None, None, trakkt_auth::notification_service::DEFAULT_NOTIFICATION_LIMIT, 0)
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

    // Fetch milestones across all projects in the workspace.
    let milestones = trakkt_auth::project_service::list_milestones_for_workspace(db, workspace_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id, workspace_id, error = %e, "list_milestones_for_workspace failed during bootstrap");
            vec![]
        });

    // Fetch workspace settings snapshot.
    let ws_settings = trakkt_auth::workspace_service::get_workspace_settings_for_sync(db, workspace_id)
        .await;

    // 4. Get the current sync watermark.
    let latest_sync_id =
        trakkt_auth::sync_log_service::get_latest_sync_id(db, workspace_id)
            .await
            .unwrap_or(0);

    // 5. Stream each entity as a SyncAction with action=Insert.
    //
    // Each batch is serialized only when its turn comes and the stream stops at
    // the first failed send: once the socket is gone there is nothing to gain
    // from serializing and queueing the remaining batches. Bailing out also
    // skips the `SyncComplete` below, so a client that dropped mid-bootstrap
    // never records a watermark for entities it did not receive.
    macro_rules! stream_batch {
        ($entity_type:expr, $id_field:expr, $values:expr) => {
            if !stream_entities(conn_tx, workspace_id, $entity_type, $id_field, $values).await {
                tracing::debug!(
                    user_id,
                    workspace_id,
                    entity_type = $entity_type,
                    "sync_bootstrap aborted: connection closed"
                );
                return;
            }
        };
    }

    stream_batch!(entity_types::ISSUE, "issue_id", to_sync_values(&issues));
    stream_batch!(entity_types::LABEL, "label_id", to_sync_values(&labels));
    stream_batch!(entity_types::STATUS, "status_id", to_sync_values(&statuses));
    stream_batch!(entity_types::TEAM, "team_id", to_sync_values(&teams));
    stream_batch!(entity_types::PROJECT, "project_id", to_sync_values(&projects));
    stream_batch!(entity_types::VIEW, "view_id", to_sync_values(&views));
    stream_batch!(entity_types::FAVORITE, "favorite_id", to_sync_values(&favorites));
    stream_batch!(entity_types::NOTIFICATION, "notification_id", to_sync_values(&notifications));
    stream_batch!(entity_types::COMMENT, "comment_id", to_sync_values(&comments));
    stream_batch!(entity_types::PROJECT_MILESTONE, "milestone_id", to_sync_values(&milestones));

    // Workspace settings is a single entity (not a list).
    if let Some(ws_settings_val) = ws_settings {
        stream_batch!(entity_types::WORKSPACE_SETTINGS, "workspace_id", vec![ws_settings_val]);
    }

    // 6. Signal completion with the current sync watermark.
    send_sync_response(
        conn_tx,
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
    conn_tx: &WsSender,
    catching_up: &CatchUpFlag,
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    last_sync_id: i64,
) {
    use trakkt_types::sync::SyncResponse;

    // Same exemption as bootstrap: a long delta stream is catch-up traffic, not
    // a slow client. Dropped on every return below, including the SyncReset
    // early exits and the abort on a failed send.
    let _catching_up = CatchUpGuard::new(catching_up);

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
                send_sync_response(conn_tx, SyncResponse::SyncReset).await;
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
                send_sync_response(conn_tx, SyncResponse::SyncReset).await;
                return;
            }
        }
    }

    // 2. Drain the whole backlog, one page at a time.
    let outcome = drain_delta(
        conn_tx,
        db,
        user_id,
        workspace_id,
        last_sync_id,
        DELTA_BATCH_SIZE,
        MAX_DELTA_BATCHES,
    )
    .await;

    // 3. Stopping on the iteration cap means a backlog remains. Unlike the
    //    failure outcomes we still send the watermark, because everything we
    //    streamed did reach the client: the id is honest, and the client's next
    //    reconnect resumes from it rather than replaying half a million rows it
    //    already has. The error log is the signal that a workspace is producing
    //    sync entries faster than clients drain them.
    if let DrainOutcome::CapExhausted { cursor } = outcome {
        tracing::error!(
            user_id,
            workspace_id,
            cursor,
            max_batches = MAX_DELTA_BATCHES,
            "sync_delta hit its batch cap -- backlog remains, client must reconnect to continue"
        );
    }

    // 4. Signal completion once, with the last id actually streamed (or the
    //    requested one when the delta was empty). The failure outcomes yield no
    //    watermark at all — `drain_delta` has already logged why — and staying
    //    silent leaves the client's stored watermark where it was, so its next
    //    delta re-requests the same range.
    let Some(cursor) = outcome.watermark() else {
        return;
    };
    send_sync_response(
        conn_tx,
        SyncResponse::SyncComplete {
            last_sync_id: cursor,
        },
    )
    .await;

    tracing::debug!(user_id, workspace_id, cursor, "sync_delta complete");
}

/// How a `drain_delta` run ended, and — only where sending one is safe — the
/// watermark the client should be given.
///
/// The two failure outcomes carry no cursor on purpose. A watermark is only
/// honest when everything below it actually reached the client, and neither a
/// failed fetch nor a dead connection can promise that; making the cursor
/// unreachable in those cases is what keeps a `SyncComplete` off an abandoned
/// stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainOutcome {
    /// The log had nothing more to give: `cursor` covers the whole backlog.
    Drained { cursor: i64 },
    /// The iteration cap stopped the drain with a backlog still pending.
    /// `cursor` is the last id actually streamed — honest, but partial.
    CapExhausted { cursor: i64 },
    /// A page fetch failed, so what is still pending is unknown.
    FetchFailed,
    /// A frame could not be delivered: the client went away mid-stream.
    ClientGone,
}

impl DrainOutcome {
    /// The `SyncComplete` watermark this outcome permits, or `None` when the
    /// client must not be told it is caught up.
    fn watermark(&self) -> Option<i64> {
        match self {
            Self::Drained { cursor } | Self::CapExhausted { cursor } => Some(*cursor),
            Self::FetchFailed | Self::ClientGone => None,
        }
    }
}

/// Stream every sync log entry above `last_sync_id`, one page at a time,
/// advancing a local cursor.
///
/// A single capped fetch would stream the first page and then let the caller
/// hand the client a `SyncComplete` covering only what it received, so a client
/// more than one page behind would record itself as caught up and stay silently
/// stale until its next reconnect. The paging is server-side only: the client
/// sees one continuous stream and one watermark, so no protocol change is
/// needed to fix it.
///
/// Passing the authenticated `user_id` is what keeps per-user rows —
/// notifications, favorites, preferences, personal views — out of other
/// members' streams, and makes the delta dataset match what
/// `handle_sync_bootstrap` would have given the same user.
///
/// `batch_size` and `max_batches` are parameters rather than the constants
/// production calls them with so the cap-exhaustion path — the one branch that
/// deliberately hands back a partial watermark — is reachable from a test with
/// a handful of rows instead of half a million.
///
/// Sends nothing but `SyncAction` frames: the caller owns the decision of what,
/// if anything, closes the stream.
async fn drain_delta(
    conn_tx: &WsSender,
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    last_sync_id: i64,
    batch_size: i64,
    max_batches: usize,
) -> DrainOutcome {
    use trakkt_types::sync::SyncResponse;

    let mut cursor = last_sync_id;

    for _ in 0..max_batches {
        let entries = match trakkt_auth::sync_log_service::get_entries_since(
            db,
            workspace_id,
            user_id,
            cursor,
            batch_size,
        )
        .await
        {
            Ok(entries) => entries,
            Err(e) => {
                // A failed fetch leaves us with no idea what is still pending,
                // so the one thing we must not do is report a cursor: that
                // watermark would tell the client it is caught up while rows it
                // never saw remain in the log — the exact silent staleness this
                // loop exists to prevent. Returning empty-handed leaves the
                // client's stored watermark where it was, so its next delta
                // re-requests the same range.
                tracing::error!(
                    user_id,
                    workspace_id,
                    cursor,
                    error = %e,
                    "sync_delta fetch failed -- returning without a watermark"
                );
                return DrainOutcome::FetchFailed;
            }
        };

        // Stream each entry as a SyncAction message. Stop at the first failed
        // send — the remaining entries have nowhere to go, and reporting the
        // client gone instead of a cursor keeps its stored watermark honest.
        for entry in &entries {
            if !send_sync_response(conn_tx, SyncResponse::SyncAction(entry.clone())).await {
                tracing::debug!(
                    user_id,
                    workspace_id,
                    sync_id = entry.sync_id,
                    "sync_delta aborted: connection closed"
                );
                return DrainOutcome::ClientGone;
            }
        }

        // The last delivered id is both the next page's cursor and the
        // watermark we will hand back. A short page — including an empty one —
        // means the log had nothing more to give, so the backlog is drained and
        // there is no point paying for a query that can only come back empty.
        if let Some(last) = entries.last() {
            cursor = last.sync_id;
        }
        if (entries.len() as i64) < batch_size {
            return DrainOutcome::Drained { cursor };
        }
    }

    // Falling out of the loop means the iteration cap stopped us, so a backlog
    // remains behind an otherwise honest cursor.
    DrainOutcome::CapExhausted { cursor }
}

/// Serialize a batch of entities for streaming.
///
/// An entity that cannot be serialized cannot be put on the wire at all, so it
/// is logged and dropped rather than aborting the bootstrap.
fn to_sync_values<T: serde::Serialize>(items: &[T]) -> Vec<serde_json::Value> {
    items
        .iter()
        .filter_map(|item| match serde_json::to_value(item) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(
                    entity = std::any::type_name::<T>(),
                    error = %e,
                    "Failed to serialize entity for bootstrap, skipping"
                );
                None
            }
        })
        .collect()
}

/// Stream a batch of entities as individual `SyncAction(Insert)` messages.
///
/// Used by `handle_sync_bootstrap` to avoid copy-pasting the same loop for
/// each entity type. `id_field` is the JSON key that holds the entity's
/// primary key (e.g. `"issue_id"`, `"label_id"`).
///
/// Returns `false` as soon as a frame cannot be delivered, leaving the rest of
/// `items` unsent — the caller must abandon the stream rather than keep
/// serializing into a closed channel.
async fn stream_entities(
    conn_tx: &WsSender,
    workspace_id: &str,
    entity_type: &str,
    id_field: &str,
    items: Vec<serde_json::Value>,
) -> bool {
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
        if !send_sync_response(conn_tx, SyncResponse::SyncAction(action)).await {
            return false;
        }
    }

    true
}

/// Send a `SyncResponse` to the connection that requested it.
///
/// Writes straight to that connection's outbound channel with `.send().await`:
/// real backpressure (never dropped on a full buffer), and no fan-out to the
/// user's other connections. The requesting socket is by definition local to
/// this pod, so this never needs the user-routing or Redis layers.
///
/// Returns `false` when nothing was delivered — either the receiver is gone
/// (connection dead) or the response could not be serialized. Callers stop
/// streaming on `false`; a partial stream that still ended in `SyncComplete`
/// would hand the client a watermark covering data it never got.
async fn send_sync_response(
    conn_tx: &WsSender,
    response: trakkt_types::sync::SyncResponse,
) -> bool {
    let json = match serde_json::to_string(&response) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("Failed to serialize SyncResponse: {e}");
            return false;
        }
    };
    conn_tx.send(json).await.is_ok()
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

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use trakkt_types::sync::{entity_types, SyncActionType, SyncResponse};

    /// Minimal issue-shaped JSON — `stream_entities` only reads the id field
    /// and the timestamp, so no DB round-trip is needed.
    fn issue_value(issue_id: &str) -> serde_json::Value {
        serde_json::json!({
            "issue_id": issue_id,
            "title": "streamed issue",
            "updated_at": "2026-07-26T12:00:00Z",
        })
    }

    fn parse_frame(frame: &str) -> SyncResponse {
        serde_json::from_str(frame).expect("frame deserializes as a SyncResponse")
    }

    #[tokio::test]
    async fn stream_entities_writes_one_frame_per_item_in_order() {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);
        let items = vec![
            issue_value("iss_1"),
            issue_value("iss_2"),
            issue_value("iss_3"),
        ];

        assert!(
            stream_entities(&conn_tx, "ws_1", entity_types::ISSUE, "issue_id", items).await,
            "streaming to a live connection must report success"
        );
        drop(conn_tx);

        let mut streamed_ids = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            match parse_frame(&frame) {
                SyncResponse::SyncAction(action) => {
                    assert_eq!(action.entity_type, entity_types::ISSUE);
                    assert_eq!(action.workspace_id, "ws_1");
                    assert!(matches!(action.action, SyncActionType::Insert));
                    streamed_ids.push(action.entity_id);
                }
                other => panic!("expected SyncAction, got {other:?}"),
            }
        }

        assert_eq!(streamed_ids, vec!["iss_1", "iss_2", "iss_3"]);
    }

    #[tokio::test]
    async fn stream_entities_stops_early_when_the_connection_dies() {
        // Capacity 1 means the stream can never run ahead of the receiver by
        // more than one frame, so closing the channel after the first frame
        // lands squarely in the middle of the batch.
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(1);
        let items: Vec<serde_json::Value> = (0..50)
            .map(|i| issue_value(&format!("iss_{i}")))
            .collect();
        let item_count = items.len();

        let streamer = tokio::spawn(async move {
            stream_entities(&conn_tx, "ws_1", entity_types::ISSUE, "issue_id", items).await
        });

        // Receiving the first frame proves the stream is under way; closing then
        // fails every subsequent send.
        let first = conn_rx.recv().await.expect("first frame");
        match parse_frame(&first) {
            SyncResponse::SyncAction(action) => assert_eq!(action.entity_id, "iss_0"),
            other => panic!("expected SyncAction, got {other:?}"),
        }
        conn_rx.close();

        assert!(
            !streamer.await.expect("stream task completes"),
            "a dead connection must be reported to the caller"
        );

        let mut delivered = 1;
        while conn_rx.recv().await.is_some() {
            delivered += 1;
        }
        assert!(
            delivered < item_count,
            "stream must abandon the batch, but delivered all {item_count} items"
        );
        // Capacity 1 bounds the in-flight frames: the first frame, plus at most
        // one that was buffered before the close took effect.
        assert!(
            delivered <= 2,
            "expected the stream to stop within one frame of the close, delivered {delivered}"
        );
    }

    #[tokio::test]
    async fn send_sync_response_delivers_and_round_trips() {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(4);

        assert!(send_sync_response(&conn_tx, SyncResponse::SyncComplete { last_sync_id: 42 }).await);
        assert!(send_sync_response(&conn_tx, SyncResponse::SyncReset).await);

        let complete = conn_rx.recv().await.expect("SyncComplete frame");
        match parse_frame(&complete) {
            SyncResponse::SyncComplete { last_sync_id } => assert_eq!(last_sync_id, 42),
            other => panic!("expected SyncComplete, got {other:?}"),
        }

        let reset = conn_rx.recv().await.expect("SyncReset frame");
        assert!(matches!(parse_frame(&reset), SyncResponse::SyncReset));
    }

    #[tokio::test]
    async fn send_sync_response_reports_failure_when_the_receiver_is_gone() {
        let (conn_tx, conn_rx) = mpsc::channel::<String>(4);
        drop(conn_rx);

        assert!(!send_sync_response(&conn_tx, SyncResponse::SyncComplete { last_sync_id: 7 }).await);
        assert!(!send_sync_response(&conn_tx, SyncResponse::SyncReset).await);
    }

    /// A workspace with `count` sync_log entries to stream back.
    async fn db_with_sync_entries(workspace_id: &str, count: usize) -> trakkt_core::DbPool {
        let db = trakkt_core::DbPool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");

        for i in 0..count {
            trakkt_auth::sync_log_service::write_sync_entry(
                &db,
                entity_types::ISSUE,
                &format!("iss_{i}"),
                workspace_id,
                None,
                SyncActionType::Update,
                None,
            )
            .await
            .expect("write sync entry");
        }

        db
    }

    /// Wait for a connection to be flagged as catching up. Fails the test
    /// rather than hanging if the handler never sets it.
    async fn wait_until_flagged(flag: &CatchUpFlag) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !flag.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connection was never flagged as catching up");
    }

    /// Bootstrap is the stream that provoked this exemption in the first place
    /// — an unpaginated workspace load — so its flag has to hold for the whole
    /// handler, not just around the sends.
    #[tokio::test]
    async fn sync_bootstrap_flags_the_connection_for_the_whole_stream() {
        let db = trakkt_core::DbPool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");
        let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(1);
        // Occupy the only slot so the handler cannot finish before we look.
        conn_tx.try_send("occupied".to_string()).expect("prefill");

        let flag = Arc::clone(&catching_up);
        let stream = tokio::spawn(async move {
            handle_sync_bootstrap(&conn_tx, &flag, &db, "usr_1", "ws_empty").await;
        });

        wait_until_flagged(&catching_up).await;

        // Make room so the bootstrap can finish.
        assert_eq!(conn_rx.recv().await.as_deref(), Some("occupied"));
        stream.await.expect("bootstrap task");

        assert!(
            !catching_up.load(Ordering::Acquire),
            "the exemption must not outlive the bootstrap"
        );
    }

    #[tokio::test]
    async fn sync_delta_clears_the_catch_up_flag_when_the_stream_finishes() {
        let workspace_id = "ws_catch_up";
        let db = db_with_sync_entries(workspace_id, 3).await;
        let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);

        handle_sync_delta(&conn_tx, &catching_up, &db, "usr_1", workspace_id, 0).await;

        assert!(
            !catching_up.load(Ordering::Acquire),
            "the catch-up exemption must not outlive the stream"
        );

        // Three actions then the watermark — i.e. the stream really ran.
        for _ in 0..3 {
            let frame = conn_rx.recv().await.expect("SyncAction frame");
            assert!(matches!(parse_frame(&frame), SyncResponse::SyncAction(_)));
        }
        let complete = conn_rx.recv().await.expect("SyncComplete frame");
        assert!(matches!(
            parse_frame(&complete),
            SyncResponse::SyncComplete { .. }
        ));
    }

    #[tokio::test]
    async fn sync_delta_clears_the_catch_up_flag_when_the_stream_aborts_early() {
        let workspace_id = "ws_catch_up_abort";
        let db = db_with_sync_entries(workspace_id, 5).await;
        let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));
        // Capacity 1 keeps the handler blocked on a send it cannot complete
        // until the receiver goes away, which is the abort path under test.
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(1);

        let flag = Arc::clone(&catching_up);
        let stream = tokio::spawn(async move {
            handle_sync_delta(&conn_tx, &flag, &db, "usr_1", workspace_id, 0).await;
        });

        // Receiving a frame proves the stream started, so the flag is set and
        // stays set until the handler returns — and it cannot return yet,
        // because four more entries do not fit in a one-slot channel.
        conn_rx.recv().await.expect("first SyncAction frame");
        assert!(
            catching_up.load(Ordering::Acquire),
            "a connection being caught up must be flagged for the whole stream"
        );

        drop(conn_rx);
        stream.await.expect("stream task");

        assert!(
            !catching_up.load(Ordering::Acquire),
            "aborting mid-stream must still clear the catch-up exemption"
        );
    }

    /// Outbound capacity used by the delta tests.
    ///
    /// Small on purpose: a page is 10,000 frames, so the handler blocks on
    /// `send` constantly and the receiver drives it. That is what makes these
    /// tests exercise the page boundaries under real backpressure rather than
    /// letting the whole stream sit in a buffer.
    const TEST_CHANNEL_CAPACITY: usize = 64;

    /// Run a sync stream to completion, collecting every frame it sends.
    ///
    /// The streams await `conn_tx.send()` on a bounded channel, so calling one
    /// inline and draining afterwards deadlocks the moment the channel fills.
    /// Draining concurrently is the only way to run a multi-page delta — and it
    /// is also what the real connection does.
    ///
    /// `run` takes the sender by value so it is dropped when the stream returns;
    /// that is what closes the channel and ends the drain loop below, which is
    /// why this returns exactly when the stream is over.
    async fn collect_stream_frames<Fut, T>(
        run: impl FnOnce(WsSender) -> Fut,
    ) -> (T, Vec<SyncResponse>)
    where
        Fut: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(TEST_CHANNEL_CAPACITY);
        let stream = tokio::spawn(run(conn_tx));

        let mut frames = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            frames.push(parse_frame(&frame));
        }
        let result = stream.await.expect("sync stream task completes");

        (result, frames)
    }

    /// Run `handle_sync_delta` to completion, collecting every frame it sends.
    async fn collect_delta_frames(
        db: trakkt_core::DbPool,
        workspace_id: &str,
        last_sync_id: i64,
    ) -> Vec<SyncResponse> {
        let workspace_id = workspace_id.to_string();

        let ((), frames) = collect_stream_frames(move |conn_tx| async move {
            let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));
            handle_sync_delta(&conn_tx, &catching_up, &db, "usr_1", &workspace_id, last_sync_id)
                .await;
        })
        .await;

        frames
    }

    /// Run `drain_delta` directly with test-sized paging, collecting both the
    /// outcome and every frame it streamed.
    async fn collect_drain(
        db: trakkt_core::DbPool,
        workspace_id: &str,
        last_sync_id: i64,
        batch_size: i64,
        max_batches: usize,
    ) -> (DrainOutcome, Vec<SyncResponse>) {
        let workspace_id = workspace_id.to_string();

        collect_stream_frames(move |conn_tx| async move {
            drain_delta(
                &conn_tx,
                &db,
                "usr_1",
                &workspace_id,
                last_sync_id,
                batch_size,
                max_batches,
            )
            .await
        })
        .await
    }

    /// The `sync_id`s of a captured stream, in the order they were delivered.
    fn streamed_sync_ids(frames: &[SyncResponse]) -> Vec<i64> {
        frames
            .iter()
            .filter_map(|frame| match frame {
                SyncResponse::SyncAction(action) => Some(action.sync_id),
                _ => None,
            })
            .collect()
    }

    /// The defect this whole loop exists to fix: a backlog wider than one page
    /// used to be truncated to the first page and then closed with a
    /// `SyncComplete`, which the client stored as "fully caught up".
    #[tokio::test]
    async fn sync_delta_drains_a_backlog_larger_than_one_page() {
        let workspace_id = "ws_multi_batch";
        let entry_count = 12_000usize;
        assert!(
            entry_count as i64 > DELTA_BATCH_SIZE,
            "this test is meaningless unless the backlog spans more than one page"
        );

        let db = db_with_sync_entries(workspace_id, entry_count).await;
        let max_sync_id = trakkt_auth::sync_log_service::get_latest_sync_id(&db, workspace_id)
            .await
            .expect("latest sync_id");

        let frames = collect_delta_frames(db, workspace_id, 0).await;

        // Splitting off the tail is also the "nothing follows it" assertion:
        // anything sent after the watermark would land in `streamed`.
        let (watermark, streamed) = frames.split_last().expect("at least one frame");
        assert_eq!(
            streamed.len(),
            entry_count,
            "every seeded entry must reach the client, not just the first page"
        );
        assert!(
            streamed
                .iter()
                .all(|frame| matches!(frame, SyncResponse::SyncAction(_))),
            "only the final frame may be anything other than a SyncAction"
        );
        match watermark {
            SyncResponse::SyncComplete { last_sync_id } => assert_eq!(
                *last_sync_id, max_sync_id,
                "the watermark must cover the whole backlog, not the first page"
            ),
            other => panic!("expected a trailing SyncComplete, got {other:?}"),
        }
    }

    /// The page boundary is where a cursor off-by-one shows up: reusing the
    /// last id as the next `since` would replay it, skipping past it would drop
    /// the row after it. Neither is visible from a single-page delta.
    #[tokio::test]
    async fn sync_delta_streams_every_entry_exactly_once_across_page_boundaries() {
        let workspace_id = "ws_batch_boundary";
        let entry_count = 12_000usize;
        let db = db_with_sync_entries(workspace_id, entry_count).await;
        let max_sync_id = trakkt_auth::sync_log_service::get_latest_sync_id(&db, workspace_id)
            .await
            .expect("latest sync_id");

        let frames = collect_delta_frames(db, workspace_id, 0).await;
        let ids = streamed_sync_ids(&frames);

        assert_eq!(ids.len(), entry_count, "wrong number of entries streamed");
        for pair in ids.windows(2) {
            assert!(
                pair[1] > pair[0],
                "sync_ids must be strictly ascending, got {} then {}",
                pair[0],
                pair[1]
            );
        }
        let first = *ids.first().expect("a non-empty stream");
        let last = *ids.last().expect("a non-empty stream");
        // Strictly ascending rules out repeats; a contiguous span from the
        // first to the last id rules out anything being skipped between them.
        assert_eq!(
            ids.len() as i64,
            last - first + 1,
            "sync_ids {first}..={last} must be gap-free, but only {} were streamed",
            ids.len()
        );
        assert_eq!(last, max_sync_id, "the stream must reach the end of the log");
    }

    #[tokio::test]
    async fn sync_delta_streams_a_single_page_backlog_unchanged() {
        let workspace_id = "ws_single_batch";
        let db = db_with_sync_entries(workspace_id, 3).await;
        let max_sync_id = trakkt_auth::sync_log_service::get_latest_sync_id(&db, workspace_id)
            .await
            .expect("latest sync_id");

        let frames = collect_delta_frames(db, workspace_id, 0).await;

        assert_eq!(streamed_sync_ids(&frames).len(), 3, "expected three actions");
        assert_eq!(frames.len(), 4, "three actions and one watermark, nothing else");
        match frames.last().expect("a trailing frame") {
            SyncResponse::SyncComplete { last_sync_id } => assert_eq!(*last_sync_id, max_sync_id),
            other => panic!("expected a trailing SyncComplete, got {other:?}"),
        }
    }

    /// An already-current client still needs its watermark echoed back,
    /// otherwise it would treat the silence as an unfinished sync.
    #[tokio::test]
    async fn sync_delta_with_nothing_new_echoes_the_requested_watermark() {
        let workspace_id = "ws_no_new_entries";
        let db = db_with_sync_entries(workspace_id, 3).await;
        let max_sync_id = trakkt_auth::sync_log_service::get_latest_sync_id(&db, workspace_id)
            .await
            .expect("latest sync_id");

        let frames = collect_delta_frames(db, workspace_id, max_sync_id).await;

        assert_eq!(frames.len(), 1, "an empty delta is one frame: the watermark");
        match &frames[0] {
            SyncResponse::SyncComplete { last_sync_id } => assert_eq!(
                *last_sync_id, max_sync_id,
                "an empty delta must echo the requested id, not reset the client"
            ),
            other => panic!("expected SyncComplete, got {other:?}"),
        }
    }

    /// Dropping out mid-stream must not produce a watermark — including when
    /// the drop happens after the first page, where the handler is between
    /// queries rather than partway through one list.
    #[tokio::test]
    async fn sync_delta_abandons_a_multi_page_stream_when_the_client_disconnects() {
        let workspace_id = "ws_abort_second_batch";
        // More than one page, and far enough past the boundary that the unsent
        // remainder cannot fit in the channel: the handler is provably still
        // streaming when the receiver goes away.
        let entry_count = 10_500usize;
        let db = db_with_sync_entries(workspace_id, entry_count).await;
        let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(TEST_CHANNEL_CAPACITY);

        let flag = Arc::clone(&catching_up);
        let handler = tokio::spawn(async move {
            handle_sync_delta(&conn_tx, &flag, &db, "usr_1", workspace_id, 0).await;
        });

        // Read one frame past the page boundary, so the abort lands in the
        // second page rather than the first.
        let read_before_disconnect = DELTA_BATCH_SIZE as usize + 1;
        let mut delivered = Vec::with_capacity(read_before_disconnect);
        for _ in 0..read_before_disconnect {
            let frame = conn_rx.recv().await.expect("frame before the disconnect");
            delivered.push(parse_frame(&frame));
        }

        // Closing rather than dropping fails every subsequent send the same way
        // a dead connection does, while still letting us inspect what was
        // already buffered — which is how we can prove no watermark was sent.
        conn_rx.close();
        handler.await.expect("sync_delta task returns cleanly");
        while let Some(frame) = conn_rx.recv().await {
            delivered.push(parse_frame(&frame));
        }
        drop(conn_rx);

        assert!(
            delivered.len() < entry_count,
            "the stream must be abandoned, but all {entry_count} entries were delivered"
        );
        assert!(
            delivered
                .iter()
                .all(|frame| matches!(frame, SyncResponse::SyncAction(_))),
            "a client that disconnected mid-drain must never receive a watermark"
        );
        assert!(
            !catching_up.load(Ordering::Acquire),
            "aborting across a page boundary must still clear the catch-up exemption"
        );
    }

    /// The `sync_id`s of every seeded entry, in log order. Read back rather
    /// than assumed: the assertions below are about which rows were streamed,
    /// which is only meaningful against the ids the log actually assigned.
    async fn seeded_sync_ids(db: &trakkt_core::DbPool, workspace_id: &str) -> Vec<i64> {
        trakkt_auth::sync_log_service::get_entries_since(db, workspace_id, "usr_1", 0, 1_000)
            .await
            .expect("seeded entries")
            .iter()
            .map(|entry| entry.sync_id)
            .collect()
    }

    /// The cap is the one branch that deliberately hands back a *partial*
    /// watermark, and the one branch whose regression re-creates TRA-9922
    /// exactly: a client told it is caught up while rows it never saw remain in
    /// the log. Production's 50 x 10,000 cap needs half a million rows to
    /// reach, which is why `drain_delta` takes its paging as parameters — this
    /// calls it with a cap a test can afford.
    #[tokio::test]
    async fn drain_delta_stops_at_the_batch_cap_with_a_partial_watermark() {
        let workspace_id = "ws_batch_cap";
        let db = db_with_sync_entries(workspace_id, 10).await;
        let seeded = seeded_sync_ids(&db, workspace_id).await;
        assert_eq!(seeded.len(), 10, "seeding must produce ten entries");

        let batch_size = 3i64;
        let max_batches = 2usize;
        let capped_at = batch_size as usize * max_batches;

        let (outcome, frames) = collect_drain(db, workspace_id, 0, batch_size, max_batches).await;

        let ids = streamed_sync_ids(&frames);
        assert_eq!(
            ids.as_slice(),
            &seeded[..capped_at],
            "the cap must stop the drain after exactly {max_batches} pages of {batch_size}"
        );
        assert_eq!(
            frames.len(),
            capped_at,
            "drain_delta streams actions only -- the caller decides what closes the stream"
        );

        let partial_cursor = seeded[capped_at - 1];
        let max_sync_id = *seeded.last().expect("a non-empty log");
        assert_ne!(
            partial_cursor, max_sync_id,
            "this test is meaningless unless the cap left entries behind"
        );
        assert_eq!(
            outcome,
            DrainOutcome::CapExhausted {
                cursor: partial_cursor
            },
            "hitting the cap must report a backlog remaining, not a drained log"
        );

        // The caller-level consequence: this partial id, not the log's maximum,
        // is what `handle_sync_delta` puts in the client's `SyncComplete`, so
        // the client resumes from the last row it saw instead of stepping over
        // the backlog it never received.
        assert_eq!(
            outcome.watermark(),
            Some(partial_cursor),
            "the client's watermark must be the last row actually streamed"
        );
    }

    /// The same small-page path with cap headroom to spare. Raising only
    /// `max_batches` turns the identical backlog into a fully drained one,
    /// which is what proves the cap — and not a short page or some other
    /// short-circuit — is what truncated the stream above.
    #[tokio::test]
    async fn drain_delta_below_the_batch_cap_drains_the_whole_backlog() {
        let workspace_id = "ws_batch_cap_headroom";
        let db = db_with_sync_entries(workspace_id, 10).await;
        let seeded = seeded_sync_ids(&db, workspace_id).await;
        let max_sync_id = *seeded.last().expect("a non-empty log");

        let (outcome, frames) = collect_drain(db, workspace_id, 0, 3, 10).await;

        assert_eq!(
            streamed_sync_ids(&frames),
            seeded,
            "every seeded entry must be streamed when the cap is not reached"
        );
        assert_eq!(
            outcome,
            DrainOutcome::Drained {
                cursor: max_sync_id
            },
            "a backlog that fits inside the cap must report itself drained"
        );
        assert_eq!(
            outcome.watermark(),
            Some(max_sync_id),
            "a drained backlog hands the client the end of the log"
        );
    }

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
