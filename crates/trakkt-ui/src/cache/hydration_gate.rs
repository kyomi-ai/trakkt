// SPDX-License-Identifier: AGPL-3.0-or-later

//! One-shot latch that orders the leader tab's WebSocket dial *after* the
//! in-memory store has been hydrated from the local cache.
//!
//! ## Why this exists
//!
//! Hydration bulk-replaces every list in [`SyncStore`](crate::cache::store::SyncStore)
//! (`set_issues`, `set_labels`, …). The sync stream applies individual actions
//! to those same lists. Run concurrently, a bootstrap or delta action can be
//! applied and then wiped by a hydration `set_*` that resolves a moment later —
//! and because the cursor has already advanced past it, nothing ever re-delivers
//! that action. The entity stays stale until it is edited again or the page is
//! reloaded.
//!
//! The fix is ordering, not buffering: no message can be applied before
//! hydration finishes if the socket does not exist yet. This latch is what the
//! two halves of the startup sequence agree on — hydration opens it, the dial
//! waits for it.
//!
//! ## Why a latch and not the `initialized` signal
//!
//! `SyncStore::initialized` looks like the same fact, but it is not: the sync
//! engine also sets it on `sync_complete` and `SyncStore::reset` clears it, so
//! it is a UI-facing "is there anything to show" flag rather than a monotonic
//! "hydration has finished" edge. Gating the dial on it would couple startup
//! ordering to a signal the sync engine itself mutates. A latch says one thing,
//! says it once, and can be awaited directly — a Leptos signal cannot.
//!
//! ## Ordering
//!
//! Both startup orderings resolve correctly, which is the whole requirement:
//! the tab that wins leadership in the same pass that starts hydration parks on
//! [`HydrationGate::opened`] until hydration finishes, while a follower promoted
//! long after hydration completed sees an already-open gate and dials
//! immediately.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct GateState {
    open: bool,
    /// Waiters parked on the gate. Drained when it opens; the gate has a
    /// single waiter by construction (the leader tab's dial task).
    wakers: Vec<Waker>,
}

/// A latch that starts closed and opens exactly once.
///
/// Cheap to clone; every clone observes the same latch. Single-threaded by
/// construction — the startup sequence it orders runs entirely on the browser's
/// one JS task queue.
#[derive(Clone, Default)]
pub struct HydrationGate {
    state: Rc<RefCell<GateState>>,
}

impl HydrationGate {
    /// Create a closed gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the gate, releasing every current and future waiter.
    ///
    /// Idempotent: opening an already-open gate does nothing. Callers must open
    /// the gate on *every* path out of hydration, including failure — an empty
    /// store is a valid state to start syncing from, and a gate left closed
    /// would strand the tab with no socket at all.
    pub fn open(&self) {
        let wakers = {
            let mut state = self.state.borrow_mut();
            if state.open {
                return;
            }
            state.open = true;
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Whether the gate has been opened.
    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }

    /// A future that resolves when the gate opens, or immediately if it is
    /// already open.
    pub fn opened(&self) -> Opened {
        Opened {
            state: Rc::clone(&self.state),
        }
    }
}

/// Future returned by [`HydrationGate::opened`].
#[must_use = "the gate is only awaited if this future is polled"]
pub struct Opened {
    state: Rc<RefCell<GateState>>,
}

impl Future for Opened {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        if state.open {
            return Poll::Ready(());
        }
        if !state.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            state.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use futures::task::noop_waker;

    use super::*;

    /// Poll a future once with a waker that does nothing, so a test can assert
    /// on "still pending" rather than deadlocking to prove it.
    fn poll_once(fut: &mut Pin<Box<Opened>>) -> Poll<()> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        fut.as_mut().poll(&mut cx)
    }

    #[test]
    fn a_closed_gate_never_releases_its_waiter() {
        let gate = HydrationGate::new();
        let mut waiter = Box::pin(gate.opened());

        for _ in 0..10 {
            assert_eq!(
                poll_once(&mut waiter),
                Poll::Pending,
                "the socket must not be dialled while hydration is outstanding"
            );
        }
        assert!(!gate.is_open());
    }

    #[test]
    fn a_waiter_is_released_once_the_gate_opens() {
        let gate = HydrationGate::new();
        let mut waiter = Box::pin(gate.opened());

        assert_eq!(poll_once(&mut waiter), Poll::Pending);
        gate.open();
        assert_eq!(poll_once(&mut waiter), Poll::Ready(()));
    }

    /// The promoted-follower ordering: hydration finished long before this tab
    /// took the leadership lock, so the dial must not park forever waiting for
    /// an edge that has already passed.
    #[test]
    fn waiting_on_an_already_open_gate_resolves_immediately() {
        let gate = HydrationGate::new();
        gate.open();

        let mut waiter = Box::pin(gate.opened());
        assert_eq!(poll_once(&mut waiter), Poll::Ready(()));
    }

    #[test]
    fn opening_twice_is_harmless() {
        let gate = HydrationGate::new();
        gate.open();
        gate.open();

        let mut waiter = Box::pin(gate.opened());
        assert_eq!(poll_once(&mut waiter), Poll::Ready(()));
        assert!(gate.is_open());
    }

    #[test]
    fn every_waiter_is_released() {
        let gate = HydrationGate::new();
        let mut first = Box::pin(gate.opened());
        let mut second = Box::pin(gate.opened());

        assert_eq!(poll_once(&mut first), Poll::Pending);
        assert_eq!(poll_once(&mut second), Poll::Pending);

        gate.open();

        assert_eq!(poll_once(&mut first), Poll::Ready(()));
        assert_eq!(poll_once(&mut second), Poll::Ready(()));
    }

    /// Clones share one latch: hydration opens the gate through its clone, the
    /// dial task waits on a different clone.
    #[test]
    fn a_clone_opens_the_same_gate() {
        let gate = HydrationGate::new();
        let mut waiter = Box::pin(gate.opened());
        assert_eq!(poll_once(&mut waiter), Poll::Pending);

        gate.clone().open();

        assert!(gate.is_open());
        assert_eq!(poll_once(&mut waiter), Poll::Ready(()));
    }

    /// Re-polling before the gate opens must not accumulate one waker per poll.
    #[test]
    fn repeated_polls_register_a_single_waker() {
        let gate = HydrationGate::new();
        let mut waiter = Box::pin(gate.opened());

        for _ in 0..5 {
            assert_eq!(poll_once(&mut waiter), Poll::Pending);
        }

        assert_eq!(gate.state.borrow().wakers.len(), 1);
    }

    /// Hand control back to the executor exactly once.
    async fn yield_now() {
        let mut yielded = false;
        futures::future::poll_fn(move |cx| {
            if yielded {
                return Poll::Ready(());
            }
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;
    }

    /// The real executor path: a task parked on the gate resumes when another
    /// task opens it, rather than relying on manual polling.
    #[test]
    fn a_parked_task_is_woken_by_open() {
        use futures::executor::block_on;
        use std::cell::Cell;

        let gate = HydrationGate::new();
        let opener = gate.clone();
        let hydrated = Rc::new(Cell::new(false));

        let observed = Rc::clone(&hydrated);
        let waiter = async move {
            gate.opened().await;
            observed.get()
        };
        let hydration = async move {
            // Yield first so the waiter is genuinely parked before the open.
            yield_now().await;
            hydrated.set(true);
            opener.open();
        };

        let (hydrated_at_release, ()) = block_on(futures::future::join(waiter, hydration));
        assert!(
            hydrated_at_release,
            "the dial must not resume until hydration has actually finished"
        );
    }
}
