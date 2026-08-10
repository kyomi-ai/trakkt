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
use trakkt_types::models::{
    Comment, Favorite, IssueWithDetails, Label, Notification, Project, ProjectMilestone, Status,
    Team, View, WorkspaceSettingsSnapshot,
};
use trakkt_types::sync::SyncEntity;

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
/// Streams the whole workspace dataset as `SyncAction` messages with
/// `action = Insert`, then closes with a `SyncComplete` carrying the current
/// `latest_sync_id`. Clients should store this ID and use `sync_delta` for
/// subsequent reconnects.
///
/// The whole stream goes to `conn_tx` — the connection that asked for it — so
/// the `SyncComplete` watermark is only ever adopted by the client that
/// received the entities it covers.
///
/// Any failure — a query that errors, an entity that cannot be encoded, a
/// connection that goes away — ends the handler *without* a `SyncComplete`.
/// That is the one invariant this handler exists to hold, and it is why every
/// step below returns rather than degrades: a client handed a short dataset
/// plus a full watermark records itself as caught up, and no future
/// `sync_delta` will ever backfill what it missed — only wiping its IndexedDB
/// will. A client handed nothing retries on its next reconnect. `handle_sync_delta`
/// makes the same trade for the same reason.
///
/// Public because the per-connection addressing is a property worth asserting
/// from outside this module: `tests/sync_ws.rs` drives it against a real
/// `WebSocketManager` with several connections registered for one user, which
/// is the only place the fan-out bug this doc comment describes can be seen.
pub async fn handle_sync_bootstrap(
    conn_tx: &WsSender,
    catching_up: &CatchUpFlag,
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
) {
    // Held for the whole handler, so a live edit arriving while this stream
    // saturates the outbound channel drops its frame instead of killing a
    // connection that is mid-load. Dropped on every return below.
    let _catching_up = CatchUpGuard::new(catching_up);

    tracing::debug!(user_id, workspace_id, "Handling sync_bootstrap");

    // 1. Read the sync watermark before any of the data it will be handed out
    //    alongside. The cursor handed to a client must never exceed the data
    //    actually streamed; a too-low watermark only causes harmless idempotent
    //    re-delivery on the next delta. Reading it last would invert that: a
    //    mutation committing after the entity queries but before the watermark
    //    read is covered by the cursor yet missing from the stream, so it is
    //    below every future delta's floor and the client never sees it again.
    let latest_sync_id = trakkt_auth::sync_log_service::get_latest_sync_id(db, workspace_id)
        .await
        .unwrap_or(0);

    // 2. Read the whole dataset before any of it goes on the wire.
    let data = match fetch_bootstrap_data(db, user_id, workspace_id).await {
        Ok(data) => data,
        // `fetch_bootstrap_data` has already logged which read failed. Returning
        // here is the whole point: no watermark is sent, so the client keeps the
        // cursor it had and asks again on its next reconnect. Substituting an
        // empty list for the failed read instead — as this handler used to —
        // streams a workspace that looks emptied and then certifies it as
        // complete.
        Err(_) => return,
    };

    // 3. Assemble the batches in the order clients receive them. Nothing is
    //    serialized yet; `stream_bootstrap` converts each batch when its turn
    //    comes, so an abandoned stream never pays for the batches behind it.
    let batches = bootstrap_batches(&data);

    // 4. Stream them, and close with the watermark read back in step 1 only if
    //    every one of them made it.
    stream_bootstrap(conn_tx, user_id, workspace_id, latest_sync_id, batches).await;
}

/// The batches one bootstrap streams, in the order clients receive them.
///
/// Every batch is named only by the list it carries. Its entity type and each
/// row's `entity_id` come from the element type's [`SyncEntity`] impl (see the
/// table at the foot of `crates/trakkt-types/src/models.rs`), so there is no
/// per-entity string here to disagree with the model — which is what the
/// `"issue_id"`-style literals that used to sit beside each list could do, and
/// silently.
///
/// Split out of [`handle_sync_bootstrap`] so the list itself is testable: the
/// derivation removes the mistyped-id class, but "a list mentioned twice, and
/// another not at all" is a plain editing slip that no type can catch, and
/// `bootstrap_streams_every_entity_type_exactly_once` is what catches it.
fn bootstrap_batches(data: &BootstrapData) -> Vec<PendingBatch<'_>> {
    let mut batches = vec![
        PendingBatch::new(&data.issues),
        PendingBatch::new(&data.labels),
        PendingBatch::new(&data.statuses),
        PendingBatch::new(&data.teams),
        PendingBatch::new(&data.projects),
        PendingBatch::new(&data.views),
        PendingBatch::new(&data.favorites),
        PendingBatch::new(&data.notifications),
        PendingBatch::new(&data.comments),
        PendingBatch::new(&data.milestones),
    ];

    // Workspace settings is a single entity, not a list — and a workspace with
    // no row simply has none to stream, which is not a failure.
    if let Some(settings) = &data.workspace_settings {
        batches.push(PendingBatch::new(std::slice::from_ref(settings)));
    }

    batches
}

/// Everything a bootstrap streams, read before any of it is put on the wire.
///
/// Read as a unit so that a failure in any single query aborts the bootstrap
/// before the client sees a single frame, rather than silently shrinking the
/// dataset the trailing watermark then certifies as complete.
struct BootstrapData {
    issues: Vec<IssueWithDetails>,
    labels: Vec<Label>,
    statuses: Vec<Status>,
    teams: Vec<Team>,
    projects: Vec<Project>,
    views: Vec<View>,
    favorites: Vec<Favorite>,
    notifications: Vec<Notification>,
    comments: Vec<Comment>,
    milestones: Vec<ProjectMilestone>,
    /// `None` means the workspace has no row at all — a real answer, streamed
    /// as "no settings entity". A read that *failed* never reaches this field:
    /// it comes back as `Err` from [`fetch_bootstrap_data`].
    workspace_settings: Option<WorkspaceSettingsSnapshot>,
}

/// Await one bootstrap read, naming it in the log if it fails.
///
/// Every read aborts the bootstrap the same way, so the only thing worth
/// varying between them is `query` — knowing *which* read failed is the entire
/// diagnostic value, and it is what eleven copy-pasted `match` arms were
/// spending eleven blocks of code to say. The error is returned unchanged so
/// the caller's `?` does the aborting.
///
/// Logged at `error!`: a failed read no longer degrades the response, it ends
/// it, and a bootstrap that never completes is not a warning-level event.
async fn bootstrap_read<T>(
    query: &'static str,
    user_id: &str,
    workspace_id: &str,
    read: impl std::future::Future<Output = trakkt_core::Result<T>>,
) -> trakkt_core::Result<T> {
    read.await.map_err(|e| {
        tracing::error!(
            user_id,
            workspace_id,
            query,
            error = %e,
            "sync_bootstrap read failed -- aborting without a watermark"
        );
        e
    })
}

/// Read every entity list a bootstrap streams, in the order it streams them.
///
/// Returns `Err` as soon as any read fails; the caller must abandon the
/// bootstrap rather than stream what it did manage to read.
async fn fetch_bootstrap_data(
    db: &trakkt_core::DbPool,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<BootstrapData> {
    use trakkt_types::models::IssueFilters;

    // Archived issues are excluded from bootstrap.
    let issue_filters = IssueFilters {
        include_archived: Some(false),
        ..Default::default()
    };

    let issues = bootstrap_read(
        "list_issues",
        user_id,
        workspace_id,
        trakkt_auth::issue_service::list_issues(db, workspace_id, None, &issue_filters),
    )
    .await?;

    let labels = bootstrap_read(
        "list_labels",
        user_id,
        workspace_id,
        trakkt_auth::label_service::list_labels(db, workspace_id),
    )
    .await?;

    let statuses = bootstrap_read(
        "list_statuses",
        user_id,
        workspace_id,
        trakkt_auth::status_service::list_statuses(db, workspace_id, None),
    )
    .await?;

    let teams = bootstrap_read(
        "list_teams",
        user_id,
        workspace_id,
        trakkt_auth::team_service::list_teams(db, workspace_id, Some(user_id)),
    )
    .await?;

    let projects = bootstrap_read(
        "list_projects",
        user_id,
        workspace_id,
        trakkt_auth::project_service::list_projects(db, workspace_id),
    )
    .await?;

    let views = bootstrap_read(
        "list_views",
        user_id,
        workspace_id,
        trakkt_auth::view_service::list_views(db, workspace_id, user_id, None),
    )
    .await?;

    let favorites = bootstrap_read(
        "list_favorites",
        user_id,
        workspace_id,
        trakkt_auth::favorite_service::list_favorites(db, user_id, workspace_id),
    )
    .await?;

    let notifications = bootstrap_read(
        "list_notifications",
        user_id,
        workspace_id,
        trakkt_auth::notification_service::list_notifications(
            db,
            user_id,
            false,
            false,
            None,
            None,
            None,
            trakkt_auth::notification_service::DEFAULT_NOTIFICATION_LIMIT,
            0,
        ),
    )
    .await?;

    let comments = bootstrap_read(
        "list_comments_for_workspace",
        user_id,
        workspace_id,
        trakkt_auth::comment_service::list_comments_for_workspace(db, workspace_id),
    )
    .await?;

    // Milestones across all projects in the workspace.
    let milestones = bootstrap_read(
        "list_milestones_for_workspace",
        user_id,
        workspace_id,
        trakkt_auth::project_service::list_milestones_for_workspace(db, workspace_id),
    )
    .await?;

    let workspace_settings = bootstrap_read(
        "get_workspace_settings_for_sync",
        user_id,
        workspace_id,
        trakkt_auth::workspace_service::get_workspace_settings_for_sync(db, workspace_id),
    )
    .await?;

    Ok(BootstrapData {
        issues,
        labels,
        statuses,
        teams,
        projects,
        views,
        favorites,
        notifications,
        comments,
        milestones,
        workspace_settings,
    })
}

/// One entity ready for the wire: the id the client will address it by, and the
/// payload that id addresses.
///
/// The two are carried side by side, and separately, because the id must not be
/// recovered from the payload. Reading it back out of the encoded JSON is what
/// needed a key to read it *under*, and that key was the string literal this
/// refactor removed. Here `entity_id` is a `String` taken from
/// [`SyncEntity::entity_id`] before the payload was ever built, so the payload's
/// key names — including any `#[serde(rename)]` on them — cannot affect
/// addressing at all.
struct AddressedEntity {
    /// Becomes `SyncAction.entity_id`, which the client's cache keys on.
    entity_id: String,
    /// Becomes `SyncAction.data`.
    payload: serde_json::Value,
}

/// Turn one entity list into the addressed frames it will be streamed as.
///
/// Boxed because the batches differ in element type: erasing that behind a
/// closure is what lets [`stream_bootstrap`] hold them in one list and write
/// the serialize-stream-abort logic once instead of once per entity type. It
/// also keeps serialization lazy, so peak memory is one batch of JSON rather
/// than all eleven at once, and an abandoned stream never encodes the batches
/// behind it.
///
/// The erasure is why [`PendingBatch::entity_type`] is a plain `&'static str`
/// rather than a `T::ENTITY_TYPE` read at the point of use: once the element
/// type is behind the closure there is no `T` left to ask. It is copied off the
/// same `T` in [`PendingBatch::new`], in the same expression that builds the
/// closure, so the pair cannot be assembled from two different types.
type BatchSerializer<'a> =
    Box<dyn FnOnce() -> Result<Vec<AddressedEntity>, serde_json::Error> + Send + 'a>;

/// One entity list waiting for its turn on the wire.
struct PendingBatch<'a> {
    /// Entity type carried on every frame in the batch, from `T::ENTITY_TYPE`.
    entity_type: &'static str,
    /// Encodes and addresses the batch, or reports the entity that could not be
    /// encoded.
    serialize: BatchSerializer<'a>,
}

impl<'a> PendingBatch<'a> {
    /// A batch of `items`, typed by what they are.
    ///
    /// Takes no `entity_type` and no id field: both come from `T`'s
    /// [`SyncEntity`] impl. That is the whole of TRA-10004 — a caller cannot
    /// hand a list of issues the label entity type, or name an id field that
    /// does not exist on the model, because there is no parameter left to say
    /// either in.
    fn new<T: SyncEntity + Sync>(items: &'a [T]) -> Self {
        Self {
            entity_type: T::ENTITY_TYPE,
            serialize: Box::new(move || to_addressed_entities(items)),
        }
    }
}

/// Stream every batch, then close the stream with `latest_sync_id`.
///
/// The watermark is sent only after every batch has been encoded and delivered
/// in full. All three failure modes — an entity that will not serialize, an
/// entity that carries no usable id (see [`stream_entities`]), and a connection
/// that stopped accepting frames — return early instead, leaving the client's
/// stored cursor exactly where it was so its next reconnect asks for the same
/// data again. Only the connection going away is routine; the other two are
/// defects and are logged at `error!`.
async fn stream_bootstrap(
    conn_tx: &WsSender,
    user_id: &str,
    workspace_id: &str,
    latest_sync_id: i64,
    batches: Vec<PendingBatch<'_>>,
) {
    for batch in batches {
        let entities = match (batch.serialize)() {
            Ok(entities) => entities,
            // `to_addressed_entities` has already logged the entity and error.
            Err(_) => {
                tracing::error!(
                    user_id,
                    workspace_id,
                    entity_type = batch.entity_type,
                    "sync_bootstrap aborted: batch could not be serialized"
                );
                return;
            }
        };

        match stream_entities(conn_tx, workspace_id, batch.entity_type, entities).await {
            StreamOutcome::Delivered => {}
            StreamOutcome::ClientGone => {
                tracing::debug!(
                    user_id,
                    workspace_id,
                    entity_type = batch.entity_type,
                    "sync_bootstrap aborted: connection closed"
                );
                return;
            }
            StreamOutcome::UnusableEntity => {
                // `stream_entities` has already logged the batch and the
                // position within it of the row whose stored id is empty. This
                // line records the consequence: the watermark below is not
                // sent, so the client keeps its old cursor and asks for the
                // whole dataset again next reconnect.
                tracing::error!(
                    user_id,
                    workspace_id,
                    entity_type = batch.entity_type,
                    "sync_bootstrap aborted: entity could not be addressed"
                );
                return;
            }
        }
    }

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
///
/// Public for the same reason as [`handle_sync_bootstrap`]: `tests/sync_ws.rs`
/// asserts that a `SyncReset` reaches only the connection that asked for it,
/// which needs a real multi-connection `WebSocketManager` to observe.
///
/// # Why there is no unaddressable-entity guard here
///
/// `stream_entities` aborts a bootstrap batch whose entity has an empty
/// `entity_id`, because the trailing watermark would otherwise certify a dataset
/// that entity is missing from. TRA-10005 audited whether this handler needs the
/// same guard and found it does not. The reasoning is recorded here so the next
/// reader does not have to redo it, and so that adding one is a decision rather
/// than an oversight being corrected.
///
/// 1. **These ids are a typed column, not a lookup.** `SyncAction.entity_id` on
///    this path is `sync_log.entity_id`, decoded by `SyncLogRow` as a `String`
///    from a column declared `NOT NULL` on both dialects — `VARCHAR(100)` on
///    Postgres, `TEXT` on SQLite. The JSON-field lookup that produced TRA-9960's
///    empty ids has no counterpart here; nothing between the column and the
///    frame can substitute a default.
///
/// 2. **No writer can put an empty string in that column.** All 60 production
///    `entity_id` arguments to `write_sync_entry_in_tx`, `commit_and_deliver`
///    and `SyncBatch::record` are one of three shapes: an id minted in the same
///    function as `Uuid::new_v4().to_string()`; a primary-key column read back
///    off a row the same transaction has already proved exists; or a `format!`
///    composite (`{project_id}:{user_id}`, `{issue_id}:{attachment_id}`) that
///    always contains its separator. The caller-supplied ids among them reach
///    the write only past a `NotFound`-returning read or a
///    `rows_affected() == 0` check. The second shape bottoms out too: every
///    production `INSERT` into an entity table binds a server-minted id —
///    `Uuid::new_v4()`, `format!("team-{uuid}")`,
///    `format!("{workspace_id}::{suffix}")`, or a personal-mode literal — so no
///    primary key it can read is empty either.
///
/// 3. **Neither dialect enforces that**, so (2) is an invariant of this codebase
///    and not of the schema — which is exactly why it is recorded here rather
///    than left to a constraint to state. Measured by replaying both migration
///    chains: the column is `NOT NULL` on both and NULL is rejected on both,
///    `''` is accepted on both, and the two disagree only on width — Postgres
///    rejects anything past `VARCHAR(100)` where SQLite's `TEXT` takes any
///    length. Adding a `CHECK (entity_id <> '')` is schema work, and TRA-10005
///    opened no migration front for it: TRA-9999, the ticket that would have
///    carried it, had already shipped (`744f9e0`) by the time this audit ran.
///
/// A guard would also not be free. `drain_delta` aborting yields no watermark,
/// so the client's cursor never advances past the offending `sync_id` and every
/// reconnect replays the same page into the same abort — one row would deny a
/// whole workspace its delta stream until retention pruned it. That trade is
/// right in `stream_entities`, where the trigger is a coding error affecting an
/// entire entity type for every workspace; it is not right here, where the only
/// trigger left is a single stored row with an empty primary key.
///
/// And such a row would not cost the client its data, which is the last reason
/// it is not worth aborting over. `cache/apply.rs`'s `apply_action_to_memory`
/// reads `entity_id` only on `Delete` — every insert/update arm keys the
/// reactive store off the payload or bumps a version counter — and
/// `cache/sync_engine.rs`'s `hydrate_store_from_db` rebuilds entities from the
/// stored JSON while discarding the IndexedDB key. Every column these ids are
/// read from is a primary key, so at most one row per table can hold `''` and
/// the cache row it lands under cannot collide with another entity's.
pub async fn handle_sync_delta(
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

/// Serialize a batch of entities for streaming, pairing each with its id.
///
/// The id is taken from [`SyncEntity::entity_id`] — the model's own field —
/// and never from the value `serde_json::to_value` just produced. That is the
/// single hop where the old design read the id back out of the encoded payload
/// under a per-call-site string key, and it is the hop this whole refactor
/// exists to delete.
///
/// An entity that cannot be serialized cannot be put on the wire at all, and
/// the first one that fails takes the whole bootstrap with it: dropping it
/// instead would hand the client a batch quietly missing a row, and then a
/// `SyncComplete` certifying that batch as the complete dataset. The client has
/// no way to learn otherwise — the row is below its new watermark, so no delta
/// will ever mention it again.
///
/// The `Err` is the encoding failure itself; the entity type and the underlying
/// error are logged here, where the concrete `T` is still known.
fn to_addressed_entities<T: SyncEntity>(
    items: &[T],
) -> Result<Vec<AddressedEntity>, serde_json::Error> {
    items
        .iter()
        .map(|item| {
            serde_json::to_value(item)
                .map(|payload| AddressedEntity {
                    entity_id: item.entity_id().to_owned(),
                    payload,
                })
                .inspect_err(|e| {
                    tracing::error!(
                        entity = std::any::type_name::<T>(),
                        error = %e,
                        "Failed to serialize entity for bootstrap, aborting the stream"
                    );
                })
        })
        .collect()
}

/// How a batch stopped streaming.
///
/// Two of these variants end the bootstrap without a `SyncComplete`, but for
/// opposite reasons, and collapsing them into one `bool` is what let the
/// serious one be logged as the routine one. A client that hangs up mid-stream
/// is ordinary; an entity the server cannot address is a defect in this server.
#[derive(Debug, PartialEq, Eq)]
enum StreamOutcome {
    /// Every item in the batch reached the connection.
    Delivered,
    /// The receiving connection is gone. The remaining items were not sent.
    ClientGone,
    /// An entity's id was empty, so it could not be addressed. The remaining
    /// items were not sent.
    UnusableEntity,
}

/// A bounded, user-data-free description of an entity's payload, for the
/// timestamp `warn!` in [`stream_entities`].
///
/// Reports the entity's *key names* — never any value. Those keys are serde
/// field names from a fixed struct in `trakkt-types`, so they are schema, not
/// user records: naming them is what makes a missing or renamed timestamp field
/// diagnosable from an aggregated log, while a dump of the payload would put
/// issue titles and comment bodies into the log at `warn!` level.
///
/// A value that is not a JSON object has no keys at all, so it reports its JSON
/// kind instead — also a fixed, finite string.
fn payload_shape(item: &serde_json::Value) -> String {
    match item.as_object() {
        Some(fields) => fields
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(","),
        None => format!("<not a JSON object: {}>", json_kind(item)),
    }
}

/// The JSON kind of `value`, as one of six fixed words.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Stream a batch of entities as individual `SyncAction(Insert)` messages.
///
/// Used by `handle_sync_bootstrap` to avoid copy-pasting the same loop for each
/// entity type. Each item arrives already addressed: [`to_addressed_entities`]
/// took its `entity_id` from the model's [`SyncEntity`] impl before encoding it.
///
/// # The id is mandatory
///
/// `entity_id` is how the client addresses the row: `cache/apply.rs` keys its
/// IndexedDB upsert on it, so an entity streamed without one is either cached
/// under `""` or not applied at all. Either way the client still counts the
/// frame as received, and the trailing watermark then certifies a dataset that
/// entity is missing from — no later `sync_delta` mentions it again, because it
/// sits below the cursor. So an entity with an empty id returns
/// [`StreamOutcome::UnusableEntity`] and the caller must not send
/// `SyncComplete`.
///
/// TRA-9960 added this guard against four ways an id could be unusable —
/// absent, `null`, non-string, or empty — because the id was then looked up in
/// the encoded payload under a per-call-site string key, and a key that named
/// no field on the model produced any of the first three. TRA-10004 deleted
/// that key: the id is now a `String` read off the model itself, so absent,
/// `null` and non-string are no longer states this function can be handed. The
/// guard stays for the fourth, which the type system cannot exclude and which
/// is not a coding error at all — it is a row stored with an empty primary key,
/// and it would still be certified by a watermark and never re-sent.
///
/// # The timestamp is not
///
/// `timestamp` is deliberately *not* held to the same standard, because the
/// client does not use it to address, order, or reconcile anything.
/// `cache/apply.rs` reads `SyncAction.timestamp` in exactly two places (its
/// `IdbOp::Upsert` for the entity and the paired one for `issue_content`) and
/// both only pass it through as `IdbOp::Upsert.ts`. From there
/// `cache/idb_writer.rs`'s `run_writer` hands it to `IdbSink::upsert`, and
/// `cache/db.rs::upsert` stores it on `EntityRecord.updated_at` — a field
/// outside the record key, which `entity_key` builds from entity type,
/// workspace and entity id alone. Every read discards it:
/// `sync_engine.rs::hydrate_store_from_db` destructures `read_all`'s rows as
/// `(id, json, _ts)`, and the one caller of `read_one`
/// (`pages/issues/issue_detail.rs`) binds it to `_`. Nothing compares it,
/// sorts by it, or uses it to resolve conflicts — the timestamps the UI renders
/// come from the entity payload's own `updated_at`, which is untouched here.
///
/// So an empty `timestamp` is inert, while aborting on one would withhold the
/// whole workspace from the client over a field with no reader. It is logged at
/// `warn!` instead, because it is still anomalous: every entity type the
/// bootstrap streams carries `created_at`, `updated_at`, or both.
///
/// TRA-10004 asked whether `timestamp` should move onto [`SyncEntity`] beside
/// `entity_id`, since it has the same stringly-typed shape. It should not, for
/// three reasons that do not apply to the id:
///
/// 1. There is no per-call-site literal to remove. `"updated_at"` and
///    `"created_at"` are written once, here, for all eleven entity types — so
///    there is no typo that can affect one entity type and not the others, and
///    nothing to keep in step with eleven models. The id literals were the
///    defect class precisely because there were eleven of them.
/// 2. A wrong answer is inert. The paragraphs above trace every reader of this
///    field to a discard; a wrong `entity_id` corrupts the client's cache under
///    a watermark that seals it.
/// 3. A `sync_timestamp()` on the trait would have to *re-render* the value,
///    not borrow it: `Comment` stores `chrono::DateTime<Utc>`, so the method
///    would format it by hand while serde formats the payload's copy its own
///    way. That trades a divergence nothing reads for a new one between two
///    renderings of the same instant.
async fn stream_entities(
    conn_tx: &WsSender,
    workspace_id: &str,
    entity_type: &str,
    items: Vec<AddressedEntity>,
) -> StreamOutcome {
    use trakkt_types::sync::{SyncAction, SyncActionType, SyncResponse};

    for (index, entity) in items.into_iter().enumerate() {
        let AddressedEntity { entity_id, payload } = entity;

        if entity_id.is_empty() {
            tracing::error!(
                workspace_id,
                entity_type,
                index,
                "sync_bootstrap: entity's stored id is empty -- aborting the stream"
            );
            return StreamOutcome::UnusableEntity;
        }

        let timestamp = match payload
            .get("updated_at")
            .or_else(|| payload.get("created_at"))
            .and_then(|v| v.as_str())
        {
            Some(ts) => ts.to_string(),
            None => {
                tracing::warn!(
                    workspace_id,
                    entity_type,
                    entity_id,
                    entity_keys = %payload_shape(&payload),
                    "sync_bootstrap: entity has neither updated_at nor created_at as a \
                     string -- streaming it with an empty timestamp, which no client \
                     reader consumes"
                );
                String::new()
            }
        };

        let action = SyncAction {
            sync_id: 0,
            entity_type: entity_type.to_string(),
            entity_id,
            workspace_id: workspace_id.to_string(),
            action: SyncActionType::Insert,
            data: Some(payload),
            timestamp,
        };
        if !send_sync_response(conn_tx, SyncResponse::SyncAction(action)).await {
            return StreamOutcome::ClientGone;
        }
    }

    StreamOutcome::Delivered
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

    /// A minimal stand-in for a streamed entity.
    ///
    /// The real models carry thirty-odd fields and a DB round-trip to build;
    /// the streaming path reads exactly two things off one — the id its
    /// [`SyncEntity`] impl returns, and a timestamp key on the encoded payload
    /// — so three fields exercise it at full fidelity.
    ///
    /// This replaces the `serde_json::json!` literal these tests used to stream.
    /// The literal could not be used any more and should not be: a bare `Value`
    /// has no `SyncEntity` impl, so it cannot say what it is or what its id is,
    /// which is the guarantee under test. Going through a typed stand-in means
    /// the tests exercise the real derivation — `PendingBatch::new` reading
    /// `T::ENTITY_TYPE`, `to_addressed_entities` reading `entity_id()` — rather
    /// than a shape that mimics its output.
    #[derive(serde::Serialize)]
    struct TestIssue {
        issue_id: String,
        title: &'static str,
        updated_at: &'static str,
    }

    impl SyncEntity for TestIssue {
        const ENTITY_TYPE: &'static str = entity_types::ISSUE;

        fn entity_id(&self) -> &str {
            &self.issue_id
        }
    }

    /// A test issue with the given stored id.
    fn issue(issue_id: &str) -> TestIssue {
        TestIssue {
            issue_id: issue_id.to_owned(),
            title: "streamed issue",
            updated_at: "2026-07-26T12:00:00Z",
        }
    }

    /// Address a batch the way `PendingBatch` does, for the tests that drive
    /// [`stream_entities`] directly rather than through a batch.
    ///
    /// `TestIssue` is three owned fields, so the encode cannot fail and this
    /// `expect` cannot be what a failing test is reporting.
    fn addressed(items: &[TestIssue]) -> Vec<AddressedEntity> {
        to_addressed_entities(items).expect("encoding well-formed test issues")
    }

    fn parse_frame(frame: &str) -> SyncResponse {
        serde_json::from_str(frame).expect("frame deserializes as a SyncResponse")
    }

    #[tokio::test]
    async fn stream_entities_writes_one_frame_per_item_in_order() {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);
        let items = [issue("iss_1"), issue("iss_2"), issue("iss_3")];

        assert_eq!(
            stream_entities(&conn_tx, "ws_1", entity_types::ISSUE, addressed(&items)).await,
            StreamOutcome::Delivered,
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
        let items: Vec<TestIssue> = (0..50).map(|i| issue(&format!("iss_{i}"))).collect();
        let item_count = items.len();
        let items = addressed(&items);

        let streamer = tokio::spawn(async move {
            stream_entities(&conn_tx, "ws_1", entity_types::ISSUE, items).await
        });

        // Receiving the first frame proves the stream is under way; closing then
        // fails every subsequent send.
        let first = conn_rx.recv().await.expect("first frame");
        match parse_frame(&first) {
            SyncResponse::SyncAction(action) => assert_eq!(action.entity_id, "iss_0"),
            other => panic!("expected SyncAction, got {other:?}"),
        }
        conn_rx.close();

        // `ClientGone`, specifically — not merely "not Delivered". A dead
        // connection is routine and must stay distinguishable from
        // `UnusableEntity`, which is a server defect and is logged at `error!`.
        assert_eq!(
            streamer.await.expect("stream task completes"),
            StreamOutcome::ClientGone,
            "a dead connection must be reported to the caller as ClientGone"
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

    /// An entity that refuses to be encoded.
    ///
    /// `serde_json::to_value` only fails on values JSON cannot express, and no
    /// entity model this handler streams can produce one, so the failure has to
    /// be introduced deliberately. This stands in for an *entity*, not for any
    /// production code: everything it is handed to below — `PendingBatch`,
    /// `to_addressed_entities`, `stream_bootstrap` — is exactly what
    /// `handle_sync_bootstrap` runs.
    struct Unencodable;

    impl serde::Serialize for Unencodable {
        fn serialize<S: serde::Serializer>(
            &self,
            _serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("this entity cannot be encoded"))
        }
    }

    /// The id here is never reached: `to_addressed_entities` encodes first and
    /// only asks for the id of an entity that encoded, so the failing batch
    /// below fails on the `Serialize` impl above rather than on addressing.
    impl SyncEntity for Unencodable {
        const ENTITY_TYPE: &'static str = entity_types::LABEL;

        fn entity_id(&self) -> &str {
            "lbl_1"
        }
    }

    /// The entity ids carried by the `SyncAction` frames in `frames`.
    fn streamed_entity_ids(frames: &[SyncResponse]) -> Vec<&str> {
        frames
            .iter()
            .filter_map(|f| match f {
                SyncResponse::SyncAction(action) => Some(action.entity_id.as_str()),
                _ => None,
            })
            .collect()
    }

    /// A batch that cannot be encoded ends the stream where it stands.
    ///
    /// The batches ahead of it have already reached the client, so what is at
    /// stake is not the missing frames — it is the watermark. Sending it would
    /// have the client file a dataset it knows is short as the complete
    /// workspace, and every future delta starts above the rows it never got.
    #[tokio::test]
    async fn a_batch_that_cannot_be_encoded_ends_the_bootstrap_without_a_watermark() {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);
        let issues = [issue("iss_1"), issue("iss_2")];
        let unencodable = [Unencodable];

        stream_bootstrap(
            &conn_tx,
            "usr_1",
            "ws_1",
            99,
            vec![PendingBatch::new(&issues), PendingBatch::new(&unencodable)],
        )
        .await;
        drop(conn_tx);

        let mut frames = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            frames.push(parse_frame(&frame));
        }

        // Non-vacuity: the stream ran, and got as far as the batch in front of
        // the one that could not be encoded.
        assert_eq!(
            streamed_entity_ids(&frames),
            vec!["iss_1", "iss_2"],
            "the batches ahead of the failure should have streamed, got {frames:?}"
        );
        assert!(
            !frames
                .iter()
                .any(|f| matches!(f, SyncResponse::SyncComplete { .. })),
            "a bootstrap that could not encode a batch must not certify what it did \
             send as complete, got {frames:?}"
        );
    }

    /// The same stream with nothing broken in it: every batch encodes, so the
    /// watermark is sent. Without this, the assertion above is satisfied by a
    /// `stream_bootstrap` that never sends a watermark at all.
    #[tokio::test]
    async fn a_fully_encodable_stream_ends_with_the_watermark() {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);
        let issues = [issue("iss_1"), issue("iss_2")];

        stream_bootstrap(&conn_tx, "usr_1", "ws_1", 99, vec![PendingBatch::new(&issues)]).await;
        drop(conn_tx);

        let mut frames = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            frames.push(parse_frame(&frame));
        }

        assert_eq!(streamed_entity_ids(&frames), vec!["iss_1", "iss_2"]);
        assert!(
            matches!(
                frames.last(),
                Some(SyncResponse::SyncComplete { last_sync_id: 99 })
            ),
            "an unbroken stream closes with the watermark it was given, got {:?}",
            frames.last()
        );
        assert_eq!(
            frames
                .iter()
                .filter(|f| matches!(f, SyncResponse::SyncComplete { .. }))
                .count(),
            1,
            "the watermark is sent once and only once, got {frames:?}"
        );
    }

    /// Stream one issue batch to a live receiver, reporting the outcome and the
    /// entity ids that actually reached the wire.
    ///
    /// The receiver stays alive for the whole send, so `ClientGone` is not
    /// reachable from here — any abort this reports is an addressing abort.
    /// Channel capacity comfortably exceeds every batch below, so no send can
    /// park and no second task is needed to drain concurrently.
    async fn stream_issue_batch(items: &[TestIssue]) -> (StreamOutcome, Vec<String>) {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);
        let outcome =
            stream_entities(&conn_tx, "ws_1", entity_types::ISSUE, addressed(items)).await;
        drop(conn_tx);

        let mut streamed = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            match parse_frame(&frame) {
                SyncResponse::SyncAction(action) => streamed.push(action.entity_id),
                other => panic!("expected SyncAction, got {other:?}"),
            }
        }

        (outcome, streamed)
    }

    /// An entity whose stored id is empty stops the batch where it stands.
    ///
    /// # Why this used to be four cases
    ///
    /// TRA-9960 ran this as a table over four shapes — the id key absent,
    /// `null`, non-string, and empty — because `stream_entities` looked the id
    /// up in the encoded payload under a string named at the call site, and a
    /// string that named no field on the model produced the first three. All
    /// four collapsed to the same empty `entity_id` before that guard existed.
    ///
    /// TRA-10004 deleted the lookup. The id is now a `String` read off the
    /// model by `SyncEntity::entity_id`, so `stream_entities` cannot be handed
    /// an absent, `null` or non-string id — there is no code that could
    /// construct one, and none that could be written. Those three shapes were
    /// not dropped from this test because they stopped mattering; they were
    /// dropped because writing them no longer compiles, which is the guarantee
    /// TRA-10004 exists to provide and is the strongest form this assertion
    /// could take.
    ///
    /// The fourth is a different thing and survives: an empty id is not a
    /// coding error but a stored row whose primary key is `""`. No type
    /// excludes it, and it is still fatal for the same reason — the client keys
    /// its IndexedDB upsert on `entity_id`, so the row lands under `""` or is
    /// not applied, and the watermark that follows puts it below the floor of
    /// every future delta.
    ///
    /// The abort is mid-stream, not a rollback — the frame in front of the bad
    /// entity has already been delivered and stays delivered. What the guard
    /// buys is the watermark that never follows it.
    #[tokio::test]
    async fn an_entity_with_an_empty_id_aborts_the_batch() {
        let (outcome, streamed) =
            stream_issue_batch(&[issue("iss_1"), issue(""), issue("iss_3")]).await;

        // `UnusableEntity` and not merely "not Delivered": an unaddressable
        // entity must stay distinguishable from the client hanging up, which is
        // routine. `["iss_1"]` and not `["iss_1", "iss_3"]`: the batch stops
        // where it stands, and the frame already on the wire is not retracted.
        assert_eq!(
            (outcome, streamed),
            (StreamOutcome::UnusableEntity, vec!["iss_1".to_owned()]),
            "an empty stored id must abort the batch at the offending entity"
        );
    }

    /// The same batch with nothing wrong in it. Without this, the assertion
    /// above is satisfied by a `stream_entities` that aborts on every batch.
    #[tokio::test]
    async fn a_batch_of_addressable_entities_streams_to_the_end() {
        let (outcome, streamed) = stream_issue_batch(&[issue("iss_1"), issue("iss_3")]).await;

        assert_eq!(
            (outcome, streamed),
            (
                StreamOutcome::Delivered,
                vec!["iss_1".to_owned(), "iss_3".to_owned()]
            ),
            "a batch whose entities all carry ids streams in full"
        );
    }

    /// A batch holding an unaddressable entity ends the bootstrap without a
    /// watermark, exactly as an unencodable batch does.
    ///
    /// Non-vacuity has two halves. The batches ahead of the failure are
    /// asserted to have streamed, so the run reached the failing batch rather
    /// than dying earlier. And the failure can only be the addressing guard:
    /// `TestIssue` is three owned fields, so `to_addressed_entities` cannot
    /// fail to encode it and `stream_bootstrap`'s encode guard is unreachable
    /// here.
    ///
    /// The second batch is another `TestIssue` batch rather than a differently
    /// typed one because what is under test is the batch *boundary* — that the
    /// abort happens after an earlier batch has gone out — and not anything
    /// about which entity type it holds.
    #[tokio::test]
    async fn an_entity_that_cannot_be_addressed_ends_the_bootstrap_without_a_watermark() {
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(16);
        let issues = [issue("iss_1"), issue("iss_2")];
        let unaddressable = [issue("")];

        stream_bootstrap(
            &conn_tx,
            "usr_1",
            "ws_1",
            99,
            vec![
                PendingBatch::new(&issues),
                PendingBatch::new(&unaddressable),
            ],
        )
        .await;
        drop(conn_tx);

        let mut frames = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            frames.push(parse_frame(&frame));
        }

        assert_eq!(
            streamed_entity_ids(&frames),
            vec!["iss_1", "iss_2"],
            "the batches ahead of the unaddressable entity should have streamed, got \
             {frames:?}"
        );
        assert!(
            !frames
                .iter()
                .any(|f| matches!(f, SyncResponse::SyncComplete { .. })),
            "a bootstrap that could not address an entity must not certify what it did \
             send as complete, got {frames:?}"
        );
    }

    /// A `BootstrapData` with every list empty, and the settings row the caller
    /// asks for.
    ///
    /// The batch list is built from the *fields*, not their contents, so empty
    /// lists exercise it exactly as populated ones would — and keep the two
    /// tests below free of a database.
    fn empty_bootstrap_data(
        workspace_settings: Option<WorkspaceSettingsSnapshot>,
    ) -> BootstrapData {
        BootstrapData {
            issues: Vec::new(),
            labels: Vec::new(),
            statuses: Vec::new(),
            teams: Vec::new(),
            projects: Vec::new(),
            views: Vec::new(),
            favorites: Vec::new(),
            notifications: Vec::new(),
            comments: Vec::new(),
            milestones: Vec::new(),
            workspace_settings,
        }
    }

    fn settings_snapshot() -> WorkspaceSettingsSnapshot {
        WorkspaceSettingsSnapshot {
            workspace_id: "ws_1".to_owned(),
            name: Some("Test workspace".to_owned()),
            settings: None,
            default_team_id: None,
            updated_at: "2026-07-26T12:00:00Z".to_owned(),
        }
    }

    /// The batch list covers each entity type the bootstrap sends, once.
    ///
    /// `PendingBatch::new` now takes only the list, so the entity type is
    /// derived and cannot be wrong for the list it was given. What it cannot
    /// catch is a list named twice while another is not named at all — the two
    /// arguments used to differ, so a duplicated line was visible; now the
    /// lines differ only in a field name. That slip would stream one type
    /// twice, omit another entirely, and still end in a `SyncComplete`
    /// certifying the omission — silent staleness of exactly the kind this
    /// handler is written to avoid.
    ///
    /// The expected list is the bootstrap's own set, which is deliberately
    /// smaller than `entity_types::ALL`: types not named here reach clients
    /// through `sync_delta` rather than through a bootstrap batch.
    #[test]
    fn bootstrap_streams_every_entity_type_exactly_once() {
        let data = empty_bootstrap_data(Some(settings_snapshot()));

        let streamed: Vec<&str> = bootstrap_batches(&data)
            .iter()
            .map(|batch| batch.entity_type)
            .collect();

        assert_eq!(
            streamed,
            vec![
                entity_types::ISSUE,
                entity_types::LABEL,
                entity_types::STATUS,
                entity_types::TEAM,
                entity_types::PROJECT,
                entity_types::VIEW,
                entity_types::FAVORITE,
                entity_types::NOTIFICATION,
                entity_types::COMMENT,
                entity_types::PROJECT_MILESTONE,
                entity_types::WORKSPACE_SETTINGS,
            ],
            "the bootstrap must stream each of its entity types exactly once, in order"
        );
    }

    /// A workspace with no settings row streams the other ten and nothing in
    /// place of the eleventh.
    ///
    /// This is the half of the conditional the test above cannot see: it would
    /// pass just as well against a list that always appends the settings batch,
    /// which for a workspace without a row would put a frame on the wire with
    /// no entity behind it.
    #[test]
    fn a_workspace_with_no_settings_row_contributes_no_batch() {
        let data = empty_bootstrap_data(None);

        let streamed: Vec<&str> = bootstrap_batches(&data)
            .iter()
            .map(|batch| batch.entity_type)
            .collect();

        assert!(
            !streamed.contains(&entity_types::WORKSPACE_SETTINGS),
            "there is no settings row, so there is no settings batch, got {streamed:?}"
        );
        assert_eq!(
            streamed.len(),
            10,
            "the other ten batches are unaffected, got {streamed:?}"
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
                trakkt_auth::sync_log_service::SyncAudience::Workspace,
                SyncActionType::Update,
                None,
            )
            .await
            .expect("write sync entry");
        }

        db
    }

    /// Wait for a spawned task to raise `flag`. Fails the test rather than
    /// hanging if it never does; `what` names the milestone in that failure.
    ///
    /// On the current-thread runtime these tests run on, a task that raises its
    /// flag and then awaits gets there in a single poll, so observing the flag
    /// from here also proves the task is parked on whatever it awaited next.
    async fn wait_until_flagged(flag: &CatchUpFlag, what: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !flag.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
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

        wait_until_flagged(&catching_up, "the bootstrap to flag the connection").await;

        // Make room so the bootstrap can finish.
        assert_eq!(conn_rx.recv().await.as_deref(), Some("occupied"));
        stream.await.expect("bootstrap task");

        assert!(
            !catching_up.load(Ordering::Acquire),
            "the exemption must not outlive the bootstrap"
        );
    }

    /// A bootstrap's watermark and its entity reads are separate queries over no
    /// shared snapshot, so a mutation can always commit between them. Which side
    /// of the watermark read it lands on decides whether it is recoverable:
    /// reading the watermark first leaves the mutation above the cursor, so the
    /// client's first delta re-delivers it; reading it last buries the mutation
    /// under a cursor that never carried it, below the floor of every future
    /// delta.
    ///
    /// The interleave is forced, not raced. The SQLite pool holds exactly one
    /// connection and hands it out fairly, so its waiter queue works as a baton:
    /// parking the bootstrap on it, queueing the mutation behind it, then
    /// releasing makes the mutation commit in the gap that opens after the
    /// bootstrap's very first query. That first query is the watermark read —
    /// and the point of the test is that when it is not, this fails.
    #[tokio::test]
    async fn sync_bootstrap_watermark_stays_below_a_mutation_that_commits_mid_bootstrap() {
        const WORKSPACE: &str = "ws_bootstrap_watermark";
        const USER: &str = "usr_1";

        let db = db_with_sync_entries(WORKSPACE, 3).await;
        let head_before = trakkt_auth::sync_log_service::get_latest_sync_id(&db, WORKSPACE)
            .await
            .expect("latest sync_id");

        let trakkt_core::DbPool::Sqlite(pool) = db.clone() else {
            panic!("db_with_sync_entries builds an in-memory SQLite pool");
        };
        // Holding the pool's only connection parks every query issued from here
        // on in the waiter queue, in the order the tasks arrive.
        let baton = pool.acquire().await.expect("the pool's only connection");

        // Capacity one, pre-filled: the bootstrap cannot deliver even its first
        // frame — let alone return — until this test makes room. That is what
        // makes the assertion below ("still in flight") mean something.
        let (conn_tx, mut conn_rx) = mpsc::channel::<String>(1);
        conn_tx.try_send("occupied".to_string()).expect("prefill");

        let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));
        let bootstrap = tokio::spawn({
            let db = db.clone();
            let flag = Arc::clone(&catching_up);
            async move {
                handle_sync_bootstrap(&conn_tx, &flag, &db, USER, WORKSPACE).await;
            }
        });
        wait_until_flagged(&catching_up, "the bootstrap to reach its first query").await;

        let queued: CatchUpFlag = Arc::new(AtomicBool::new(false));
        let mutation = tokio::spawn({
            let db = db.clone();
            let queued = Arc::clone(&queued);
            async move {
                queued.store(true, Ordering::Release);
                trakkt_auth::sync_log_service::write_sync_entry(
                    &db,
                    entity_types::ISSUE,
                    "iss_mid_bootstrap",
                    WORKSPACE,
                    trakkt_auth::sync_log_service::SyncAudience::Workspace,
                    SyncActionType::Update,
                    None,
                )
                .await
                .expect("write the mid-bootstrap sync entry")
            }
        });
        wait_until_flagged(&queued, "the mutation to queue behind the bootstrap").await;

        // Hand the baton on. The bootstrap runs one query and releases; the
        // mutation, next in line, commits before the bootstrap's second.
        drop(baton);
        let mutation_sync_id = mutation.await.expect("mutation task");

        assert!(
            mutation_sync_id > head_before,
            "the mutation must extend the log past {head_before}, got {mutation_sync_id}"
        );
        assert!(
            !bootstrap.is_finished(),
            "the mutation must commit while the bootstrap is still in flight"
        );

        // Drain, which is also what lets the bootstrap run to completion.
        assert_eq!(conn_rx.recv().await.as_deref(), Some("occupied"));
        let mut frames = Vec::new();
        while let Some(frame) = conn_rx.recv().await {
            frames.push(parse_frame(&frame));
        }
        bootstrap.await.expect("bootstrap task");

        let watermark = match frames.last().expect("a trailing frame") {
            SyncResponse::SyncComplete { last_sync_id } => *last_sync_id,
            other => panic!("expected a trailing SyncComplete, got {other:?}"),
        };
        assert!(
            watermark < mutation_sync_id,
            "watermark {watermark} covers sync_id {mutation_sync_id}, which the bootstrap \
             never streamed -- no delta can reach it again"
        );

        // And the criterion that watermark exists to serve: resuming from it
        // hands the client the mutation it missed.
        let delivered = streamed_sync_ids(&collect_delta_frames(db, WORKSPACE, watermark).await);
        assert!(
            delivered.contains(&mutation_sync_id),
            "the first delta from {watermark} must deliver sync_id {mutation_sync_id}, \
             but streamed {delivered:?}"
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
