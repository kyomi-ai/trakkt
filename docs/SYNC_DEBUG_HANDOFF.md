# Sync Engine Debug Handoff

**Date:** 2026-05-08
**Status:** WebSocket connects but WASM client's `onmessage` closure doesn't fire for bootstrap responses.

## The Problem

The sync engine's real-time update pipeline is broken at the last mile: the server correctly processes `sync_bootstrap` requests and sends responses, but the WASM `web_sys::WebSocket` client's `onmessage` closure never fires for those responses.

**A manual JS `new WebSocket()` from the same browser receives all messages perfectly.** This proves the server is sending correctly — the bug is specifically in how the WASM client's closures are wired up.

## What Works

1. **Server WS handler** — accepts connections, processes `sync_bootstrap`/`sync_delta`, sends `SyncResponse` JSON via `send_to_user_raw` → `deliver` → `deliver_to_local_user`. Confirmed with `RUST_LOG=debug`.
2. **Server protocol format** — `{"type":"sync_action","sync_id":0,"entity_type":"issue",...}` — verified by manual JS test receiving all messages.
3. **WASM WS connect** — `web_sys::WebSocket::new(url)` succeeds, `onopen` closure fires, `ConnectionState::Connected` is set.
4. **Sync engine bootstrap request** — `ws.send({"type":"sync_bootstrap"})` succeeds (server logs confirm receipt and processing).
5. **SyncStore and issue list page** — wired up correctly, reads from store's reactive signals with client-side filtering.

## What's Broken

The `onmessage` `Closure<dyn FnMut(MessageEvent)>` set via `ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()))` **never fires** after the bootstrap response is sent by the server.

Yet `onopen` (same closure pattern) fires correctly. And a raw JS `WebSocket` created in the same browser tab receives all messages.

## Key Files

- `crates/trakkt-ui/src/cache/websocket.rs` — WS client, `do_connect` function (line ~213) sets up closures
- `crates/trakkt-ui/src/cache/sync_engine.rs` — watches connection state, sends bootstrap
- `crates/trakkt-ui/src/components/layout.rs` — wires sync into the Layout (line ~50)
- `apps/server/src/routes/websocket.rs` — server-side WS handler
- `crates/trakkt-auth/src/websocket/manager.rs` — WebSocketManager with `deliver()` method

## Diagnostic Console Logs Already in Place

The WASM code has `web_sys::console::log_1` calls at every step:
- `[trakkt-sync] connect(...)` — fires ✓
- `[trakkt-sync] calling do_connect` — fires ✓
- `[trakkt-sync] WS URL: ws://...` — fires ✓
- `[trakkt-sync] WebSocket::new succeeded` — fires ✓
- `[trakkt-sync] WS OPEN - connected!` — fires ✓ (onopen works)
- `[trakkt-sync] WS MSG: ...` — **NEVER fires** (onmessage broken)
- `[trakkt-sync] WS ERROR: ...` — never fires
- `[trakkt-sync] WS CLOSED ...` — never fires
- `[trakkt-sync] connection state changed: connected` — fires ✓
- `[trakkt-sync] sending sync_bootstrap` — fires ✓

## Hypotheses to Investigate

### 1. Closure lifecycle / GC issue
The onmessage closure is created via `Closure::<dyn FnMut(MessageEvent)>::new(...)`, passed to `ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()))`, then stored as `closures.push(on_message.into_js_value())`. 

`into_js_value()` consumes the `Closure` and returns a `JsValue`. The `JsValue` is stored in `_closures: Vec<JsValue>` on `WsState`. If `WsState` gets dropped or the closures vec gets cleared between `set_onmessage` and the first message arrival, the JS function reference becomes dangling.

**Check:** Add `web_sys::console::log_1` inside `do_connect` AFTER storing `s._closures = closures` to confirm the closures are stored. Then check if `_closures` gets cleared anywhere unexpectedly.

### 2. The outbound task sends messages BEFORE the WS upgrade completes
The server's `handle_authenticated_ws` splits the socket into sender/receiver, spawns a `send_task` (outbound) and `recv_task` (inbound). The `send_task` reads from `manager_rx`. The bootstrap response is sent via `manager.send_to_user_raw()` which pushes to the mpsc channel. If the `send_task` hasn't started reading from the channel yet when the message is pushed, the message sits in the buffer. But mpsc channels are buffered (capacity 256), so this shouldn't lose messages.

**Check:** Add `tracing::debug!("send_task: forwarding message")` in the outbound task's message-receive arm.

### 3. The server sends the response to a different user_id or connection
`send_to_user_raw` calls `deliver_to_local_user(user_id, json)`. The `user_id` for personal mode is `"user-local"`. But the WS connection registered with `ws_manager.connect("user-local")` uses the same ID. So messages should route correctly.

**Check:** Add `tracing::debug!("deliver_to_local_user: user={user_id}, connections={}", conns.value().len())` to confirm the message is being pushed to the right connection.

### 4. Axum WS `split()` + `ws_sender` issue
The server uses `socket.split()` from `futures_util::StreamExt`. The `ws_sender` half forwards messages. But if `send_sync_response` sends via the `WebSocketManager` channel rather than directly through `ws_sender`, the outbound task must relay. If the outbound task's `tokio::select!` loop prioritizes pings over messages, messages might be delayed.

**Check:** The `send_task` uses `tokio::select!` with `manager_rx.recv()` and `ping_interval.tick()`. Both arms are equally prioritized in tokio::select. This should work but verify the task is actually running.

### 5. The WASM client is receiving from a DIFFERENT WebSocket object
If `do_connect` is called twice (e.g., by the reconnect logic), the second call creates a new `WebSocket` and sets `onmessage` on it, but the first one's `onmessage` is nulled. If the server's bootstrap response targets the first connection (which is now dead), the second connection (which is active) won't receive it.

**Check:** Log `ws.ready_state()` after setting onmessage. Log the `connection_id` returned by `ws_manager.connect()` and verify only one connection exists.

## How to Reproduce

```bash
cd /home/jason/repos/trakkt

# Kill any running server
pkill -f "target/dev-server/tane"

# Build frontend
cd crates/trakkt-ui && trunk build && cd ../..

# Start server with debug logging
DATABASE_URL="sqlite:data/trakkt.db?mode=rwc" \
JWT_SECRET_KEY="dev-jwt-secret" \
ENCRYPTION_KEY="dGVzdC1hZXMta2V5LWZvci11bml0LXRlc3RzISEhISE=" \
PORT=3100 TRAKKT_MODE=personal RUST_LOG=debug \
cargo run --package trakkt-server --profile dev-server

# Open http://localhost:3100/issues in browser
# Check browser console for [trakkt-sync] messages
# Check server stderr for "Handling sync_bootstrap" / "sync_bootstrap complete"
```

## Manual JS Test (proves server works)

Open browser console on the same page and run:
```javascript
var ws = new WebSocket('ws://localhost:3100/ws/workspace-local_user-local');
ws.onopen = () => { console.log('OPEN'); ws.send(JSON.stringify({type:'sync_bootstrap'})); };
ws.onmessage = (e) => console.log('MSG:', e.data.substring(0, 150));
```
This receives all sync_action and sync_complete messages correctly.

## Kyomi Reference

Kyomi's working WS client is at `/home/jason/repos/kyomi/crates/kyomi-ui/src/components/chat/websocket_client.rs`. Key difference: Kyomi's `connect()` is an `async fn` called from `spawn_local`, while Trakkt's `do_connect` is synchronous. This might matter for how closure lifetimes interact with the WASM event loop.

## What NOT to Change

- The `WebSocketManager` and `deliver()` abstraction — just refactored, works correctly
- The `SyncResponse` JSON format — matches what the manual test receives
- The issue list page's SyncStore wiring — correct, just needs the store to actually get populated
- The server-side WS handler — processes bootstrap correctly per debug logs
