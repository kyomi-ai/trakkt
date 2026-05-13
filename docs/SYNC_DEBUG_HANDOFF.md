# Sync Engine Status

**Date:** 2026-05-08
**Status:** WORKING — bootstrap sync, live mutation broadcasts, and SyncStore all verified end-to-end.

## What Was Fixed

### 1. Bootstrap sync (was already working)
The previous handoff incorrectly reported that the WASM `onmessage` closure "never fires." Adding diagnostic logging proved it fires correctly for every message. The previous debugging sessions lacked a `console.log` at the entry of the `onmessage` closure — all 4 silent early-return paths swallowed messages without any trace, leading to a false diagnosis.

### 2. Live mutation broadcasts (fixed this session)
The `broadcast_sync_notify` function sent a `WebSocketMessage` notification format (`{"type":"sync_action","data":{"entity_type":"issue",...}}`), which doesn't match the `SyncResponse::SyncAction` schema the client expects. The client's `serde_json::from_value::<SyncResponse>(raw)` deserialization silently failed for these notifications.

**Fix:** Created `broadcast_sync_action()` in `sync_log_service.rs` that sends proper `SyncResponse::SyncAction` messages with full entity data. Updated all issue and label service mutations (create, update, delete, set_labels) to fetch the full entity after mutation and broadcast the correct format.

Added `broadcast_raw_to_workspace()` to `WebSocketManager` for sending pre-serialized JSON to all workspace members (the existing `broadcast_to_workspace` only accepts `WebSocketMessage`).

Added `get_issue_by_id()` helper to fetch `IssueWithDetails` by UUID for broadcast data.

### 3. Unused variables cleaned up
Removed dead `ws_send` and `wid` variables in `start_sync_engine`.

## What Works Now

1. **Bootstrap**: First visit loads all issues + labels via WebSocket sync_bootstrap
2. **Live create**: Create issue via MCP → appears in browser instantly (no refresh)
3. **Live update**: Update issue → reflected in browser instantly
4. **Live delete**: Delete issue → removed from browser instantly
5. **Label sync**: Label CRUD broadcasts correct SyncResponse format
6. **SyncStore**: Reactive signals update correctly, issue list reads from store
7. **Server function fallback**: Issue list falls back to server function when store isn't initialized

## What's Not Yet Wired

- **Delta sync**: Client always sends `sync_bootstrap` on reconnect (should check IDB cursor and send `sync_delta` with `last_sync_id` when available)
- **Comment/Team/Notification broadcasts**: Still use old `broadcast_sync_notify` format (client doesn't handle these entity types yet)
- **Multi-tab sync**: Not tested but infrastructure is in place
- **IndexedDB persistence**: Hydration code exists but IDB cursor lookup is deferred

## Key Files

- `crates/trakkt-ui/src/cache/websocket.rs` — WASM WebSocket client
- `crates/trakkt-ui/src/cache/sync_engine.rs` — Client-side sync engine
- `crates/trakkt-ui/src/cache/store.rs` — Reactive SyncStore
- `crates/trakkt-auth/src/sync_log_service.rs` — `broadcast_sync_action()` + `broadcast_sync_notify()`
- `crates/trakkt-auth/src/websocket/manager.rs` — `broadcast_raw_to_workspace()`
- `crates/trakkt-auth/src/issue_service.rs` — `get_issue_by_id()` + updated broadcasts
- `apps/server/src/routes/websocket.rs` — Server WS handler
