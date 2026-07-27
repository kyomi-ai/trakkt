// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single FIFO writer for the client-side persistent cache.
//!
//! ## Why this exists
//!
//! The sync protocol streams entity changes and then, on `sync_complete`,
//! records a cursor (`last_sync_id`). The cursor is a **claim about durability**:
//! "everything up to sync id N is in the local cache". The next reconnect asks
//! the server only for actions *after* the cursor, so any entity that is missing
//! locally while the cursor sits ahead of it is lost permanently — delta will
//! never re-send it.
//!
//! IndexedDB gives no ordering guarantee between independent readwrite
//! transactions, and the cursor lives in a *different* object store from the
//! entities. Writing them from independent fire-and-forget tasks therefore lets
//! the cursor commit while entity writes are still pending: a refresh mid-stream
//! leaves a cache with permanent holes.
//!
//! ## The design
//!
//! One writer task per sync engine owns a single database handle and drains an
//! ordered queue of [`IdbOp`]s. The cursor is just another queue item, so FIFO
//! ordering *is* the durability ordering: `SetCursor` cannot run until every op
//! queued before it has completed.
//!
//! A failed entity write **poisons the stream**: the next `SetCursor` is skipped
//! so the cursor stays behind, and the next delta re-fetches the range that was
//! not persisted. That is the designed recovery path — the client is never
//! allowed to claim durability it does not have.
//!
//! ## Testability
//!
//! The queue semantics (ordering, poisoning, flush) are pure Rust and live in
//! [`run_writer`], which is generic over an [`IdbSink`]. The IndexedDB-backed
//! sink ([`CacheDbSink`]) is the only `wasm32`-gated part, so the ordering and
//! poisoning rules are unit-tested on the native target against the real loop.

use std::fmt;
use std::future::Future;

use futures::StreamExt;
use futures::channel::{mpsc, oneshot};

// ── Operations ──────────────────────────────────────────────────────────────

/// A single persistence operation processed by the writer task, in order.
///
/// Ops carry no workspace id: the sink is constructed for one workspace and
/// scopes every key itself.
pub enum IdbOp {
    /// Write (insert or replace) one entity record.
    Upsert {
        entity_type: String,
        entity_id: String,
        json: String,
        ts: String,
    },
    /// Delete one entity record.
    Delete {
        entity_type: String,
        entity_id: String,
    },
    /// Delete every record of one entity type for this workspace.
    DeleteAllOfType { entity_type: String },
    /// Record the sync cursor. Skipped when earlier entity writes failed.
    SetCursor { cursor: String },
    /// Record the schema hash the cache was written with.
    SetSchemaHash,
    /// Resolve the paired receiver once every earlier op has been processed.
    Flush(oneshot::Sender<()>),
    /// Run a callback once every earlier op has been processed.
    ///
    /// The synchronous sibling of [`IdbOp::Flush`]: same ordering guarantee,
    /// but without a task to park on a oneshot. The sync engine publishes each
    /// applied action to the other tabs from here, so a follower can never be
    /// told about data the shared cache does not hold yet — and so broadcasts
    /// reach followers in the same order the cache took them.
    Notify(Box<dyn FnOnce()>),
}

// ── Sink ────────────────────────────────────────────────────────────────────

/// A single write against the persistence backend failed.
///
/// Carries a rendered message because the concrete backend error type is
/// `wasm32`-only while the writer loop is not.
#[derive(Debug)]
pub struct SinkError(pub String);

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The persistence backend the writer loop drives.
///
/// Implemented by [`CacheDbSink`] over IndexedDB on `wasm32`, and by recording
/// sinks in the native unit tests.
///
/// Methods are declared as `-> impl Future` rather than `async fn` so the trait
/// carries no implicit `Send` promise: the IndexedDB handles behind it are
/// `!Send` and the writer task always runs on the single-threaded WASM loop.
pub trait IdbSink {
    fn upsert(
        &self,
        entity_type: &str,
        entity_id: &str,
        json: &str,
        ts: &str,
    ) -> impl Future<Output = Result<(), SinkError>>;

    fn delete(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> impl Future<Output = Result<(), SinkError>>;

    fn delete_all_of_type(
        &self,
        entity_type: &str,
    ) -> impl Future<Output = Result<(), SinkError>>;

    fn set_cursor(&self, cursor: &str) -> impl Future<Output = Result<(), SinkError>>;

    fn set_schema_hash(&self) -> impl Future<Output = Result<(), SinkError>>;
}

// ── Handle ──────────────────────────────────────────────────────────────────

/// Handle used by the sync engine to enqueue work on the writer task.
///
/// Cheap to clone; every clone feeds the same ordered queue.
#[derive(Clone)]
pub struct IdbWriter {
    ops: mpsc::UnboundedSender<IdbOp>,
}

impl IdbWriter {
    /// Append an op to the queue.
    ///
    /// The queue is unbounded, so this never blocks and never applies
    /// backpressure to the WebSocket message callback.
    pub fn enqueue(&self, op: IdbOp) {
        if let Err(e) = self.ops.unbounded_send(op) {
            tracing::warn!("idb writer: queue closed, op dropped: {e}");
        }
    }

    /// Wait until every op enqueued so far has been processed.
    ///
    /// Returns immediately if the writer task is gone — callers use this to
    /// sequence a network request after a cache clear, and must not hang when
    /// the cache is unavailable.
    pub async fn flush(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.enqueue(IdbOp::Flush(ack_tx));
        if ack_rx.await.is_err() {
            tracing::warn!("idb writer: flush never acknowledged — writer task is gone");
        }
    }
}

/// Create the writer handle and the receiver its task drains.
pub fn channel() -> (IdbWriter, mpsc::UnboundedReceiver<IdbOp>) {
    let (tx, rx) = mpsc::unbounded();
    (IdbWriter { ops: tx }, rx)
}

// ── Writer loop ─────────────────────────────────────────────────────────────

/// Drain `ops` in FIFO order against `sink` until every [`IdbWriter`] handle is
/// dropped.
///
/// Tracks how many entity writes failed since the last cursor decision. A
/// non-zero count means the cache is missing data the cursor would claim, so the
/// next `SetCursor` is skipped and the count reset: the cursor stays behind and
/// the next delta re-fetches the range.
pub async fn run_writer<S: IdbSink>(sink: S, mut ops: mpsc::UnboundedReceiver<IdbOp>) {
    let mut failed_writes: usize = 0;

    while let Some(op) = ops.next().await {
        match op {
            IdbOp::Upsert {
                entity_type,
                entity_id,
                json,
                ts,
            } => {
                if let Err(e) = sink.upsert(&entity_type, &entity_id, &json, &ts).await {
                    failed_writes += 1;
                    tracing::warn!(
                        entity_type = %entity_type,
                        entity_id = %entity_id,
                        "idb writer: upsert failed: {e}"
                    );
                }
            }
            IdbOp::Delete {
                entity_type,
                entity_id,
            } => {
                if let Err(e) = sink.delete(&entity_type, &entity_id).await {
                    failed_writes += 1;
                    tracing::warn!(
                        entity_type = %entity_type,
                        entity_id = %entity_id,
                        "idb writer: delete failed: {e}"
                    );
                }
            }
            IdbOp::DeleteAllOfType { entity_type } => match sink.delete_all_of_type(&entity_type).await {
                // A whole-type wipe supersedes earlier per-entity failures: the
                // rows those writes would have produced are gone by design. The
                // wipe paths (sync_reset, cursor-less bootstrap) rewind the
                // cursor to "0" immediately afterwards, and a rewind can never
                // claim more than the cache holds — so letting an earlier
                // failure suppress that rewind would strand the cursor *ahead*
                // of an emptied cache, which is the exact corruption this
                // writer exists to prevent.
                Ok(()) => failed_writes = 0,
                Err(e) => tracing::warn!(
                    entity_type = %entity_type,
                    "idb writer: delete_all_of_type failed: {e}"
                ),
            },
            IdbOp::SetCursor { cursor } => {
                if failed_writes > 0 {
                    tracing::warn!(
                        "idb writer: skipping cursor persist: {failed_writes} writes failed — next delta will re-sync"
                    );
                    failed_writes = 0;
                    continue;
                }
                if let Err(e) = sink.set_cursor(&cursor).await {
                    // The cursor simply stays where it was, which is safe: it
                    // under-claims rather than over-claims durability.
                    tracing::warn!(cursor = %cursor, "idb writer: cursor persist failed: {e}");
                }
            }
            IdbOp::SetSchemaHash => {
                if let Err(e) = sink.set_schema_hash().await {
                    tracing::warn!("idb writer: schema hash persist failed: {e}");
                }
            }
            IdbOp::Flush(ack) => {
                if ack.send(()).is_err() {
                    tracing::warn!("idb writer: flush waiter dropped before acknowledgement");
                }
            }
            IdbOp::Notify(callback) => callback(),
        }
    }
}

// ── IndexedDB sink (wasm32) ─────────────────────────────────────────────────

/// [`IdbSink`] backed by one long-lived IndexedDB connection.
#[cfg(target_arch = "wasm32")]
pub enum CacheDbSink {
    /// The cache database opened successfully.
    Open {
        db: crate::cache::db::CacheDb,
        workspace_id: String,
    },
    /// The cache database could not be opened. Every write reports failure,
    /// which keeps the cursor from advancing over a cache nothing was written
    /// to, while flushes still resolve so the sync handshake proceeds.
    Unavailable,
}

#[cfg(target_arch = "wasm32")]
impl CacheDbSink {
    pub fn open(db: crate::cache::db::CacheDb, workspace_id: String) -> Self {
        Self::Open { db, workspace_id }
    }

    /// Borrow the open database, or report the sink is unavailable.
    fn parts(&self) -> Result<(&crate::cache::db::CacheDb, &str), SinkError> {
        match self {
            Self::Open { db, workspace_id } => Ok((db, workspace_id.as_str())),
            Self::Unavailable => Err(SinkError("cache database is unavailable".to_owned())),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl IdbSink for CacheDbSink {
    async fn upsert(
        &self,
        entity_type: &str,
        entity_id: &str,
        json: &str,
        ts: &str,
    ) -> Result<(), SinkError> {
        let (db, wid) = self.parts()?;
        crate::cache::db::upsert(db, entity_type, entity_id, wid, json, ts)
            .await
            .map_err(|e| SinkError(e.to_string()))
    }

    async fn delete(&self, entity_type: &str, entity_id: &str) -> Result<(), SinkError> {
        let (db, wid) = self.parts()?;
        crate::cache::db::delete(db, entity_type, entity_id, wid)
            .await
            .map_err(|e| SinkError(e.to_string()))
    }

    async fn delete_all_of_type(&self, entity_type: &str) -> Result<(), SinkError> {
        let (db, wid) = self.parts()?;
        crate::cache::db::delete_all_of_type(db, entity_type, wid)
            .await
            .map_err(|e| SinkError(e.to_string()))
    }

    async fn set_cursor(&self, cursor: &str) -> Result<(), SinkError> {
        let (db, wid) = self.parts()?;
        crate::cache::db::set_last_sync_id(db, wid, cursor)
            .await
            .map_err(|e| SinkError(e.to_string()))
    }

    async fn set_schema_hash(&self) -> Result<(), SinkError> {
        let (db, _) = self.parts()?;
        crate::cache::db::set_meta(
            db,
            crate::cache::db::SCHEMA_HASH_KEY,
            crate::cache::db::SCHEMA_HASH,
        )
        .await
        .map_err(|e| SinkError(e.to_string()))
    }
}

// ── Real IndexedDB tests (wasm32) ───────────────────────────────────────────

/// End-to-end tests of the writer against a real IndexedDB, run in a browser.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::db;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    const ENTITY_TYPE: &str = "issue";

    /// A sink that writes to real IndexedDB but fails on one nominated entity,
    /// so the poisoning rule can be exercised end to end.
    struct FailOnSink {
        inner: CacheDbSink,
        fail_entity_id: String,
    }

    impl IdbSink for FailOnSink {
        async fn upsert(
            &self,
            entity_type: &str,
            entity_id: &str,
            json: &str,
            ts: &str,
        ) -> Result<(), SinkError> {
            if entity_id == self.fail_entity_id {
                return Err(SinkError("injected upsert failure".to_owned()));
            }
            self.inner.upsert(entity_type, entity_id, json, ts).await
        }

        async fn delete(&self, entity_type: &str, entity_id: &str) -> Result<(), SinkError> {
            self.inner.delete(entity_type, entity_id).await
        }

        async fn delete_all_of_type(&self, entity_type: &str) -> Result<(), SinkError> {
            self.inner.delete_all_of_type(entity_type).await
        }

        async fn set_cursor(&self, cursor: &str) -> Result<(), SinkError> {
            self.inner.set_cursor(cursor).await
        }

        async fn set_schema_hash(&self) -> Result<(), SinkError> {
            self.inner.set_schema_hash().await
        }
    }

    async fn open(workspace_id: &str) -> db::CacheDb {
        match db::init_cache_db(workspace_id).await {
            Ok(cache_db) => cache_db,
            Err(e) => panic!("failed to open cache db: {e}"),
        }
    }

    fn upsert(entity_id: &str) -> IdbOp {
        IdbOp::Upsert {
            entity_type: ENTITY_TYPE.to_owned(),
            entity_id: entity_id.to_owned(),
            json: format!(r#"{{"issue_id":"{entity_id}"}}"#),
            ts: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    async fn count_entities(workspace_id: &str) -> usize {
        let cache_db = open(workspace_id).await;
        match db::read_all(&cache_db, ENTITY_TYPE, workspace_id).await {
            Ok(rows) => rows.len(),
            Err(e) => panic!("read_all failed: {e}"),
        }
    }

    async fn cursor(workspace_id: &str) -> Option<String> {
        let cache_db = open(workspace_id).await;
        match db::get_last_sync_id(&cache_db, workspace_id).await {
            Ok(value) => value,
            Err(e) => panic!("get_last_sync_id failed: {e}"),
        }
    }

    #[wasm_bindgen_test]
    async fn hundred_upserts_all_land_before_the_cursor() {
        let wid = "ws-writer-fifo";
        let (writer, rx) = channel();
        for i in 0..100 {
            writer.enqueue(upsert(&format!("issue-{i}")));
        }
        writer.enqueue(IdbOp::SetCursor {
            cursor: "1234".to_owned(),
        });
        drop(writer);

        run_writer(CacheDbSink::open(open(wid).await, wid.to_owned()), rx).await;

        assert_eq!(count_entities(wid).await, 99); // TRA-9932 TEMPORARY: proving CI fails on a broken wasm test
        assert_eq!(cursor(wid).await.as_deref(), Some("1234"));
    }

    #[wasm_bindgen_test]
    async fn a_failed_write_stops_the_cursor_from_being_persisted() {
        let wid = "ws-writer-poison";
        let (writer, rx) = channel();
        for i in 0..10 {
            writer.enqueue(upsert(&format!("issue-{i}")));
        }
        writer.enqueue(upsert("issue-boom"));
        writer.enqueue(IdbOp::SetCursor {
            cursor: "5555".to_owned(),
        });
        drop(writer);

        let sink = FailOnSink {
            inner: CacheDbSink::open(open(wid).await, wid.to_owned()),
            fail_entity_id: "issue-boom".to_owned(),
        };
        run_writer(sink, rx).await;

        assert_eq!(count_entities(wid).await, 10);
        assert_eq!(
            cursor(wid).await,
            None,
            "the cursor must not claim data that never reached IndexedDB"
        );
    }

    #[wasm_bindgen_test]
    async fn reset_flow_empties_the_cache_and_rewinds_the_cursor_before_flush() {
        let wid = "ws-writer-reset";

        // Seed a populated cache with an advanced cursor.
        let (writer, rx) = channel();
        for i in 0..20 {
            writer.enqueue(upsert(&format!("issue-{i}")));
        }
        writer.enqueue(IdbOp::SetCursor {
            cursor: "900".to_owned(),
        });
        drop(writer);
        run_writer(CacheDbSink::open(open(wid).await, wid.to_owned()), rx).await;
        assert_eq!(count_entities(wid).await, 20);

        // The sync_reset sequence: wipe every type, rewind, then flush.
        let (writer, rx) = channel();
        writer.enqueue(IdbOp::DeleteAllOfType {
            entity_type: ENTITY_TYPE.to_owned(),
        });
        writer.enqueue(IdbOp::SetCursor {
            cursor: "0".to_owned(),
        });
        let flusher = writer.clone();
        drop(writer);

        let sink = CacheDbSink::open(open(wid).await, wid.to_owned());
        let driver = async move {
            flusher.flush().await;
            // State the bootstrap request would be sent on.
            (count_entities(wid).await, cursor(wid).await)
        };
        let (_, (entities, cursor_at_flush)) =
            futures::future::join(run_writer(sink, rx), driver).await;

        assert_eq!(entities, 0, "the entity cache must be empty at flush time");
        assert_eq!(cursor_at_flush.as_deref(), Some("0"));
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures::executor::block_on;

    use super::*;

    /// Sink that records every call in order and can be told to fail on
    /// specific entity ids.
    #[derive(Default)]
    struct RecordingSink {
        calls: Rc<RefCell<Vec<String>>>,
        fail_entities: Vec<String>,
        fail_delete_all: bool,
    }

    impl RecordingSink {
        fn failing_on(ids: &[&str]) -> Self {
            Self {
                fail_entities: ids.iter().map(|s| (*s).to_string()).collect(),
                ..Self::default()
            }
        }

        fn record(&self, call: String) {
            self.calls.borrow_mut().push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        /// Share the call log with a notifier so it can read what had already
        /// been written at the instant it fired.
        fn calls_handle(&self) -> Rc<RefCell<Vec<String>>> {
            Rc::clone(&self.calls)
        }
    }

    impl IdbSink for RecordingSink {
        async fn upsert(
            &self,
            entity_type: &str,
            entity_id: &str,
            _json: &str,
            _ts: &str,
        ) -> Result<(), SinkError> {
            self.record(format!("upsert:{entity_type}:{entity_id}"));
            if self.fail_entities.iter().any(|id| id == entity_id) {
                return Err(SinkError("injected upsert failure".to_owned()));
            }
            Ok(())
        }

        async fn delete(&self, entity_type: &str, entity_id: &str) -> Result<(), SinkError> {
            self.record(format!("delete:{entity_type}:{entity_id}"));
            if self.fail_entities.iter().any(|id| id == entity_id) {
                return Err(SinkError("injected delete failure".to_owned()));
            }
            Ok(())
        }

        async fn delete_all_of_type(&self, entity_type: &str) -> Result<(), SinkError> {
            self.record(format!("delete_all:{entity_type}"));
            if self.fail_delete_all {
                return Err(SinkError("injected delete_all failure".to_owned()));
            }
            Ok(())
        }

        async fn set_cursor(&self, cursor: &str) -> Result<(), SinkError> {
            self.record(format!("set_cursor:{cursor}"));
            Ok(())
        }

        async fn set_schema_hash(&self) -> Result<(), SinkError> {
            self.record("set_schema_hash".to_owned());
            Ok(())
        }
    }

    fn upsert(entity_id: &str) -> IdbOp {
        IdbOp::Upsert {
            entity_type: "issue".to_owned(),
            entity_id: entity_id.to_owned(),
            json: "{}".to_owned(),
            ts: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    fn delete(entity_id: &str) -> IdbOp {
        IdbOp::Delete {
            entity_type: "issue".to_owned(),
            entity_id: entity_id.to_owned(),
        }
    }

    fn set_cursor(cursor: &str) -> IdbOp {
        IdbOp::SetCursor {
            cursor: cursor.to_owned(),
        }
    }

    /// Enqueue `ops`, close the queue, and run the writer to completion.
    fn drain(sink: RecordingSink, ops: Vec<IdbOp>) -> RecordingSink {
        let (writer, rx) = channel();
        for op in ops {
            writer.enqueue(op);
        }
        drop(writer);
        block_on(run_writer(&sink, rx));
        sink
    }

    /// Lets the tests keep inspecting a sink after handing it to the writer.
    impl<S: IdbSink> IdbSink for &S {
        async fn upsert(
            &self,
            entity_type: &str,
            entity_id: &str,
            json: &str,
            ts: &str,
        ) -> Result<(), SinkError> {
            (*self).upsert(entity_type, entity_id, json, ts).await
        }

        async fn delete(&self, entity_type: &str, entity_id: &str) -> Result<(), SinkError> {
            (*self).delete(entity_type, entity_id).await
        }

        async fn delete_all_of_type(&self, entity_type: &str) -> Result<(), SinkError> {
            (*self).delete_all_of_type(entity_type).await
        }

        async fn set_cursor(&self, cursor: &str) -> Result<(), SinkError> {
            (*self).set_cursor(cursor).await
        }

        async fn set_schema_hash(&self) -> Result<(), SinkError> {
            (*self).set_schema_hash().await
        }
    }

    #[test]
    fn ops_execute_in_fifo_order() {
        let sink = drain(
            RecordingSink::default(),
            vec![
                upsert("a"),
                upsert("b"),
                delete("c"),
                IdbOp::SetSchemaHash,
                upsert("d"),
                set_cursor("42"),
            ],
        );

        assert_eq!(
            sink.calls(),
            vec![
                "upsert:issue:a",
                "upsert:issue:b",
                "delete:issue:c",
                "set_schema_hash",
                "upsert:issue:d",
                "set_cursor:42",
            ]
        );
    }

    #[test]
    fn clean_run_persists_cursor() {
        let sink = drain(
            RecordingSink::default(),
            (0..100)
                .map(|i| upsert(&format!("issue-{i}")))
                .chain(std::iter::once(set_cursor("7")))
                .collect(),
        );

        let calls = sink.calls();
        assert_eq!(calls.len(), 101, "every op should have reached the sink");
        assert_eq!(calls.last().map(String::as_str), Some("set_cursor:7"));
    }

    #[test]
    fn failed_upsert_skips_the_following_cursor() {
        let sink = drain(
            RecordingSink::failing_on(&["bad"]),
            vec![upsert("good"), upsert("bad"), set_cursor("42")],
        );

        assert_eq!(sink.calls(), vec!["upsert:issue:good", "upsert:issue:bad"]);
    }

    #[test]
    fn failed_delete_skips_the_following_cursor() {
        let sink = drain(
            RecordingSink::failing_on(&["bad"]),
            vec![delete("bad"), set_cursor("42")],
        );

        assert_eq!(sink.calls(), vec!["delete:issue:bad"]);
    }

    #[test]
    fn dirty_flag_resets_after_a_skipped_cursor() {
        let sink = drain(
            RecordingSink::failing_on(&["bad"]),
            vec![
                upsert("bad"),
                set_cursor("42"),
                upsert("good"),
                set_cursor("43"),
            ],
        );

        assert_eq!(
            sink.calls(),
            vec!["upsert:issue:bad", "upsert:issue:good", "set_cursor:43"],
            "the skip must not poison later, clean stretches of the stream"
        );
    }

    #[test]
    fn cache_wipe_clears_earlier_failures_so_the_cursor_can_rewind() {
        let sink = drain(
            RecordingSink::failing_on(&["bad"]),
            vec![
                upsert("bad"),
                IdbOp::DeleteAllOfType {
                    entity_type: "issue".to_owned(),
                },
                set_cursor("0"),
            ],
        );

        assert_eq!(
            sink.calls(),
            vec!["upsert:issue:bad", "delete_all:issue", "set_cursor:0"],
            "a wipe must not leave the cursor stranded ahead of an emptied cache"
        );
    }

    #[test]
    fn failed_cache_wipe_does_not_block_the_cursor_rewind() {
        let sink = drain(
            RecordingSink {
                fail_delete_all: true,
                ..RecordingSink::default()
            },
            vec![
                IdbOp::DeleteAllOfType {
                    entity_type: "issue".to_owned(),
                },
                set_cursor("0"),
            ],
        );

        assert_eq!(sink.calls(), vec!["delete_all:issue", "set_cursor:0"]);
    }

    #[test]
    fn a_wholly_failing_sink_never_advances_the_cursor() {
        let sink = drain(
            RecordingSink::failing_on(&["a", "b"]),
            vec![upsert("a"), upsert("b"), set_cursor("99")],
        );

        assert!(
            !sink.calls().iter().any(|c| c.starts_with("set_cursor")),
            "no cursor may be written when every entity write failed"
        );
    }

    #[test]
    fn flush_resolves_only_after_all_preceding_ops() {
        let sink = RecordingSink::default();
        let (writer, rx) = channel();

        let driver = async {
            writer.enqueue(upsert("a"));
            writer.enqueue(upsert("b"));
            writer.enqueue(upsert("c"));
            writer.flush().await;
            let seen = sink.calls();
            // Drop the last handle so the writer loop terminates.
            drop(writer);
            seen
        };

        let (_, seen) = block_on(futures::future::join(run_writer(&sink, rx), driver));

        assert_eq!(
            seen,
            vec!["upsert:issue:a", "upsert:issue:b", "upsert:issue:c"],
            "flush must not resolve before every op queued ahead of it ran"
        );
    }

    #[test]
    fn notify_runs_only_after_the_ops_queued_before_it() {
        let sink = RecordingSink::default();
        let seen_at_notify: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let (writer, rx) = channel();
        writer.enqueue(upsert("a"));
        writer.enqueue(upsert("b"));
        {
            // The notifier observes the sink at the moment it fires.
            let recorded = Rc::clone(&seen_at_notify);
            let observed = sink.calls_handle();
            writer.enqueue(IdbOp::Notify(Box::new(move || {
                *recorded.borrow_mut() = observed.borrow().clone();
            })));
        }
        writer.enqueue(upsert("c"));
        drop(writer);

        block_on(run_writer(&sink, rx));

        assert_eq!(
            *seen_at_notify.borrow(),
            vec!["upsert:issue:a", "upsert:issue:b"],
            "a broadcast must not go out before the writes it describes have committed"
        );
        assert_eq!(
            sink.calls(),
            vec!["upsert:issue:a", "upsert:issue:b", "upsert:issue:c"],
            "the notifier must not disturb the op stream around it"
        );
    }

    #[test]
    fn notifiers_fire_in_queue_order() {
        let order: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut ops: Vec<IdbOp> = Vec::new();
        for i in 0..5u8 {
            ops.push(upsert(&format!("issue-{i}")));
            let order = Rc::clone(&order);
            ops.push(IdbOp::Notify(Box::new(move || order.borrow_mut().push(i))));
        }

        drain(RecordingSink::default(), ops);

        assert_eq!(
            *order.borrow(),
            vec![0, 1, 2, 3, 4],
            "followers must receive actions in the order the cache took them"
        );
    }

    #[test]
    fn flush_resolves_when_the_writer_task_is_gone() {
        let (writer, rx) = channel();
        drop(rx);
        // Must return rather than hang: callers sequence network requests on it.
        block_on(writer.flush());
    }
}
