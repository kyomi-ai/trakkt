// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a tab sends the cache deletes its own UI initiates.
//!
//! ## Why a route rather than a direct write
//!
//! Every tab of a browser shares one `trakkt-sync` IndexedDB database, and
//! exactly one tab per (browser, workspace) is allowed to write it: the holder
//! of the leadership lock. That tab is also the only one that runs a sync
//! engine, so it is the only one that has an
//! [`IdbWriter`](crate::cache::idb_writer::IdbWriter) at all — a follower has no
//! writer to enqueue on.
//!
//! A delete, though, can be clicked in any tab. Performing it from whichever tab
//! the click landed in is precisely the multi-writer race the lock exists to
//! remove: the write is unordered against the sync stream's writes to the same
//! object store, and unordered against the cursor that claims the cache is
//! complete up to a given sync id.
//!
//! So the delete is *routed* to the one writer instead:
//!
//! * the leader enqueues it directly on the writer it owns, in FIFO order with
//!   every sync-stream write — no round trip through a channel to itself;
//! * a follower posts a [`SyncBroadcastMessage::CacheDelete`], and the leader
//!   enqueues it on that same queue when it arrives.
//!
//! Either way one writer performs every cache write, which is the invariant.
//!
//! ## What is *not* routed
//!
//! The in-memory removal. These are user-initiated actions, so the clicking tab
//! drops the entity from its reactive store synchronously and the UI updates at
//! once — it never waits on another tab, or on the server. Only the durable half
//! travels.
//!
//! ## Why this is not "the server will send a delete anyway"
//!
//! For two of the three flows it would be: deleting a project emits
//! `PROJECT`/`Delete` and deleting a team emits `TEAM`/`Delete`. But *leaving* a
//! team is a membership change — the team still exists, so the server emits
//! `TEAM`/`Update` and no delete is ever sent. The routed delete is the only
//! thing that removes that team from the local cache, so it has to be durable.

use std::rc::Rc;

use crate::cache::idb_writer::{IdbOp, IdbWriter};
use crate::cache::tab_leader::CachedEntity;
#[cfg(target_arch = "wasm32")]
use crate::cache::tab_leader::{SyncBroadcast, SyncBroadcastMessage};

/// Hands a batch of cache deletes to whatever this tab's role can reach.
///
/// A boxed closure rather than an enum because the follower's transport is a
/// browser type that does not exist on the server target, while this module —
/// like the store that holds the route — compiles on both.
type DeleteSink = Rc<dyn Fn(Vec<CachedEntity>)>;

/// Where this tab's UI-initiated cache deletes go.
///
/// Cheap to clone. Held by [`SyncStore`](crate::cache::store::SyncStore) and set
/// by the Layout as the tab's role is established — and set again when a
/// follower is promoted to leader, so a promoted tab stops delegating and starts
/// enqueueing on the writer it now owns.
///
/// The [`Default`] route is *unrouted*: it belongs to a tab that has reached
/// neither role, which happens during SSR and in a browser with neither Web
/// Locks nor `BroadcastChannel`. It logs and drops, because the one thing it
/// must not do is write IndexedDB itself.
#[derive(Clone, Default)]
pub struct DeleteRoute {
    sink: Option<DeleteSink>,
}

impl DeleteRoute {
    /// This tab owns the cache writer: deletes go straight onto its queue.
    ///
    /// Ordered against every other op on that queue, including the sync
    /// stream's writes to the same records and the cursor that covers them.
    pub fn owned(writer: IdbWriter) -> Self {
        Self {
            sink: Some(Rc::new(move |entities: Vec<CachedEntity>| {
                for entity in entities {
                    writer.enqueue(IdbOp::Delete {
                        entity_type: entity.entity_type,
                        entity_id: entity.entity_id,
                    });
                }
            })),
        }
    }

    /// Another tab owns the cache writer: ask it to perform the delete.
    ///
    /// The request rides the same `BroadcastChannel` the leader publishes
    /// applied actions on, so no second transport is needed.
    #[cfg(target_arch = "wasm32")]
    pub fn delegated(broadcast: SyncBroadcast) -> Self {
        Self {
            sink: Some(Rc::new(move |entities: Vec<CachedEntity>| {
                broadcast.post(&SyncBroadcastMessage::CacheDelete { entities });
            })),
        }
    }

    /// Delete `entities` from the shared cache, through whichever writer owns it.
    pub fn delete(&self, entities: Vec<CachedEntity>) {
        match self.sink {
            Some(ref sink) => sink(entities),
            None => tracing::warn!(
                ?entities,
                "cache delete dropped: this tab holds no leadership lock and has no \
                 BroadcastChannel, so it can reach no cache writer"
            ),
        }
    }
}

// ── Real browser tests (wasm32) ─────────────────────────────────────────────

/// Tests of the routed delete against a real IndexedDB and a real
/// `BroadcastChannel`.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use std::cell::RefCell;

    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use trakkt_types::sync::{SyncAction, SyncActionType, entity_types};
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::apply::{apply_action_to_memory, apply_broadcast, enqueue_cache_writes};
    use crate::cache::db;
    use crate::cache::idb_writer::{CacheDbSink, IdbSink, SinkError, channel, run_writer};
    use crate::cache::store::SyncStore;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    const TEAM_ID: &str = "team-1";

    /// A sink that writes to the real IndexedDB and records what it was asked
    /// to do, in the order the writer asked for it.
    struct RecordingSink {
        inner: CacheDbSink,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl RecordingSink {
        fn over(cache_db: db::CacheDb, workspace_id: &str) -> Self {
            Self {
                inner: CacheDbSink::open(cache_db, workspace_id.to_owned()),
                calls: Rc::new(RefCell::new(Vec::new())),
            }
        }

        /// Share the call log so it can be read after the sink is handed away.
        fn log(&self) -> Rc<RefCell<Vec<String>>> {
            Rc::clone(&self.calls)
        }
    }

    impl IdbSink for RecordingSink {
        async fn upsert(
            &self,
            entity_type: &str,
            entity_id: &str,
            json: &str,
            ts: &str,
        ) -> Result<(), SinkError> {
            self.calls
                .borrow_mut()
                .push(format!("upsert:{entity_type}:{entity_id}"));
            self.inner.upsert(entity_type, entity_id, json, ts).await
        }

        async fn delete(&self, entity_type: &str, entity_id: &str) -> Result<(), SinkError> {
            self.calls
                .borrow_mut()
                .push(format!("delete:{entity_type}:{entity_id}"));
            self.inner.delete(entity_type, entity_id).await
        }

        async fn delete_all_of_type(&self, entity_type: &str) -> Result<(), SinkError> {
            self.calls
                .borrow_mut()
                .push(format!("delete_all:{entity_type}"));
            self.inner.delete_all_of_type(entity_type).await
        }

        async fn set_cursor(&self, cursor: &str) -> Result<(), SinkError> {
            self.calls.borrow_mut().push(format!("set_cursor:{cursor}"));
            self.inner.set_cursor(cursor).await
        }

        async fn set_schema_hash(&self) -> Result<(), SinkError> {
            self.calls.borrow_mut().push("set_schema_hash".to_owned());
            self.inner.set_schema_hash().await
        }
    }

    async fn open(workspace_id: &str) -> db::CacheDb {
        match db::init_cache_db(workspace_id).await {
            Ok(cache_db) => cache_db,
            Err(e) => panic!("failed to open cache db: {e}"),
        }
    }

    /// Is the team still in the shared cache?
    async fn team_is_cached(workspace_id: &str) -> bool {
        let cache_db = open(workspace_id).await;
        match db::read_one(&cache_db, entity_types::TEAM, TEAM_ID, workspace_id).await {
            Ok(record) => record.is_some(),
            Err(e) => panic!("read_one failed: {e}"),
        }
    }

    /// A `TEAM` insert as the sync stream would deliver it.
    fn team_sync_action(workspace_id: &str) -> SyncAction {
        SyncAction {
            sync_id: 1,
            entity_type: entity_types::TEAM.to_owned(),
            entity_id: TEAM_ID.to_owned(),
            workspace_id: workspace_id.to_owned(),
            action: SyncActionType::Insert,
            data: Some(serde_json::json!({
                "team_id": TEAM_ID,
                "workspace_id": workspace_id,
                "name": "Engineering",
                "key": "ENG",
            })),
            timestamp: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    /// The ticket's criterion. A delete the user clicked in this tab and a sync
    /// write to the same record in the same object store must be *one* ordered
    /// stream, not two racing ones.
    ///
    /// The assertion that carries it is the call log: it can only read this way
    /// if the delete went onto the writer's queue. A delete that opened its own
    /// database connection — what `SyncStore::delete_from_idb` used to do —
    /// never appears here at all, and the final state of the record becomes a
    /// race between an unordered transaction and the two upserts around it.
    #[wasm_bindgen_test]
    async fn a_ui_delete_and_a_concurrent_sync_write_are_serialized_through_one_writer() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-delete-serialized";
        let store = SyncStore::new();

        let (writer, ops) = channel();
        store.set_delete_route(DeleteRoute::owned(writer.clone()));

        // Interleaved from the two independent sources: the sync stream, and
        // the user clicking delete in this tab.
        apply_action_to_memory(&store, &team_sync_action(wid));
        enqueue_cache_writes(&writer, &team_sync_action(wid));
        store.remove_team(TEAM_ID);
        assert!(
            store.teams().get_untracked().is_empty(),
            "the clicking tab must drop the entity from its own store immediately, \
             before anything drains the queue"
        );
        enqueue_cache_writes(&writer, &team_sync_action(wid));
        // The route holds a writer handle of its own; both have to go before
        // the loop below can see the queue closed and return.
        store.set_delete_route(DeleteRoute::default());
        drop(writer);

        let sink = RecordingSink::over(open(wid).await, wid);
        let calls = sink.log();
        run_writer(sink, ops).await;

        assert_eq!(
            *calls.borrow(),
            vec![
                "upsert:team:team-1",
                "delete:team:team-1",
                "upsert:team:team-1",
            ],
            "the UI delete bypassed the writer queue — its ordering against the sync \
             stream's writes to the same record is then undefined"
        );
        assert!(
            team_is_cached(wid).await,
            "with one queue the last write wins deterministically; the re-upsert is last"
        );
    }

    /// The leave-team case. Leaving a team emits `TEAM`/`Update`, never a
    /// delete, so this write is the only thing that ever evicts the row — it has
    /// to actually reach IndexedDB, not just memory.
    #[wasm_bindgen_test]
    async fn a_leader_tabs_ui_delete_is_durable_in_indexeddb() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-delete-durable";
        let store = SyncStore::new();

        // A cache in the state a tab would find it in: the team is there.
        let (writer, ops) = channel();
        enqueue_cache_writes(&writer, &team_sync_action(wid));
        drop(writer);
        run_writer(CacheDbSink::open(open(wid).await, wid.to_owned()), ops).await;
        assert!(team_is_cached(wid).await, "fixture: the team should be cached");

        // The user leaves the team in this tab, which holds the lock.
        let (writer, ops) = channel();
        store.set_delete_route(DeleteRoute::owned(writer.clone()));
        store.remove_team(TEAM_ID);
        // See above: the route holds a writer handle of its own.
        store.set_delete_route(DeleteRoute::default());
        drop(writer);

        let sink = RecordingSink::over(open(wid).await, wid);
        let calls = sink.log();
        run_writer(sink, ops).await;

        assert_eq!(
            *calls.borrow(),
            vec!["delete:team:team-1"],
            "the delete must be performed by the writer that owns the cache, not by \
             whichever tab took the click"
        );
        assert!(
            !team_is_cached(wid).await,
            "the team would be hydrated back on the next page load"
        );
    }

    /// A follower has no writer of its own. Its delete has to reach the tab that
    /// does, and be performed there.
    #[wasm_bindgen_test]
    async fn a_follower_tabs_delete_reaches_the_leaders_writer() {
        let owner = Owner::new();
        owner.set();

        let wid = "ws-delete-follower";
        let leader_store = SyncStore::new();
        let follower_store = SyncStore::new();

        let leader_channel = match SyncBroadcast::open(wid) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the leader's channel: {e:?}"),
        };
        let follower_channel = match SyncBroadcast::open(wid) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the follower's channel: {e:?}"),
        };

        // The leader: one writer, and the same message handler the Layout
        // installs on every tab.
        let (writer, ops) = channel();
        {
            let handler_writer = writer.clone();
            leader_channel.set_on_message(move |message| {
                apply_broadcast(&leader_store, Some(&handler_writer), &message);
            });
        }
        // The follower: no writer at all, only the channel.
        follower_store.set_delete_route(DeleteRoute::delegated(follower_channel));

        let driver = async move {
            // Seed through the leader's queue, as a real bootstrap would.
            enqueue_cache_writes(&writer, &team_sync_action(wid));
            apply_action_to_memory(&follower_store, &team_sync_action(wid));
            writer.flush().await;
            assert!(team_is_cached(wid).await, "fixture: the team should be cached");

            follower_store.remove_team(TEAM_ID);
            assert!(
                follower_store.teams().get_untracked().is_empty(),
                "the follower's own UI must update without waiting for the leader"
            );

            // `BroadcastChannel` delivery is a task, so it needs turns of the
            // event loop — not a duration.
            for _ in 0..20 {
                TimeoutFuture::new(1).await;
            }
            // Ordered behind whatever the handler enqueued.
            writer.flush().await;
            let still_cached = team_is_cached(wid).await;

            // Release the listener — and the writer handle it holds — so the
            // writer loop can finish.
            drop(leader_channel);
            drop(writer);
            still_cached
        };

        let sink = RecordingSink::over(open(wid).await, wid);
        let calls = sink.log();
        let (_, still_cached) = futures::future::join(run_writer(sink, ops), driver).await;

        assert_eq!(
            *calls.borrow(),
            vec!["upsert:team:team-1", "delete:team:team-1"],
            "the delete has to be performed by the leader's writer. A follower that \
             reached IndexedDB itself would empty the row without ever appearing here — \
             and would be the second writer this design exists to rule out"
        );
        assert!(
            !still_cached,
            "a follower's delete never reached the leader's writer — the shared cache \
             still holds a team the user deleted"
        );
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use futures::channel::mpsc::UnboundedReceiver;

    use crate::cache::idb_writer::channel;

    use super::*;

    /// Every op waiting on the queue, as `"{entity_type}:{entity_id}"` — and a
    /// marker for anything that is not a delete, so a stray op fails the
    /// assertion rather than being filtered out of it.
    fn queued_deletes(mut ops: UnboundedReceiver<IdbOp>) -> Vec<String> {
        let mut seen = Vec::new();
        while let Ok(op) = ops.try_recv() {
            match op {
                IdbOp::Delete {
                    entity_type,
                    entity_id,
                } => seen.push(format!("{entity_type}:{entity_id}")),
                _ => seen.push("not-a-delete".to_owned()),
            }
        }
        seen
    }

    #[test]
    fn the_leaders_route_enqueues_every_record_on_its_writer() {
        let (writer, ops) = channel();
        let route = DeleteRoute::owned(writer.clone());

        route.delete(vec![
            CachedEntity::new("issue", "issue-1"),
            CachedEntity::new("issue_content", "issue-1"),
        ]);
        drop(writer);

        assert_eq!(
            queued_deletes(ops),
            vec!["issue:issue-1", "issue_content:issue-1"],
            "a multi-record delete must reach the queue whole, in order"
        );
    }

    #[test]
    fn an_unrouted_delete_is_dropped_rather_than_written() {
        // A tab that reached neither role has nothing to write with. The point
        // is that it returns — the alternative this ticket removed was opening
        // its own database connection and writing outside the queue.
        DeleteRoute::default().delete(vec![CachedEntity::new("team", "team-1")]);
    }
}
