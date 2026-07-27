// SPDX-License-Identifier: AGPL-3.0-or-later

//! Holds cross-tab broadcast messages while the store is hydrating, then
//! applies them — all of them, in arrival order — once it is not.
//!
//! ## Why this exists
//!
//! This is the follower half of the race TRA-9921 fixed for the leader.
//! Hydration bulk-replaces every list in
//! [`SyncStore`](crate::cache::store::SyncStore) (`set_issues`, `set_labels`,
//! …). A [`SyncBroadcastMessage`] applied while that is still in flight is wiped
//! by the `set_*` that lands a moment later, and nothing ever re-delivers it:
//! the leader posted it *after* its own cache write committed and its cursor
//! moved past, so no delta will carry it again. The entity stays stale in this
//! tab until it is edited again or the page is reloaded.
//!
//! ## Why the leader's fix does not transfer
//!
//! TRA-9921 fixed the leader by *delaying the dial*. That only reorders — no
//! message has been received when the socket does not exist yet, so there is
//! nothing to buffer and nothing to lose.
//!
//! A `BroadcastChannel` has no replay. Delaying the subscription would not
//! reorder those messages, it would **drop** them: whatever other tabs posted
//! during hydration is gone, permanently, for the same reason as above. That is
//! strictly worse than the bug.
//!
//! So the subscription goes up immediately and the messages are *held* here
//! instead, then applied in one pass when hydration finishes.
//!
//! ## The ordering trap, and why one queue closes it
//!
//! The obvious implementation reorders. Give the handler a fast path — "if
//! hydration has finished, apply straight away, otherwise buffer" — and the
//! window between the flag flipping and the buffer being emptied lets a newly
//! arrived message be applied *ahead* of messages that arrived before it.
//! That is the very symptom this module exists to remove, reintroduced by the
//! fix.
//!
//! There is no fast path here. **Every** message takes the same route: onto the
//! back of one FIFO, and out of the front of it only when the queue is
//! released. Arrival order is preserved because a queue is the only thing that
//! touches it — not because a flag is checked in the right order somewhere.
//!
//! Two further properties fall out of that shape, and both are load-bearing:
//!
//! * [`BroadcastQueue::pump`] is a plain `fn`. A drain that suspended partway
//!   would be a window for a later message to overtake an earlier one, and
//!   `.await` is not expressible in a non-`async` function — so the whole drain
//!   is one synchronous pass by construction, not by comment. WASM is
//!   single-threaded, so nothing can interleave with it.
//! * Re-entrancy is ordered rather than forbidden. If applying a message
//!   somehow delivers another one, the nested call appends and returns; the
//!   outer loop picks it up after the backlog it arrived behind. It cannot jump
//!   the queue, which is what makes the ordering property total instead of
//!   conditional on nothing ever re-entering.
//!
//! ## Why every variant is held, including `CacheDelete`
//!
//! Since TRA-9933 this handler is no longer follower-only: the **leader**
//! services follower tabs' cache deletes through it
//! ([`SyncBroadcastMessage::CacheDelete`]). Holding those too means a leader
//! defers another tab's delete until its own hydration completes.
//!
//! That is the right trade. The delete is not dropped, only ordered after
//! hydration — and ordered, crucially, *behind the actions that preceded it on
//! the channel*. Letting deletes bypass the queue would let one overtake an
//! action it should follow, so a delete-then-reinsert pair could invert and
//! leave the cache holding a row the user removed. One rule for every variant
//! is also the only version of this that cannot rot as variants are added: a
//! new variant is held correctly without anyone having to classify it.
//!
//! ## Why the buffer is bounded
//!
//! It is bounded by [`HydrationGate`](crate::cache::hydration_gate) opening,
//! which [`HydrationGate::open_on_drop`](crate::cache::hydration_gate::HydrationGate::open_on_drop)
//! makes unconditional on every exit from hydration, including the path where
//! IndexedDB cannot be opened at all. The Layout also creates the queue in the
//! same synchronous block that spawns hydration, so there is no arrangement in
//! which a queue exists to fill but no hydration exists to release it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::cache::tab_leader::SyncBroadcastMessage;

/// What the queue does with a message once it is allowed to.
///
/// A boxed closure rather than a concrete call into
/// [`apply_broadcast`](crate::cache::apply::apply_broadcast) because the tab's
/// cache writer is only reachable through the Layout's `StoredValue`, which
/// this module has no business knowing about — and because it lets the tests
/// record the exact sequence that was applied.
type Apply = Box<dyn Fn(&SyncBroadcastMessage)>;

#[derive(Default)]
struct QueueState {
    /// Has hydration finished? Messages are only applied once this is true.
    released: bool,
    /// Is [`BroadcastQueue::pump`] already running further up the stack?
    ///
    /// Set for exactly as long as the drain loop is live, so a re-entrant
    /// delivery appends and returns instead of applying out of order.
    draining: bool,
    /// Messages received but not yet applied, oldest first.
    pending: VecDeque<SyncBroadcastMessage>,
}

struct QueueInner {
    apply: Apply,
    state: RefCell<QueueState>,
}

/// A one-way FIFO between the cross-tab channel and the code that applies what
/// arrives on it.
///
/// Starts *held*: messages accumulate and nothing is applied. [`release`] hands
/// the whole backlog over in arrival order and every later message with it.
///
/// Cheap to clone; every clone is the same queue. Single-threaded by
/// construction — the channel it serves delivers on the browser's one JS task
/// queue.
///
/// [`release`]: BroadcastQueue::release
#[derive(Clone)]
pub struct BroadcastQueue {
    inner: Rc<QueueInner>,
}

impl BroadcastQueue {
    /// Create a held queue that will apply messages with `apply`.
    pub fn new(apply: impl Fn(&SyncBroadcastMessage) + 'static) -> Self {
        Self {
            inner: Rc::new(QueueInner {
                apply: Box::new(apply),
                state: RefCell::new(QueueState::default()),
            }),
        }
    }

    /// Accept one message from the channel.
    ///
    /// Applied immediately if the queue has been released and nothing is ahead
    /// of it, held otherwise. Either way it goes through the same FIFO, so it
    /// can never be applied before a message that arrived earlier.
    pub fn deliver(&self, message: SyncBroadcastMessage) {
        self.inner.state.borrow_mut().pending.push_back(message);
        self.pump();
    }

    /// Hydration has finished: apply the backlog and let later messages
    /// through.
    ///
    /// Drains synchronously — every message held at the moment of the call has
    /// been applied by the time it returns. Idempotent.
    pub fn release(&self) {
        self.inner.state.borrow_mut().released = true;
        self.pump();
    }

    /// Has the queue been released?
    pub fn is_released(&self) -> bool {
        self.inner.state.borrow().released
    }

    /// How many messages are being held.
    ///
    /// Zero once the queue is released, since [`release`] and [`deliver`] both
    /// drain before returning. Non-zero while hydration is outstanding, which
    /// is what tells a test that its fixture actually reached the state it
    /// meant to exercise rather than passing vacuously.
    ///
    /// [`release`]: BroadcastQueue::release
    /// [`deliver`]: BroadcastQueue::deliver
    pub fn pending(&self) -> usize {
        self.inner.state.borrow().pending.len()
    }

    /// Apply queued messages, front first, until the queue is empty.
    ///
    /// Deliberately not `async`: a drain that could suspend partway is a window
    /// for a later message to overtake an earlier one, and a non-`async` `fn`
    /// cannot contain `.await`. The borrow is released around each `apply` call
    /// so the callback is free to re-enter [`deliver`] — the nested call sees
    /// `draining` and appends rather than applying, and this loop picks the
    /// message up in turn.
    fn pump(&self) {
        {
            let mut state = self.inner.state.borrow_mut();
            if !state.released || state.draining {
                return;
            }
            state.draining = true;
        }

        loop {
            let next = {
                let mut state = self.inner.state.borrow_mut();
                match state.pending.pop_front() {
                    Some(message) => message,
                    None => {
                        state.draining = false;
                        return;
                    }
                }
            };
            (self.inner.apply)(&next);
        }
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::RefCell;

    use trakkt_types::sync::{SyncAction, SyncActionType};

    use crate::cache::tab_leader::CachedEntity;

    use super::*;

    /// A one-line identity for a message, so assertions read as a *sequence*
    /// rather than a final state. A final-state assertion is satisfied by any
    /// order, which is precisely what these tests exist to rule out.
    fn describe(message: &SyncBroadcastMessage) -> String {
        match message {
            SyncBroadcastMessage::Action(action) => format!("action:{}", action.entity_id),
            SyncBroadcastMessage::Complete { last_sync_id } => format!("complete:{last_sync_id}"),
            SyncBroadcastMessage::Reset => "reset".to_owned(),
            SyncBroadcastMessage::CacheDelete { entities } => format!(
                "cache_delete:{}",
                entities
                    .iter()
                    .map(|e| e.entity_id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    fn action(entity_id: &str) -> SyncBroadcastMessage {
        SyncBroadcastMessage::Action(SyncAction {
            sync_id: 1,
            entity_type: "issue".to_owned(),
            entity_id: entity_id.to_owned(),
            workspace_id: "ws-1".to_owned(),
            action: SyncActionType::Update,
            data: Some(serde_json::json!({"issue_id": entity_id})),
            timestamp: "2026-07-27T00:00:00Z".to_owned(),
        })
    }

    /// A queue whose applied sequence the test can read back.
    fn recording() -> (BroadcastQueue, Rc<RefCell<Vec<String>>>) {
        let applied = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&applied);
        let queue = BroadcastQueue::new(move |message| log.borrow_mut().push(describe(message)));
        (queue, applied)
    }

    #[test]
    fn nothing_is_applied_while_hydration_is_outstanding() {
        let (queue, applied) = recording();

        queue.deliver(action("issue-1"));
        queue.deliver(action("issue-2"));

        assert!(
            applied.borrow().is_empty(),
            "a message applied now would be wiped by hydration's bulk set_*, and \
             nothing ever re-delivers it"
        );
        assert_eq!(queue.pending(), 2, "both messages must be held, not dropped");
        assert!(!queue.is_released());
    }

    #[test]
    fn the_backlog_is_applied_in_arrival_order_when_the_queue_is_released() {
        let (queue, applied) = recording();

        queue.deliver(action("issue-1"));
        queue.deliver(action("issue-2"));
        queue.release();

        assert_eq!(
            *applied.borrow(),
            vec!["action:issue-1", "action:issue-2"],
            "held messages must be applied in the order they arrived"
        );
        assert_eq!(queue.pending(), 0);
    }

    /// The drain must complete before `release` returns. A drain deferred to a
    /// later turn of the event loop — spawned, or reached through an `.await` —
    /// leaves a window in which a newly arrived message is applied ahead of the
    /// backlog, which is the exact bug this module exists to remove.
    #[test]
    fn release_drains_synchronously() {
        let (queue, applied) = recording();

        queue.deliver(action("issue-1"));
        queue.release();

        // No yield, no await, no turn of the event loop between the two lines.
        assert_eq!(
            *applied.borrow(),
            vec!["action:issue-1"],
            "release returned with messages still held — the drain was deferred, so a \
             message arriving before it runs would overtake this one"
        );
    }

    /// The ordering trap, made deterministic. A message that arrives *during*
    /// the drain must land behind the backlog it arrived behind, not in the
    /// middle of it.
    ///
    /// An implementation that flips to pass-through and then walks the buffer
    /// applies this one immediately on arrival and produces `1, 3, 2`. Only a
    /// single FIFO with no bypass produces `1, 2, 3`.
    #[test]
    fn a_message_arriving_mid_drain_lands_behind_the_backlog() {
        let applied = Rc::new(RefCell::new(Vec::new()));
        let queue: Rc<RefCell<Option<BroadcastQueue>>> = Rc::new(RefCell::new(None));

        let log = Rc::clone(&applied);
        let reentrant = Rc::clone(&queue);
        let built = BroadcastQueue::new(move |message| {
            let description = describe(message);
            log.borrow_mut().push(description.clone());
            // Applying the first held message delivers a third one, from inside
            // the drain. It must not overtake the second.
            if description == "action:issue-1"
                && let Some(queue) = reentrant.borrow().as_ref()
            {
                queue.deliver(action("issue-3"));
            }
        });
        *queue.borrow_mut() = Some(built.clone());

        built.deliver(action("issue-1"));
        built.deliver(action("issue-2"));
        built.release();

        assert_eq!(
            *applied.borrow(),
            vec!["action:issue-1", "action:issue-2", "action:issue-3"],
            "a message that arrived during the drain jumped ahead of one that arrived \
             before it — the reordering this module exists to make impossible"
        );
        assert_eq!(queue.borrow().as_ref().map(BroadcastQueue::pending), Some(0));
    }

    /// The transition the ticket names: two before, one after, all three in
    /// arrival order.
    #[test]
    fn arrival_order_survives_the_transition_from_held_to_live() {
        let (queue, applied) = recording();

        queue.deliver(action("issue-1"));
        queue.deliver(action("issue-2"));
        queue.release();
        queue.deliver(action("issue-3"));

        assert_eq!(
            *applied.borrow(),
            vec!["action:issue-1", "action:issue-2", "action:issue-3"],
            "the buffered and the live messages are one stream, not two"
        );
    }

    /// The decision this module makes explicitly: no variant bypasses the
    /// queue. A `CacheDelete` let through early could be serviced ahead of an
    /// action it should follow.
    #[test]
    fn every_variant_is_held_and_released_in_one_stream() {
        let (queue, applied) = recording();

        queue.deliver(action("issue-1"));
        queue.deliver(SyncBroadcastMessage::CacheDelete {
            entities: vec![CachedEntity::new("issue", "issue-1")],
        });
        queue.deliver(SyncBroadcastMessage::Reset);
        queue.deliver(SyncBroadcastMessage::Complete { last_sync_id: 9 });

        assert_eq!(
            queue.pending(),
            4,
            "no variant may take a fast path around the queue — a delete that \
             overtook the action it should follow would leave the cache holding a \
             row the user removed"
        );
        assert!(applied.borrow().is_empty());

        queue.release();

        assert_eq!(
            *applied.borrow(),
            vec![
                "action:issue-1",
                "cache_delete:issue-1",
                "reset",
                "complete:9",
            ],
            "every variant must come out in the order it went in"
        );
    }

    #[test]
    fn releasing_an_empty_queue_applies_nothing_and_lets_later_messages_through() {
        let (queue, applied) = recording();

        queue.release();
        assert!(queue.is_released());
        assert!(applied.borrow().is_empty());

        queue.deliver(action("issue-1"));
        assert_eq!(*applied.borrow(), vec!["action:issue-1"]);
    }

    /// A tab promoted to leader long after hydration finished re-runs the same
    /// wiring, so a second release must be a no-op rather than a replay.
    #[test]
    fn releasing_twice_does_not_replay_anything() {
        let (queue, applied) = recording();

        queue.deliver(action("issue-1"));
        queue.release();
        queue.release();

        assert_eq!(
            *applied.borrow(),
            vec!["action:issue-1"],
            "the backlog must be applied once, not once per release"
        );
    }

    #[test]
    fn clones_share_one_queue() {
        let (queue, applied) = recording();
        let other = queue.clone();

        queue.deliver(action("issue-1"));
        assert_eq!(
            other.pending(),
            1,
            "the handler and the releaser hold different clones of the same queue"
        );

        other.release();
        assert!(queue.is_released());
        assert_eq!(*applied.borrow(), vec!["action:issue-1"]);
    }
}

// ── Real browser tests (wasm32) ─────────────────────────────────────────────

/// Tests of the queue against a real `BroadcastChannel`.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use gloo_timers::future::TimeoutFuture;
    use trakkt_types::sync::{SyncAction, SyncActionType};
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::tab_leader::SyncBroadcast;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// `BroadcastChannel` delivery is a task, so it needs turns of the event
    /// loop rather than a duration.
    async fn settle() {
        for _ in 0..20 {
            TimeoutFuture::new(1).await;
        }
    }

    fn action(entity_id: &str) -> SyncBroadcastMessage {
        SyncBroadcastMessage::Action(SyncAction {
            sync_id: 1,
            entity_type: "issue".to_owned(),
            entity_id: entity_id.to_owned(),
            workspace_id: "ws-1".to_owned(),
            action: SyncActionType::Update,
            data: Some(serde_json::json!({"issue_id": entity_id})),
            timestamp: "2026-07-27T00:00:00Z".to_owned(),
        })
    }

    fn entity_id_of(message: &SyncBroadcastMessage) -> String {
        match message {
            SyncBroadcastMessage::Action(action) => action.entity_id.clone(),
            other => panic!("expected an action, got {other:?}"),
        }
    }

    /// The ticket's ordering criterion, over the real transport: two actions
    /// posted before the queue is released, one after, applied as one stream in
    /// arrival order.
    ///
    /// The synchronous assertion straight after `release()` is the half that
    /// catches a deferred drain: a drain reached through an `.await` or a
    /// `spawn_local` has applied nothing at that point.
    #[wasm_bindgen_test]
    async fn arrival_order_survives_the_release_over_a_real_channel() {
        let wid = "ws-queue-order";
        let leader = match SyncBroadcast::open(wid) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the leader's channel: {e:?}"),
        };
        let follower = match SyncBroadcast::open(wid) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the follower's channel: {e:?}"),
        };

        let applied: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&applied);
        let queue = BroadcastQueue::new(move |message| {
            log.borrow_mut().push(entity_id_of(message));
        });

        // Exactly the Layout's wiring: subscribe immediately, hold what arrives.
        let handler_queue = queue.clone();
        follower.set_on_message(move |message| handler_queue.deliver(message));

        leader.post(&action("issue-1"));
        leader.post(&action("issue-2"));
        settle().await;

        assert_eq!(
            queue.pending(),
            2,
            "fixture: both actions must have arrived while the queue was still held"
        );
        assert!(
            applied.borrow().is_empty(),
            "an action applied during hydration is wiped by it, and never re-delivered"
        );

        queue.release();
        assert_eq!(
            *applied.borrow(),
            vec!["issue-1", "issue-2"],
            "release returned before applying the backlog — a message arriving in that \
             window would be applied ahead of messages that arrived before it"
        );

        leader.post(&action("issue-3"));
        settle().await;

        assert_eq!(
            *applied.borrow(),
            vec!["issue-1", "issue-2", "issue-3"],
            "the held actions and the live one must be one ordered stream"
        );
    }

    /// A message posted before the subscription exists is gone for good. This
    /// is why the Layout subscribes first and holds, rather than deferring the
    /// subscription the way TRA-9921 deferred the dial.
    #[wasm_bindgen_test]
    async fn a_broadcast_channel_replays_nothing_to_a_late_subscriber() {
        let wid = "ws-queue-no-replay";
        let leader = match SyncBroadcast::open(wid) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the leader's channel: {e:?}"),
        };

        leader.post(&action("issue-missed"));
        settle().await;

        let follower = match SyncBroadcast::open(wid) {
            Ok(channel) => channel,
            Err(e) => panic!("failed to open the follower's channel: {e:?}"),
        };
        let applied: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&applied);
        let queue = BroadcastQueue::new(move |message| {
            log.borrow_mut().push(entity_id_of(message));
        });
        let handler_queue = queue.clone();
        follower.set_on_message(move |message| handler_queue.deliver(message));
        queue.release();
        settle().await;

        assert!(
            applied.borrow().is_empty(),
            "the channel replayed a message posted before the subscription existed — if \
             it did, deferring the subscription would have been a valid fix and this \
             queue would not need to exist"
        );
        assert_eq!(queue.pending(), 0);
    }
}
