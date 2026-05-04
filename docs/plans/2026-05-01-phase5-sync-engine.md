# Phase 5: Sync Engine

**Date:** 2026-05-01
**Goal:** Local-first sync — instant UI from IndexedDB, real-time updates via WebSocket.

## Reference

- Kyomi sync engine: `/home/jason/repos/kyomi/crates/kyomi-auth/src/websocket/`, `sync_log_service.rs`
- Kyomi client cache: `/home/jason/repos/kyomi/crates/kyomi-ui/src/cache/`
- Kyomi WS route: `/home/jason/repos/kyomi/apps/server/src/routes/websocket.rs`
- Tane types: `crates/tane-types/src/sync.rs` (SyncRequest, SyncResponse, SyncAction, entity_types)

## Tasks

### Task 1: WebSocket Manager (server-side)

Create a WebSocket connection manager that tracks connected clients and broadcasts messages.

**Files to create:**

1. `crates/tane-auth/src/websocket/mod.rs` — Module declaration, re-exports
2. `crates/tane-auth/src/websocket/manager.rs` — WebSocketManager:
   - Copy from `/home/jason/repos/kyomi/crates/kyomi-auth/src/websocket/manager.rs`
   - `WebSocketManager { inner: Arc<Inner> }` — Clone-able handle
   - `Inner { connections: DashMap<String, HashSet<TrackedConnection>>, next_id: AtomicU64 }`
   - `TrackedConnection { id: u64, sender: mpsc::Sender<String> }`
   - `connect(user_id) -> (u64, mpsc::Receiver<String>)` — registers a new connection, prunes stale ones
   - `disconnect(user_id, connection_id)` — removes a connection
   - `send_to_user(user_id, message)` — delivers to all connections for a user
   - `broadcast_to_workspace(db, workspace_id, message)` — looks up all workspace members, sends to each connected user
   - Channel capacity: 256 messages, max 10 connections per user
   - **Simplification from Kyomi:** No Redis pub/sub for v1. Single-instance local delivery only. The trait/interface should allow adding Redis later but don't implement it now.

3. Update `crates/tane-auth/src/lib.rs` — Add `pub mod websocket;`

**Dependencies:** tane-core (DbPool for member lookup), tokio (mpsc), dashmap, tracing

### Task 2: WebSocket Route Handler (server-side)

Create the WebSocket upgrade endpoint that handles sync protocol messages.

**Files to create:**

1. `apps/server/src/routes/mod.rs` — Module declaration
2. `apps/server/src/routes/ws.rs` — WebSocket handler:
   - Copy pattern from `/home/jason/repos/kyomi/apps/server/src/routes/websocket.rs`
   - Route: `GET /ws/{user_id}` with `?token=JWT` query parameter
   - Auth: validate JWT from query param, verify user_id matches claims.sub
   - Upgrade to WebSocket using `axum::extract::ws::WebSocketUpgrade`
   - Split into sender/receiver tasks:
     - **Outbound:** forward messages from WS manager mpsc receiver, send periodic pings (45s)
     - **Inbound:** parse JSON messages, handle:
       - `sync_bootstrap` → call `sync_log_service::get_bootstrap(db, workspace_id)`, stream SyncAction messages, then send SyncComplete
       - `sync_delta { last_sync_id }` → call `sync_log_service::get_delta(db, workspace_id, last_sync_id)`, stream actions, send SyncComplete
   - On disconnect: call `ws_manager.disconnect(user_id, connection_id)`
   - Custom close codes: 4001 (auth required), 4003 (forbidden), 4029 (too many connections)

**Integration — update existing files:**

3. Update `apps/server/src/state.rs` — Add `ws_manager: WebSocketManager` to AppState
4. Update `apps/server/src/main.rs` — Create WebSocketManager, add to AppState
5. Update `apps/server/src/lib.rs` — Add WS route: `.route("/ws/{user_id}", get(routes::ws::ws_handler))`

### Task 3: Broadcast from Service Layer

Wire the WebSocket manager into the service layer so mutations broadcast to connected clients in real-time.

**Files to modify:**

1. Update `crates/tane-auth/src/issue_service.rs` — After each sync_log append, broadcast the SyncAction to the workspace via ws_manager
2. Update `crates/tane-auth/src/label_service.rs` — Same
3. Update `crates/tane-auth/src/comment_service.rs` — Same
4. Update `crates/tane-auth/src/notification_service.rs` — Same

**Pattern:** Each service function that writes to sync_log should also broadcast:
```rust
// After sync_log_service::append_sync_log(...)
if let Some(ws) = ws_manager {
    let msg = serde_json::json!({
        "type": "sync_action",
        "data": { "sync_id": sync_id, "entity_type": "issue", "entity_id": &issue_id, "workspace_id": workspace_id, "action": "upsert", "data": &issue }
    });
    ws.broadcast_to_workspace(db, workspace_id, &msg.to_string()).await;
}
```

**Challenge:** Service functions currently only take `db: &DbPool`. They need access to the WS manager for broadcasting. Options:
- Pass `ws_manager: Option<&WebSocketManager>` as an additional parameter to write functions
- Use a context struct that bundles db + ws_manager

**Recommendation:** Add `ws_manager: Option<&tane_auth::websocket::WebSocketManager>` parameter to write functions. Read functions don't need it. The Option allows calling without a manager (e.g., in tests).

### Task 4: Client-Side IndexedDB Cache

Create the IndexedDB persistence layer for offline-first data.

**Files to create in `crates/tane-ui/src/cache/`:**

1. `mod.rs` — Module declarations
2. `db.rs` — IndexedDB operations:
   - Copy pattern from `/home/jason/repos/kyomi/crates/kyomi-ui/src/cache/db.rs`
   - Database name: `tane-sync`, version 1
   - Object stores:
     - `entity_cache` — composite key `"{entity_type}\x00{workspace_id}\x00{entity_id}"` → `{ entity_id, data, updated_at }`
     - `sync_cursors` — key `workspace_id` → `last_sync_id`
     - `_meta` — schema hash for cache invalidation
   - Functions:
     - `init_cache_db(workspace_id) -> Result<CacheDb>`
     - `read_all(db, entity_type, workspace_id) -> Vec<(entity_id, json, timestamp)>`
     - `upsert(db, entity_type, entity_id, workspace_id, json_str, timestamp)`
     - `delete(db, entity_type, entity_id, workspace_id)`
     - `get_last_sync_id(db, workspace_id) -> Option<i64>`
     - `set_last_sync_id(db, workspace_id, sync_id)`
     - `force_wipe_and_reload(workspace_id)` — wipe IDB + reload page
   - Schema hash constant for cache versioning: `"tane-2026-05-01-v1"`

### Task 5: Client-Side SyncStore (Reactive Signals)

Create the reactive store that holds in-memory state derived from IndexedDB + live sync updates.

**Files to create:**

1. `crates/tane-ui/src/cache/store.rs` — SyncStore:
   - Copy pattern from `/home/jason/repos/kyomi/crates/kyomi-ui/src/cache/store.rs`
   - `SyncStore` wraps inner signals in `StoredValue<SendWrapper<SyncStoreInner>>`
   - Signals:
     - `issues: ArcRwSignal<Vec<IssueWithDetails>>`
     - `labels: ArcRwSignal<Vec<Label>>`
     - `notifications_count: ArcRwSignal<i64>`
     - `initialized: ArcRwSignal<bool>`
   - Methods:
     - `new()` — create empty store
     - `issues() -> Signal<Vec<IssueWithDetails>>`
     - `labels() -> Signal<Vec<Label>>`
     - `set_issues(Vec<IssueWithDetails>)` — bulk set (bootstrap)
     - `set_labels(Vec<Label>)` — bulk set
     - `upsert_issue(Issue)` — update or insert in the list
     - `remove_issue(issue_id)` — delete from list
     - `upsert_label(Label)` — update or insert
     - `remove_label(label_id)` — delete
     - `initialized() -> Signal<bool>`
   - Provided via `provide_context(store)` in Layout, accessed via `expect_context::<SyncStore>()`

### Task 6: Client-Side Sync Engine + WebSocket Client

Create the WebSocket connection manager and sync engine that ties everything together.

**Files to create:**

1. `crates/tane-ui/src/cache/websocket.rs` — WebSocket client:
   - Copy pattern from Kyomi's websocket_client
   - `WebSocketContext` — holds connection state signal, send function, subscribe system
   - `ConnectionState` enum: Disconnected, Connecting, Connected, Reconnecting
   - Auto-reconnect with exponential backoff (1s, 2s, 4s, 8s, 16s, 30s max, 10 attempts)
   - `build_ws_url(user_id, workspace_id, token)` — derive ws/wss from location.protocol
   - Subscribe/unsubscribe pattern for typed message routing

2. `crates/tane-ui/src/cache/sync_engine.rs` — Sync engine:
   - Copy pattern from `/home/jason/repos/kyomi/crates/kyomi-ui/src/cache/sync_engine.rs`
   - `start_sync_engine(ws: WebSocketContext, store: SyncStore, workspace_id: String)`
   - On Connected: check IDB cursor, send bootstrap or delta request
   - On SyncAction: apply to store + write to IDB
   - On SyncComplete: update cursor in IDB
   - On SyncReset: wipe IDB + re-bootstrap
   - `apply_sync_action(store, action)` — dispatch by entity_type (issue, label, comment, notification)
   - `hydrate_store_from_db(store, workspace_id)` — load IDB into reactive signals on startup

**Integration — update existing files:**

3. Update `crates/tane-ui/src/lib.rs` — Add `pub mod cache;`
4. Update `crates/tane-ui/src/components/layout.rs` — Initialize sync:
   - Get current user (for user_id, workspace_id)
   - Init IDB cache
   - Hydrate store from IDB
   - Connect WebSocket
   - Start sync engine
   - Provide SyncStore via context
5. Update `crates/tane-ui/src/pages/issue_list.rs` — Read from SyncStore instead of server function for the list (fall back to server fn if store not initialized)
6. Update `crates/tane-ui/src/components/sidebar.rs` — Show notification count from SyncStore

## Compilation & Testing

After all tasks:
1. `cargo check --workspace` — zero errors, zero warnings
2. `trunk build` — WASM builds
3. Start server, open browser, sign up
4. Create an issue → verify it appears in the list without page refresh
5. Open a second tab → create issue in tab 1 → verify it appears in tab 2 instantly
6. Refresh page → verify issues load from IDB instantly (no loading skeleton flash)
