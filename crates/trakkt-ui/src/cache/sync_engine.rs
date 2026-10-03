// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client-side sync engine for the offline-first sync protocol.
//!
//! The sync engine manages three sync phases over the shared WebSocket:
//!
//! 1. **Bootstrap** (`sync_bootstrap`): sent on first connect when no local
//!    cursor exists — the server sends the full workspace dataset as a stream
//!    of `sync_action` messages followed by `sync_complete`.
//!
//! 2. **Delta** (`sync_delta`): sent on reconnect when a cursor exists — the
//!    server sends only actions that occurred after `last_sync_id`.
//!
//! 3. **Reset** (`sync_reset`): the server signals that local state is
//!    irrecoverably stale (e.g. cursor too old). The engine nukes IndexedDB,
//!    resets the reactive store, and re-bootstraps.
//!
//! ## Startup ordering
//!
//! Hydration bulk-replaces the store's lists while the sync stream applies
//! individual actions to them, so the two must not overlap: an action applied
//! during hydration is wiped by the `set_*` that lands after it, and the cursor
//! has already moved past it. [`hydrate_then_open_gate`] opens the one latch
//! both live-update paths wait on. See [`crate::cache::hydration_gate`].
//!
//! The two paths wait on it differently, because their transports differ:
//!
//! * [`dial_when_hydrated`] — the leader's WebSocket. Delaying the *dial* costs
//!   nothing: no message has been received when the socket does not exist yet,
//!   so there is no window and nothing to buffer.
//! * [`release_when_hydrated`] — every tab's cross-tab `BroadcastChannel`. That
//!   channel has no replay, so delaying the *subscription* would drop what
//!   other tabs posted during hydration rather than reorder it. The Layout
//!   subscribes at once into a
//!   [`BroadcastQueue`](crate::cache::broadcast_queue::BroadcastQueue) and this
//!   releases it. See [`crate::cache::broadcast_queue`].
//!
//! ## Reconnect handling
//!
//! The engine watches `WebSocketClient::connection_state` and re-sends the
//! appropriate request on every transition to `Connected`. The on_message
//! callback survives reconnects, so there is no need to re-register it.
//!
//! ## Cache durability
//!
//! Every write to the persistent cache — entity upserts, deletes, cache clears,
//! the sync cursor and the schema hash — goes through the engine's single
//! [`idb_writer`] queue. The cursor is a claim that the cache holds everything
//! up to that sync id, so it must never commit ahead of the entity writes it
//! covers; FIFO ordering through one writer task is what guarantees that. See
//! [`crate::cache::idb_writer`] for the full rationale.
//!
//! ## Thread safety / `!Send` types
//!
//! This module is `wasm32`-only. All async tasks use `spawn_local` (single-
//! threaded WASM event loop).

use std::future::Future;

use leptos::prelude::*;
use leptos::task::spawn_local;

use trakkt_types::models::{Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};
use trakkt_types::sync::{SyncAction, SyncResponse, entity_types};

use crate::cache::apply::{apply_action_to_memory, enqueue_cache_writes};
use crate::cache::broadcast_queue::BroadcastQueue;
use crate::cache::cached_types::all_cached_entity_types;
use crate::cache::db;
use crate::cache::hydration_gate::HydrationGate;
use crate::cache::idb_writer::{self, CacheDbSink, IdbOp, IdbWriter};
use crate::cache::store::SyncStore;
use crate::cache::tab_leader::{SyncBroadcast, SyncBroadcastMessage};
use crate::cache::websocket::{ConnectionState, WebSocketClient};

// ── Public entry point ──────────────────────────────────────────────────────

/// Start the sync engine. Call **once** from the Layout after connecting the
/// WebSocket.
///
/// Registers the message callback on the `WebSocketClient` to process
/// `sync_action`, `sync_complete`, and `sync_reset` messages. Watches the
/// connection state signal to send bootstrap or delta requests on every
/// connect/reconnect.
///
/// **Leader tabs only.** This is the only place the shared cache is written, so
/// calling it from a tab that does not hold the leadership lock is exactly the
/// multi-writer race the lock exists to prevent. `broadcast` is the channel the
/// leader republishes applied actions on so follower tabs can update their own
/// in-memory stores; it is `None` when the browser has no `BroadcastChannel`.
///
/// Returns the handle to the cache writer it spawns. Nothing else may open one:
/// the Layout keeps this handle so that this tab's own UI-initiated deletes, and
/// the deletes follower tabs ask for over the broadcast channel, land on the
/// same ordered queue as the sync stream's writes. See
/// [`crate::cache::delete_route`].
///
/// # Ownership
///
/// `owner` is the reactive owner the connection-state watcher below is
/// registered under, and it is a parameter rather than "whatever is current"
/// because the two are not the same thing here.
///
/// This function is called from inside the body of the Layout's startup
/// `Effect`, on the run that promotes this tab to leader. An `Effect` re-run
/// calls [`Owner::with_cleanup`] on its own owner, which disposes every reactive
/// node the previous run created — so a watcher registered under the *current*
/// owner would be torn down by the next re-run of that effect, with nothing left
/// to notice the socket reaching `Connected`. The socket would go on reconnecting
/// on its own backoff while no `sync_bootstrap` or `sync_delta` was ever sent
/// again: a tab that looks connected and has silently stopped syncing.
///
/// Pass an owner that outlives the effect run — the Layout creates one at
/// component setup, so the watcher lives exactly as long as the tab's Layout
/// does. Passing the current owner from inside an effect body reintroduces the
/// defect.
pub fn start_sync_engine(
    owner: &Owner,
    ws: &WebSocketClient,
    store: &SyncStore,
    workspace_id: &str,
    broadcast: Option<SyncBroadcast>,
) -> IdbWriter {
    // ── Spawn the single cache writer ───────────────────────────────────────
    //
    // One task, one database handle, one ordered queue. Nothing else in the
    // engine writes to IndexedDB, so the cursor can never overtake the entity
    // writes it claims to cover.
    let (writer, ops) = idb_writer::channel();
    {
        let wid = workspace_id.to_owned();
        spawn_local(async move {
            let sink = match db::init_cache_db(&wid).await {
                Ok(cache_db) => CacheDbSink::open(cache_db, wid),
                Err(e) => {
                    tracing::warn!(
                        "sync: failed to open cache db — every cache write this session will \
                         report failure and the cursor will not advance: {e}"
                    );
                    CacheDbSink::Unavailable
                }
            };
            idb_writer::run_writer(sink, ops).await;
        });
    }

    // ── Register message handler ────────────────────────────────────────────
    let store_msg = *store;
    let writer_msg = writer.clone();
    let ws_for_msg = ws.clone();
    let broadcast_msg = broadcast.clone();
    ws.set_on_message(move |msg: SyncResponse| {
        match msg {
            SyncResponse::SyncAction(action) => {
                apply_sync_action(&store_msg, &writer_msg, &broadcast_msg, &action);
            }
            SyncResponse::SyncComplete { last_sync_id } => {
                // Queued behind every entity write of this stream, so it only
                // commits once they have.
                writer_msg.enqueue(IdbOp::SetCursor {
                    cursor: last_sync_id.to_string(),
                });
                writer_msg.enqueue(IdbOp::SetSchemaHash);
                broadcast_after_commit(
                    &writer_msg,
                    &broadcast_msg,
                    SyncBroadcastMessage::Complete { last_sync_id },
                );
                store_msg.set_initialized(true);
                tracing::info!(last_sync_id, "sync_complete: store initialized");
            }
            SyncResponse::SyncReset => {
                tracing::info!("sync_reset: nuking local cache and re-bootstrapping");
                store_msg.reset();
                // Derived from the write path rather than listed here, so a type
                // the cache can hold cannot be left out of the wipe that is
                // supposed to leave a clean slate. See
                // [`crate::cache::cached_types`].
                for et in all_cached_entity_types() {
                    writer_msg.enqueue(IdbOp::DeleteAllOfType {
                        entity_type: et.to_owned(),
                    });
                }
                writer_msg.enqueue(IdbOp::SetCursor {
                    cursor: "0".to_owned(),
                });
                // Ordered with everything else on the queue: a marker posted
                // eagerly could overtake action broadcasts still waiting on
                // their writes, and followers would rebuild state the leader
                // had already thrown away.
                broadcast_after_commit(&writer_msg, &broadcast_msg, SyncBroadcastMessage::Reset);

                let writer_reset = writer_msg.clone();
                let ws_for_reset = ws_for_msg.clone();
                spawn_local(async move {
                    // The bootstrap request must not go out until the cache is
                    // actually empty and the cursor rewound.
                    writer_reset.flush().await;
                    if !ws_for_reset.send(serde_json::json!({"type": "sync_bootstrap"})) {
                        tracing::warn!("sync_reset: failed to send bootstrap request");
                    }
                });
            }
        }
    });

    // ── Watch connection state to send bootstrap or delta on connect ────────
    let ws_for_state = ws.clone();
    let wid_state = workspace_id.to_owned();
    let store_state = *store;
    let writer_state = writer.clone();
    let broadcast_state = broadcast;

    // Registered under `owner`, not under whatever owner happens to be current.
    // See this function's "Ownership" section: the caller is an effect body, and
    // an effect disposes everything its previous run created.
    owner.with(|| {
        Effect::new(move |_| {
            let state = ws_for_state.connection_state.get();
            if state != ConnectionState::Connected {
                return;
            }

            let wid = wid_state.clone();
            let ws_send = ws_for_state.clone();
            let writer_connect = writer_state.clone();
            let broadcast_connect = broadcast_state.clone();
            spawn_local(async move {
                let cache_db = match db::init_cache_db(&wid).await {
                    Ok(db) => Some(db),
                    Err(e) => {
                        tracing::warn!("sync: failed to open cache db: {e}");
                        None
                    }
                };

                let idb_cursor = match cache_db {
                    Some(ref db) => match db::get_last_sync_id(db, &wid).await {
                        Ok(Some(s)) => match s.parse::<i64>() {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("sync: failed to parse cursor {s:?}: {e}");
                                0
                            }
                        },
                        Ok(None) => 0,
                        Err(e) => {
                            tracing::warn!("sync: failed to read cursor from IDB: {e}");
                            0
                        }
                    },
                    None => 0,
                };

                if idb_cursor > 0 {
                    tracing::info!(idb_cursor, "sync: cursor found — sending sync_delta");
                    if !ws_send.send(serde_json::json!({
                        "type": "sync_delta",
                        "last_sync_id": idb_cursor
                    })) {
                        tracing::warn!("sync: failed to send sync_delta");
                    }
                } else {
                    store_state.reset();

                    // The same derived wipe as `sync_reset` above, for the same
                    // reason: this path re-bootstraps from nothing too.
                    for et in all_cached_entity_types() {
                        writer_connect.enqueue(IdbOp::DeleteAllOfType {
                            entity_type: et.to_owned(),
                        });
                    }
                    // Same wipe-and-re-bootstrap as sync_reset, so followers must
                    // clear their memory here too or they keep entities this tab
                    // just dropped.
                    broadcast_after_commit(
                        &writer_connect,
                        &broadcast_connect,
                        SyncBroadcastMessage::Reset,
                    );
                    // The clears must complete before the server starts streaming,
                    // or a clear could land on top of freshly bootstrapped rows.
                    writer_connect.flush().await;

                    tracing::info!("sync: no cursor — sending sync_bootstrap");
                    if !ws_send.send(serde_json::json!({"type": "sync_bootstrap"})) {
                        tracing::warn!("sync: failed to send sync_bootstrap");
                    }
                }
            });
        });
    });

    writer
}

// ── Startup ordering ────────────────────────────────────────────────────────

/// Open the leader tab's WebSocket, but not before hydration has finished.
///
/// This is the ordering the sync client depends on. Hydration replaces whole
/// store lists at once; the sync stream mutates individual entries in them. If
/// the socket were already open, a bootstrap or delta action could be applied
/// and then wiped by a hydration `set_*` landing a moment later, with the cursor
/// already advanced past it — the action is gone until the entity changes again
/// or the page is reloaded. Delaying the dial removes the window entirely: there
/// is no socket to deliver anything early, so there is nothing to buffer.
///
/// The token is fetched concurrently with hydration rather than after it: it is
/// a network round trip that touches nothing the store owns, so overlapping it
/// keeps startup at the cost of the slower half instead of the sum. Dialing with
/// a token in hand is also what avoids the connect-with-nothing, get-closed,
/// reconnect-with-a-JWT churn on every multi-user page load.
///
/// Both startup orderings land here correctly: a tab that wins leadership in the
/// same pass that started hydration parks until the gate opens, and a follower
/// promoted long after hydration finished finds it already open.
pub async fn dial_when_hydrated(
    gate: HydrationGate,
    ws: WebSocketClient,
    user_id: String,
    workspace_id: String,
    token: impl Future<Output = String>,
) {
    let ((), token) = futures::future::join(gate.opened(), token).await;
    ws.dial(&user_id, &workspace_id, &token);
}

/// Release the cross-tab message queue, but not before hydration has finished.
///
/// The follower half of the same ordering [`dial_when_hydrated`] gives the
/// leader, and the reason it is a *queue* rather than a second delayed start: a
/// `BroadcastChannel` has no replay, so deferring the subscription would drop
/// everything other tabs posted during hydration instead of merely reordering
/// it. The Layout subscribes immediately and feeds
/// [`BroadcastQueue`](crate::cache::broadcast_queue::BroadcastQueue), which
/// holds what arrives until this releases it.
///
/// Correctness does not depend on *when* that happens. The queue applies
/// messages in arrival order whether they were held or not, so a late release
/// only means a larger backlog — never a reorder. All this has to guarantee is
/// that it happens, which the gate does on every exit from hydration below.
pub async fn release_when_hydrated(gate: HydrationGate, queue: BroadcastQueue) {
    gate.opened().await;
    queue.release();
}

/// Hydrate the store from the local cache, then open `gate`.
///
/// Runs on every tab, leader or not, and starts immediately — cached data still
/// reaches the screen without waiting for the network.
///
/// The gate is what the leader's dial and the follower's message queue both wait
/// on, so it must open on every path out of here — including the one where the
/// cache cannot be opened at all. An empty store is a perfectly valid state to
/// start syncing from; a gate left closed would strand the tab with no socket
/// *and* a queue that accepts broadcast messages forever without ever applying
/// them. [`HydrationGate::open_on_drop`] is what makes that unconditional: the
/// guard below opens the gate when this scope ends, which here means the normal
/// return and the error arm; its own docs cover the exits beyond those two.
pub async fn hydrate_then_open_gate(workspace_id: String, store: SyncStore, gate: HydrationGate) {
    let _open_gate_on_exit = gate.open_on_drop();

    match db::init_cache_db(&workspace_id).await {
        Ok(cache_db) => {
            hydrate_store_from_db(&cache_db, &workspace_id, &store).await;
        }
        Err(e) => {
            tracing::warn!(
                "sync: cache database unavailable — hydrating nothing and starting from an \
                 empty store: {e}"
            );
            // Pages waiting on `initialized` would otherwise sit in their
            // skeleton state forever.
            store.set_initialized(true);
        }
    }
}

// ── Hydration ───────────────────────────────────────────────────────────────

/// Read all entity types from IndexedDB and populate the store.
///
/// Called once at startup before the WebSocket connects, so the UI can
/// render cached data immediately while the sync engine catches up.
pub async fn hydrate_store_from_db(
    cache_db: &db::CacheDb,
    workspace_id: &str,
    store: &SyncStore,
) {
    fn deser<T: serde::de::DeserializeOwned>(
        entries: &[(String, String, String)],
        entity_type: &str,
    ) -> Vec<T> {
        let mut items = Vec::with_capacity(entries.len());
        for (id, json, _ts) in entries {
            match serde_json::from_str(json) {
                Ok(item) => items.push(item),
                Err(e) => tracing::warn!(
                    entity_type,
                    entity_id = %id,
                    "hydration deser failed: {e}"
                ),
            }
        }
        items
    }

    if let Ok(entries) = db::read_all(cache_db, entity_types::ISSUE, workspace_id).await {
        let mut issues = deser::<IssueWithDetails>(&entries, entity_types::ISSUE);
        for issue in &mut issues {
            issue.description = None;
        }
        store.set_issues(issues);
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::LABEL, workspace_id).await {
        store.set_labels(deser::<Label>(&entries, entity_types::LABEL));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::STATUS, workspace_id).await {
        store.set_statuses(deser::<Status>(&entries, entity_types::STATUS));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::TEAM, workspace_id).await {
        store.set_teams(deser::<Team>(&entries, entity_types::TEAM));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::PROJECT, workspace_id).await {
        store.set_projects(deser::<Project>(&entries, entity_types::PROJECT));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::VIEW, workspace_id).await {
        store.set_views(deser::<View>(&entries, entity_types::VIEW));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::FAVORITE, workspace_id).await {
        store.set_favorites(deser::<Favorite>(&entries, entity_types::FAVORITE));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::NOTIFICATION, workspace_id).await {
        store.set_notifications(deser::<Notification>(&entries, entity_types::NOTIFICATION));
    }
    store.set_initialized(true);
    if let Ok(Some(cursor)) = db::get_last_sync_id(cache_db, workspace_id).await {
        tracing::debug!(cursor, "hydrated from IDB with cursor — will delta-sync");
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Publish `message` to the follower tabs once every op queued before it has
/// been processed.
///
/// Ordering is the whole point. Followers apply what they are told without
/// consulting IndexedDB, so telling them about an action before its cache write
/// has committed would let a follower render — and a promoted follower trust —
/// state the shared cache does not hold. Riding the writer queue also means
/// followers see actions in exactly the order the cache took them.
fn broadcast_after_commit(
    writer: &IdbWriter,
    broadcast: &Option<SyncBroadcast>,
    message: SyncBroadcastMessage,
) {
    let Some(channel) = broadcast.clone() else {
        return;
    };
    writer.enqueue(IdbOp::Notify(Box::new(move || channel.post(&message))));
}

/// Apply a single `SyncAction` as the leader tab: queue the cache write, tell
/// the follower tabs once it has committed, and update the reactive store.
///
/// The in-memory store is updated synchronously (the UI must react
/// immediately); persistence is ordered behind every earlier queued op.
fn apply_sync_action(
    store: &SyncStore,
    writer: &IdbWriter,
    broadcast: &Option<SyncBroadcast>,
    action: &SyncAction,
) {
    // Queue the cache writes first so the broadcast that describes this action
    // is ordered behind them rather than racing them.
    enqueue_cache_writes(writer, action);
    broadcast_after_commit(
        writer,
        broadcast,
        SyncBroadcastMessage::Action(action.clone()),
    );
    apply_action_to_memory(store, action);
}

// ── Startup ordering tests (wasm32) ─────────────────────────────────────────

/// Tests of the startup ordering against real IndexedDB and a real
/// `WebSocket`, run in a browser.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::task::{Context, Poll};

    use gloo_timers::future::TimeoutFuture;
    use trakkt_types::sync::SyncActionType;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::apply::apply_broadcast;
    use crate::cache::tab_leader::CachedEntity;
    use crate::cache::websocket;
    use crate::wasm_test_support::boot_leptos_executor;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Number of cached labels each hydration test seeds. Large enough that
    /// hydration takes several turns of the event loop, which is the window the
    /// gate exists to close.
    const SEEDED_LABELS: usize = 200;

    fn label(i: usize, workspace_id: &str) -> Label {
        Label {
            label_id: format!("label-{i}"),
            workspace_id: workspace_id.to_owned(),
            team_id: None,
            name: format!("label {i}"),
            color: "#0D9488".to_owned(),
            created_at: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    /// Seed `SEEDED_LABELS` cached labels so hydration has real work to do.
    async fn seed_labels(workspace_id: &str) {
        let cache_db = match db::init_cache_db(workspace_id).await {
            Ok(cache_db) => cache_db,
            Err(e) => panic!("failed to open cache db: {e}"),
        };
        for i in 0..SEEDED_LABELS {
            let item = label(i, workspace_id);
            let json = match serde_json::to_string(&item) {
                Ok(json) => json,
                Err(e) => panic!("failed to serialise seed label: {e}"),
            };
            if let Err(e) = db::upsert(
                &cache_db,
                entity_types::LABEL,
                &item.label_id,
                workspace_id,
                &json,
                &item.created_at,
            )
            .await
            {
                panic!("failed to seed label {i}: {e}");
            }
        }
    }

    /// Hand control back to the microtask queue. Wakers scheduled by the gate
    /// run as microtasks, so this observes the dial at the earliest instant it
    /// could possibly happen — before any network I/O could move the connection
    /// state on from `Connecting`.
    async fn microtask() {
        let resolved = js_sys::Promise::resolve(&wasm_bindgen::JsValue::NULL);
        if let Err(e) = wasm_bindgen_futures::JsFuture::from(resolved).await {
            panic!("microtask checkpoint failed: {e:?}");
        }
    }

    /// The property the whole ticket rests on: whatever waits on the gate does
    /// not resume until hydration has actually landed in the store.
    #[wasm_bindgen_test]
    async fn the_gate_opens_only_once_the_store_is_hydrated() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-gate-hydration";
        seed_labels(wid).await;

        let store = SyncStore::new();
        let gate = HydrationGate::new();

        assert!(
            !gate.is_open(),
            "the gate must start closed, before any hydration has run"
        );

        // What the leader's dial sees at the moment the gate releases it.
        let waiter = {
            let gate = gate.clone();
            async move {
                gate.opened().await;
                (
                    store.labels().get_untracked().len(),
                    store.initialized().get_untracked(),
                )
            }
        };

        let ((), (labels_at_release, initialized_at_release)) = futures::future::join(
            hydrate_then_open_gate(wid.to_owned(), store, gate.clone()),
            waiter,
        )
        .await;

        assert_eq!(
            labels_at_release, SEEDED_LABELS,
            "the gate released the dial while the store was still being hydrated — \
             a sync action applied now would be wiped by hydration's bulk set_*"
        );
        assert!(initialized_at_release);
        assert!(gate.is_open());
    }

    /// Hydration-then-promotion: a follower that hydrated long ago and only
    /// later takes the leadership lock must not park on an edge it already
    /// missed.
    #[wasm_bindgen_test]
    async fn a_tab_promoted_after_hydration_is_released_immediately() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-gate-promotion";
        seed_labels(wid).await;

        let store = SyncStore::new();
        let gate = HydrationGate::new();

        hydrate_then_open_gate(wid.to_owned(), store, gate.clone()).await;

        assert!(gate.is_open());

        // Resolves on its first poll — no second turn of the event loop, and no
        // edge to miss.
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut waiter = Box::pin(gate.opened());
        assert_eq!(
            waiter.as_mut().poll(&mut cx),
            Poll::Ready(()),
            "a promoted follower parked on a gate that had already opened"
        );
        assert_eq!(store.labels().get_untracked().len(), SEEDED_LABELS);
    }

    /// The dial half, against a real `WebSocketClient`: no socket is opened
    /// while hydration is outstanding, and one is opened as soon as it is not.
    #[wasm_bindgen_test]
    async fn the_socket_is_not_dialed_until_the_gate_opens() {
        let owner = Owner::new();
        owner.set();

        let gate = HydrationGate::new();
        let ws = websocket::disconnected();
        assert_eq!(
            ws.connection_state.get_untracked(),
            websocket::ConnectionState::Disconnected
        );

        // The test harness does not boot the Leptos runtime, so its global
        // executor is unset. `leptos::task::spawn_local` on wasm *is*
        // `wasm_bindgen_futures::spawn_local` (see `any_spawner`'s
        // `init_wasm_bindgen`), so scheduling here matches production exactly.
        wasm_bindgen_futures::spawn_local(dial_when_hydrated(
            gate.clone(),
            ws.clone(),
            "user-gate-test".to_owned(),
            "ws-gate-dial".to_owned(),
            // Already resolved: even the fastest possible token fetch must not
            // be enough to let the dial past the gate.
            std::future::ready(String::new()),
        ));

        // Far longer than the dial task needs to run if it were going to.
        TimeoutFuture::new(50).await;
        assert_eq!(
            ws.connection_state.get_untracked(),
            websocket::ConnectionState::Disconnected,
            "the socket was dialed while hydration was still outstanding"
        );

        gate.open();
        microtask().await;
        microtask().await;

        assert_eq!(
            ws.connection_state.get_untracked(),
            websocket::ConnectionState::Connecting,
            "the socket must be dialed as soon as hydration completes"
        );

        // Stop the backoff loop this test's doomed connection would otherwise
        // leave running.
        websocket::disconnect(&ws);
    }

    // ── The cross-tab queue is released by the same gate (TRA-9944) ─────────
    //
    // The follower half of the ordering above. These drive the production
    // wiring end to end: a real `BroadcastChannel`, the real
    // `hydrate_then_open_gate`, and the real `apply_broadcast` the Layout
    // installs — with `release_when_hydrated` as the only thing between them.

    /// Number of turns of the event loop that reliably covers a
    /// `BroadcastChannel` delivery, which is a task rather than a duration.
    async fn settle() {
        for _ in 0..20 {
            TimeoutFuture::new(1).await;
        }
    }

    /// A cross-tab message carrying a label the seeded cache does not contain,
    /// so hydration's bulk `set_labels` would visibly wipe it.
    fn live_label_action(workspace_id: &str) -> SyncBroadcastMessage {
        label_action(workspace_id, LIVE_LABEL_INDEX)
    }

    /// The same, for callers that need more than one distinguishable label.
    fn label_action(workspace_id: &str, index: usize) -> SyncBroadcastMessage {
        let item = label(index, workspace_id);
        let data = match serde_json::to_value(&item) {
            Ok(data) => data,
            Err(e) => panic!("failed to encode the live label: {e}"),
        };
        SyncBroadcastMessage::Action(SyncAction {
            sync_id: 9_999,
            entity_type: entity_types::LABEL.to_owned(),
            entity_id: item.label_id,
            workspace_id: workspace_id.to_owned(),
            action: SyncActionType::Insert,
            data: Some(data),
            timestamp: "2026-07-27T00:00:00Z".to_owned(),
        })
    }

    /// Deliberately outside the `0..SEEDED_LABELS` range the fixtures seed, so
    /// the live label cannot be confused with a hydrated one.
    const LIVE_LABEL_INDEX: usize = 9_999;

    fn live_label_id() -> String {
        format!("label-{LIVE_LABEL_INDEX}")
    }

    /// A pair of channels on one workspace: the tab under test subscribes to
    /// `subscriber`, another tab posts on `poster`.
    fn channel_pair(workspace_id: &str) -> (SyncBroadcast, SyncBroadcast) {
        let subscriber = match SyncBroadcast::open(workspace_id) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the subscriber's channel: {e:?}"),
        };
        let poster = match SyncBroadcast::open(workspace_id) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the poster's channel: {e:?}"),
        };
        (subscriber, poster)
    }

    /// The ticket's headline criterion: an action broadcast while this tab is
    /// still hydrating is applied *after* hydration — not lost, and not wiped by
    /// hydration's bulk `set_labels`.
    ///
    /// Pre-fix, the handler called `apply_broadcast` the moment the message
    /// arrived, so the label landed in the store and the `set_labels` a few
    /// turns later replaced the whole list without it. Nothing re-delivers it:
    /// the leader posted it after its own cache write committed and its cursor
    /// moved past.
    #[wasm_bindgen_test]
    async fn an_action_broadcast_during_hydration_is_applied_after_it() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-queue-hydration";
        seed_labels(wid).await;

        let store = SyncStore::new();
        let gate = HydrationGate::new();
        let (subscriber, poster) = channel_pair(wid);

        // Exactly the Layout's follower wiring, in the Layout's order: the
        // queue and its release are both in place before anything can arrive,
        // so a release that does not actually wait on the gate shows up here
        // rather than being hidden by the test's own sequencing.
        let queue = BroadcastQueue::new(move |message| apply_broadcast(&store, None, message));
        let handler_queue = queue.clone();
        subscriber.set_on_message(move |message| handler_queue.deliver(message));
        wasm_bindgen_futures::spawn_local(release_when_hydrated(gate.clone(), queue.clone()));

        poster.post(&live_label_action(wid));
        settle().await;

        assert_eq!(
            queue.pending(),
            1,
            "the action was applied before hydration had even started — it is about to \
             be wiped by hydration's bulk set_*, and nothing re-delivers it"
        );
        assert!(
            store.labels().get_untracked().is_empty(),
            "an action applied now is about to be wiped by hydration's bulk set_*"
        );

        hydrate_then_open_gate(wid.to_owned(), store, gate).await;
        settle().await;

        let labels = store.labels().get_untracked();
        assert_eq!(
            labels.len(),
            SEEDED_LABELS + 1,
            "the broadcast action was applied before hydration replaced the list, so \
             hydration wiped it — and nothing ever re-delivers it"
        );
        assert!(
            labels.iter().any(|l| l.label_id == live_label_id()),
            "the held action reached the store as itself, not merely as a count"
        );
        assert_eq!(queue.pending(), 0, "nothing may be left held after release");
    }

    /// TRA-9933 made the **leader** service follower tabs' cache deletes through
    /// this same handler, so holding messages now also defers a leader's
    /// servicing of another tab's delete until its own hydration completes.
    ///
    /// That is the intended trade — deferred, never dropped — and this is what
    /// holds it to "never dropped": the row is still cached while the queue is
    /// held, and gone once it is released.
    #[wasm_bindgen_test]
    async fn a_held_cache_delete_still_reaches_the_leaders_writer() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-queue-hydration-delete";
        const TEAM_ID: &str = "team-held-delete";
        seed_labels(wid).await;

        let team_json = serde_json::json!({
            "team_id": TEAM_ID,
            "workspace_id": wid,
            "name": "Engineering",
            "key": "ENG",
        })
        .to_string();
        let cache_db = match db::init_cache_db(wid).await {
            Ok(cache_db) => cache_db,
            Err(e) => panic!("failed to open cache db: {e}"),
        };
        if let Err(e) = db::upsert(
            &cache_db,
            entity_types::TEAM,
            TEAM_ID,
            wid,
            &team_json,
            "2026-07-27T00:00:00Z",
        )
        .await
        {
            panic!("failed to seed the team row: {e}");
        }
        drop(cache_db);

        async fn team_is_cached(workspace_id: &str, team_id: &str) -> bool {
            let cache_db = match db::init_cache_db(workspace_id).await {
                Ok(cache_db) => cache_db,
                Err(e) => panic!("failed to open cache db: {e}"),
            };
            match db::read_one(&cache_db, entity_types::TEAM, team_id, workspace_id).await {
                Ok(record) => record.is_some(),
                Err(e) => panic!("read_one failed: {e}"),
            }
        }

        assert!(
            team_is_cached(wid, TEAM_ID).await,
            "fixture: the team should be cached before the delete is asked for"
        );

        let store = SyncStore::new();
        let gate = HydrationGate::new();
        let (subscriber, poster) = channel_pair(wid);

        // This tab is the leader: it owns the one cache writer.
        let (writer, ops) = idb_writer::channel();
        let queue = {
            let writer = writer.clone();
            BroadcastQueue::new(move |message| apply_broadcast(&store, Some(&writer), message))
        };
        let handler_queue = queue.clone();
        subscriber.set_on_message(move |message| handler_queue.deliver(message));
        // Wired before anything can arrive, as the Layout wires it — so a
        // release that does not wait on the gate is caught below rather than
        // masked by the test's own sequencing.
        wasm_bindgen_futures::spawn_local(release_when_hydrated(gate.clone(), queue.clone()));

        let driver = async move {
            poster.post(&SyncBroadcastMessage::CacheDelete {
                entities: vec![CachedEntity::new(entity_types::TEAM, TEAM_ID)],
            });
            settle().await;

            assert_eq!(
                queue.pending(),
                1,
                "the delete request was serviced before hydration finished — unordered \
                 against the actions ahead of it on the channel"
            );
            writer.flush().await;
            assert!(
                team_is_cached(wid, TEAM_ID).await,
                "a delete serviced while the queue is held would be unordered against \
                 the actions ahead of it on the channel"
            );

            hydrate_then_open_gate(wid.to_owned(), store, gate).await;
            settle().await;

            assert_eq!(
                queue.pending(),
                0,
                "the delete request was still held after hydration finished"
            );
            writer.flush().await;
            let still_cached = team_is_cached(wid, TEAM_ID).await;

            // Release the handler — and the writer handle the queue holds —
            // so the writer loop can finish.
            drop(subscriber);
            drop(queue);
            drop(writer);
            still_cached
        };

        let sink = CacheDbSink::open(
            match db::init_cache_db(wid).await {
                Ok(cache_db) => cache_db,
                Err(e) => panic!("failed to open cache db: {e}"),
            },
            wid.to_owned(),
        );
        let ((), still_cached) =
            futures::future::join(idb_writer::run_writer(sink, ops), driver).await;

        assert!(
            !still_cached,
            "holding the delete turned into dropping it — the shared cache still holds \
             a team another tab's user deleted, and leaving a team emits no server-side \
             delete that would ever remove it"
        );
    }

    /// The empty-cache path: hydration that reads nothing back must still
    /// release the queue and leave the store usable.
    ///
    /// This is as close as a browser test can get to the IndexedDB-failure arm
    /// of [`hydrate_then_open_gate`]. That arm cannot be forced from here: the
    /// only deterministic way to make `Database::open` fail is a version
    /// downgrade, which is irreversible for the page and would break every
    /// other test sharing the `trakkt-sync` database. It is covered instead by
    /// [`HydrationGate::open_on_drop`], which opens the gate on *every* exit
    /// from that function — the error arm included — and is unit tested
    /// natively in `cache::hydration_gate`.
    #[wasm_bindgen_test]
    async fn hydration_with_nothing_cached_still_releases_the_queue() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-queue-empty-cache";
        let store = SyncStore::new();
        let gate = HydrationGate::new();
        let (subscriber, poster) = channel_pair(wid);

        let queue = BroadcastQueue::new(move |message| apply_broadcast(&store, None, message));
        let handler_queue = queue.clone();
        subscriber.set_on_message(move |message| handler_queue.deliver(message));
        wasm_bindgen_futures::spawn_local(release_when_hydrated(gate.clone(), queue.clone()));

        poster.post(&live_label_action(wid));
        settle().await;
        assert_eq!(
            queue.pending(),
            1,
            "the action was applied before hydration had even started"
        );

        hydrate_then_open_gate(wid.to_owned(), store, gate).await;
        settle().await;

        assert_eq!(
            queue.pending(),
            0,
            "a hydration with nothing to read left the queue held forever — every \
             later message would accumulate behind it unbounded"
        );
        assert_eq!(
            store
                .labels()
                .get_untracked()
                .iter()
                .map(|l| l.label_id.clone())
                .collect::<Vec<_>>(),
            vec![live_label_id()],
            "an empty cache is a valid starting point: the held action is still the \
             only thing that should be in the list"
        );
        assert!(
            store.initialized().get_untracked(),
            "pages waiting on `initialized` would sit in their skeleton state forever"
        );
    }

    // ── The connection-state watcher's owner (TRA-9945) ─────────────────────
    //
    // `start_sync_engine` is called from inside the body of the Layout's startup
    // effect, and it creates an `Effect` of its own. A re-run of an effect calls
    // `Owner::with_cleanup` on its own owner, which disposes every reactive node
    // the previous run created — so which owner that inner effect belongs to
    // decides whether the tab keeps syncing after the outer effect re-runs.
    //
    // This is the first test in the repo to drive a real `Effect`; see
    // `boot_leptos_executor` for what that cost.

    /// Poll `condition` every turn of the event loop until it holds, or fail.
    ///
    /// The work being waited on is real IndexedDB I/O, whose duration is not
    /// something a test can name. A fixed delay long enough to be reliable would
    /// be far longer than the wait usually needs; this costs the actual latency
    /// and still fails — loudly, and saying what it was waiting for — if the
    /// thing never happens.
    async fn wait_until(what: &str, mut condition: impl AsyncFnMut() -> bool) {
        for _ in 0..500 {
            if condition().await {
                return;
            }
            TimeoutFuture::new(10).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// How many labels the shared cache holds for `workspace_id`.
    async fn cached_label_count(workspace_id: &str) -> usize {
        let cache_db = match db::init_cache_db(workspace_id).await {
            Ok(cache_db) => cache_db,
            Err(e) => panic!("failed to open cache db: {e}"),
        };
        match db::read_all(&cache_db, entity_types::LABEL, workspace_id).await {
            Ok(entries) => entries.len(),
            Err(e) => panic!("failed to read cached labels: {e}"),
        }
    }

    /// The ticket's criterion: force a re-run of the effect that started the
    /// engine, then bring the socket up, and the client must still ask the
    /// server for its data.
    ///
    /// What is asserted is the cursor-less connect path — `store.reset()`
    /// followed by a wipe of every cached entity type — which is the whole of
    /// the work the watcher does immediately before `sync_bootstrap` goes out,
    /// on the same straight line, in the same task. The `send` itself is the one
    /// step this cannot reach: it needs a real open socket to a real sync
    /// server, and a headless browser test has neither. So the assertion is on
    /// the observable half of that sequence rather than on the wire.
    ///
    /// Pre-fix this fails on both counts: the re-run disposes the watcher, so
    /// `Connected` is observed by nobody and neither the store nor the cache is
    /// ever touched.
    #[wasm_bindgen_test]
    async fn the_connection_state_watcher_survives_a_re_run_of_the_effect_that_started_it() {
        boot_leptos_executor();

        let component = Owner::new();
        component.set();

        let wid = "ws-owner-outer-rerun";
        seed_labels(wid).await;
        assert_eq!(
            cached_label_count(wid).await,
            SEEDED_LABELS,
            "fixture: the cache must hold rows for the bootstrap wipe to remove"
        );

        let store = SyncStore::new();
        // Where hydration leaves the store, so the reset the watcher performs is
        // a visible change rather than a value that was already there.
        store.set_initialized(true);

        let ws = websocket::disconnected();

        // Created at component setup, as the Layout creates it — outside the
        // effect whose re-run is the hazard.
        let engine_owner = Owner::new();

        // The Layout's startup effect in miniature. It tracks a signal that
        // bumps after startup (leadership, in production), and its leader half
        // is guarded so the engine starts exactly once — which is why the
        // watcher is never re-registered to paper over its own disposal.
        let promoted = RwSignal::new(false);
        // Counts runs of the outer effect. Without it a test in which the
        // re-run never happened would pass whatever the ownership was, which is
        // the one way this test could quietly stop testing anything.
        let outer_runs = Rc::new(Cell::new(0_u32));
        // Holds the writer handle open: the writer task ends when the last
        // handle drops, and the wipe below has to reach it.
        let writer: Rc<RefCell<Option<IdbWriter>>> = Rc::new(RefCell::new(None));

        {
            let ws = ws.clone();
            let writer = Rc::clone(&writer);
            let outer_runs = Rc::clone(&outer_runs);
            let wid = wid.to_owned();
            Effect::new(move |_| {
                promoted.get();
                outer_runs.set(outer_runs.get() + 1);
                // The leader half's guard: the engine starts once, and is never
                // re-registered to paper over its own disposal.
                if writer.borrow().is_some() {
                    return;
                }
                *writer.borrow_mut() =
                    Some(start_sync_engine(&engine_owner, &ws, &store, &wid, None));
            });
        }
        wait_until("the engine to start", async || writer.borrow().is_some()).await;

        // The re-run the ticket names. It is scheduled rather than synchronous,
        // so wait for it to land: what follows must observe a watcher that has
        // already survived a cleanup, not one that outran it.
        promoted.set(true);
        wait_until("the outer effect to re-run", async || outer_runs.get() >= 2).await;

        ws.connection_state.set(ConnectionState::Connected);

        wait_until("the store to be reset for a bootstrap", async || {
            !store.initialized().get_untracked()
        })
        .await;
        wait_until("the cache to be wiped for a bootstrap", async || {
            cached_label_count(wid).await == 0
        })
        .await;
    }

    /// The other half of the same question: that re-run must leave the *rest* of
    /// the effect body alone.
    ///
    /// The cross-tab subscription, its [`BroadcastQueue`] and the
    /// [`release_when_hydrated`] task are set up in the same effect body as the
    /// engine and are not behind the leader check, so a promotion re-run runs a
    /// cleanup straight over them. None of them is a reactive node — they are
    /// plain `Rc`s, a JS closure the channel owns, and an unscoped
    /// `spawn_local` — so the cleanup has nothing of theirs to dispose. This
    /// holds them to that, because "nothing here is arena-allocated" is a
    /// property a later refactor can quietly take away.
    ///
    /// Three things would each show up here as a different failure: a dropped
    /// backlog, a subscription that stops delivering, and a release task that
    /// the cleanup cancelled.
    ///
    /// Re-subscribing is *not* among them, and deliberately so: dropping the
    /// guard below leaves this passing, because
    /// [`SyncBroadcast::set_on_message`] assigns `onmessage` and drops the
    /// previous `Closure` rather than adding a second listener. So a promotion
    /// that re-ran this whole block would be wasteful, not wrong — the leader
    /// check is what makes it not happen, and this is why it is not the only
    /// thing standing between a re-run and duplicate delivery.
    #[wasm_bindgen_test]
    async fn a_re_run_leaves_the_cross_tab_subscription_and_its_backlog_intact() {
        boot_leptos_executor();

        let component = Owner::new();
        component.set();

        let wid = "ws-owner-rerun-follower";
        let store = SyncStore::new();
        let gate = HydrationGate::new();
        let (subscriber, poster) = channel_pair(wid);

        let queue = BroadcastQueue::new(move |message| apply_broadcast(&store, None, message));

        let promoted = RwSignal::new(false);
        let wired = Rc::new(Cell::new(false));
        {
            // Exactly the Layout's follower wiring, in the Layout's order,
            // inside an effect that will re-run — including `leptos::task::
            // spawn_local`, which is the unscoped spawn the Layout uses. A
            // scoped one would be cancelled by the cleanup, and the release
            // assertion at the end is what would catch that.
            let queue = queue.clone();
            let subscriber = subscriber.clone();
            let gate = gate.clone();
            let wired = Rc::clone(&wired);
            Effect::new(move |_| {
                promoted.get();
                if wired.get() {
                    return;
                }
                wired.set(true);
                let handler_queue = queue.clone();
                subscriber.set_on_message(move |message| handler_queue.deliver(message));
                leptos::task::spawn_local(release_when_hydrated(gate.clone(), queue.clone()));
            });
        }
        wait_until("the follower wiring to go up", async || wired.get()).await;

        // Arrives while hydration is still outstanding, so it is held.
        poster.post(&label_action(wid, LIVE_LABEL_INDEX));
        wait_until("the first action to be held", async || queue.pending() == 1).await;

        // The promotion re-run.
        promoted.set(true);
        settle().await;

        assert_eq!(
            queue.pending(),
            1,
            "the re-run threw away messages that had already arrived — nothing \
             re-delivers them, so those entities stay stale until the page reloads"
        );

        const SECOND_LABEL_INDEX: usize = LIVE_LABEL_INDEX + 1;
        poster.post(&label_action(wid, SECOND_LABEL_INDEX));
        settle().await;

        assert_eq!(
            queue.pending(),
            2,
            "the subscription no longer delivers exactly once — the re-run either \
             dropped the listener or left a second one registered alongside it"
        );

        // The release task spawned in the *first* run must still be waiting.
        gate.open();
        settle().await;

        assert_eq!(queue.pending(), 0, "the release task did not survive the re-run");
        assert_eq!(
            store
                .labels()
                .get_untracked()
                .iter()
                .map(|l| l.label_id.clone())
                .collect::<Vec<_>>(),
            vec![
                label(LIVE_LABEL_INDEX, wid).label_id,
                label(SECOND_LABEL_INDEX, wid).label_id,
            ],
            "the held and the live message must come out as one stream, in arrival order"
        );
    }
}
