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
//! individual actions to them, so the two must not overlap: a delta action
//! applied during hydration is wiped by the `set_*` that lands after it, and the
//! cursor has already moved past it. [`hydrate_then_open_gate`] and
//! [`dial_when_hydrated`] are the two halves of that ordering — the socket is
//! not dialed until hydration has finished, so there is no window in which a
//! message could arrive early and nothing to buffer. See
//! [`crate::cache::hydration_gate`].
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
use crate::cache::db;
use crate::cache::hydration_gate::HydrationGate;
use crate::cache::idb_writer::{self, CacheDbSink, IdbOp, IdbWriter};
use crate::cache::store::SyncStore;
use crate::cache::tab_leader::{SyncBroadcast, SyncBroadcastMessage};
use crate::cache::websocket::{ConnectionState, WebSocketClient};

const ALL_CACHED_ENTITY_TYPES: &[&str] = &[
    entity_types::ISSUE,
    entity_types::ISSUE_CONTENT,
    entity_types::LABEL,
    entity_types::STATUS,
    entity_types::TEAM,
    entity_types::PROJECT,
    entity_types::PROJECT_MILESTONE,
    entity_types::VIEW,
    entity_types::FAVORITE,
    entity_types::NOTIFICATION,
    entity_types::COMMENT,
    entity_types::WORKSPACE_SETTINGS,
];

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
pub fn start_sync_engine(
    ws: &WebSocketClient,
    store: &SyncStore,
    workspace_id: &str,
    broadcast: Option<SyncBroadcast>,
) {
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
                for et in ALL_CACHED_ENTITY_TYPES {
                    writer_msg.enqueue(IdbOp::DeleteAllOfType {
                        entity_type: (*et).to_owned(),
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
    let writer_state = writer;
    let broadcast_state = broadcast;

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

                for et in ALL_CACHED_ENTITY_TYPES {
                    writer_connect.enqueue(IdbOp::DeleteAllOfType {
                        entity_type: (*et).to_owned(),
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

/// Hydrate the store from the local cache, then open `gate`.
///
/// Runs on every tab, leader or not, and starts immediately — cached data still
/// reaches the screen without waiting for the network. The gate is what the
/// leader's dial waits on, so it must open on every path out of here: a cache
/// that cannot be opened leaves an empty store, which is a perfectly valid state
/// to start syncing from, while a gate left closed would strand the tab with no
/// socket at all.
pub async fn hydrate_then_open_gate(workspace_id: String, store: SyncStore, gate: HydrationGate) {
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
    gate.open();
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
    use std::task::{Context, Poll};

    use gloo_timers::future::TimeoutFuture;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::websocket;

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
}
