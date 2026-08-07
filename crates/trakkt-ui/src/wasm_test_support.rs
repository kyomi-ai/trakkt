// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared setup for the crate's browser tests.
//!
//! This lives at the crate root rather than beside any one caller because its
//! callers are in two different subtrees — `cache/` and `pages/settings/` — so
//! the crate root is the only module that is an ancestor of all of them.
//!
//! Run the tests this supports with:
//! `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`

/// Boot the executor Leptos spawns its async work onto.
///
/// Production gets this from `mount_to_body` / `hydrate_body`, which a
/// `wasm-bindgen-test` never calls — so without it, anything that spawns
/// panics, or, where the spawn is what the test was watching for, never runs
/// and the test passes for the wrong reason. `Effect::new`, `Resource::new` and
/// `LocalResource::new` all spawn the moment they are constructed, so a test
/// that builds any of them before it mounts anything must call this first.
///
/// It is the same executor production uses either way: `init_wasm_bindgen`
/// installs `wasm_bindgen_futures::spawn_local`, which is what
/// `leptos::task::spawn_local` resolves to on this target.
///
/// # Why every test calls it
///
/// The executor is global and set once per page, and every test in a
/// `wasm-bindgen-test` binary shares one page. So a test that spawns without
/// calling this does not fail reliably — it fails only when it happens to run
/// before whichever test did call it. That is an ordering dependency, and it
/// is invisible locally right up until a different runner picks a different
/// order. Calling this unconditionally is what makes each test stand alone;
/// a second caller being told the executor is already set is the answer it
/// wanted, which is why `AlreadySet` is a success here rather than an error.
pub fn boot_leptos_executor() {
    match any_spawner::Executor::init_wasm_bindgen() {
        Ok(()) | Err(any_spawner::ExecutorError::AlreadySet) => {}
    }
}

// ── Driving a save the test controls ────────────────────────────────────────

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use send_wrapper::SendWrapper;

/// A `!Send` future made nominally `Send` so it can be an `Action`'s body.
///
/// `Action::new` requires `Future: Send`, and the fixture bodies below capture
/// `Rc` counters and a [`Gate`]. WASM is single-threaded, so nothing ever polls
/// this from another thread — the same reasoning
/// `pages/settings/live_update.rs`'s `latency_tests::SendTimer` records.
/// `send_wrapper` has its own `Future` impl but it sits behind a `futures`
/// feature this crate does not enable, so the impl is written out here.
pub struct LocalFuture<O>(SendWrapper<Pin<Box<dyn Future<Output = O>>>>);

impl<O> LocalFuture<O> {
    pub fn new(fut: impl Future<Output = O> + 'static) -> Self {
        Self(SendWrapper::new(Box::pin(fut)))
    }
}

impl<O> Future for LocalFuture<O> {
    type Output = O;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<O> {
        self.get_mut().0.as_mut().poll(cx)
    }
}

#[derive(Default)]
struct GateInner {
    open: bool,
    waker: Option<Waker>,
}

/// A latch the test opens by hand: the save cannot resolve until it does.
///
/// A latch rather than a timer on purpose. "The save was still on the wire when
/// the component rebuilt" is an ordering, and a timer makes it a race the test
/// can lose without noticing — which is the shape of failure these tests exist
/// to stop repeating.
#[derive(Clone, Default)]
pub struct Gate {
    inner: Rc<RefCell<GateInner>>,
}

impl Gate {
    /// Let every save waiting on this gate resolve.
    pub fn open(&self) {
        let waker = {
            let mut inner = self.inner.borrow_mut();
            inner.open = true;
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Wait until [`Gate::open`] is called.
    pub fn wait(&self) -> GateWait {
        GateWait {
            gate: self.clone(),
        }
    }
}

/// The future [`Gate::wait`] hands back.
pub struct GateWait {
    gate: Gate,
}

impl Future for GateWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.get_mut().gate.inner.borrow_mut();
        if inner.open {
            Poll::Ready(())
        } else {
            inner.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// How far each dispatched save got.
///
/// Held in `Rc<Cell<_>>` rather than in signals so that the instrument is not
/// itself an arena item — a probe about disposal must not be measured with
/// something disposal can reach.
#[derive(Clone, Default)]
pub struct SaveLog {
    started: Rc<Cell<u32>>,
    finished: Rc<Cell<u32>>,
}

impl SaveLog {
    /// Record that a dispatched save has begun running.
    pub fn record_start(&self) {
        self.started.set(self.started.get() + 1);
    }

    /// Record that a dispatched save has run to completion.
    pub fn record_finish(&self) {
        self.finished.set(self.finished.get() + 1);
    }

    /// How many dispatched saves have begun running.
    pub fn started(&self) -> u32 {
        self.started.get()
    }

    /// How many have run to completion.
    pub fn finished(&self) -> u32 {
        self.finished.get()
    }
}
