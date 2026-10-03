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

// ── Standing in for the server, so a page can be mounted ────────────────────

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

/// A page's server functions, answered from a table the test writes.
///
/// # What this is for
///
/// Most of what this crate tests is a function that was extracted so it could be
/// called directly. That leaves the thing nothing calls directly: the page
/// component, and specifically whether it *wires up* the helpers it is supposed
/// to. A page cannot be mounted without its server functions resolving, so
/// covering the wiring means standing in for the server — which is what this
/// does.
///
/// TRA-9994 is the ticket that wanted it: `ActivityPage` called
/// `refetch_on_live_activity`, three tests covered that function, and deleting
/// the call from the page left the whole wasm suite green. Only clippy's
/// `dead_code` noticed, and only for as long as the function had no other
/// caller.
///
/// # How it works
///
/// Leptos server functions reach the network through one call: `gloo-net` binds
/// the global `fetch` (`gloo-net-0.6.0/src/http/request.rs`) and
/// `server_fn::client::browser::BrowserClient` sends every request through it.
/// So replacing `globalThis.fetch` for the lifetime of this value intercepts all
/// of them, without the page or the server function knowing anything has
/// changed. Dropping it puts the original back — see the `Drop` impl for the
/// one case that does not cover.
///
/// A route matches when the request URL *contains* its key, so the key is just
/// the server function's name. The full path is not something to hard-code:
/// `#[server]` builds it as prefix + module path + function name + an xxh64 of
/// `CARGO_MANIFEST_DIR` and `module_path!()`
/// (`server_fn_macro-0.8.10/src/lib.rs`), so it moves whenever the function
/// changes module or the crate changes directory.
///
/// The body is the JSON the server function returns on success: for
/// `Result<Vec<T>, ServerFnError>`, `Ok(vec![])` is the body `[]`. That is the
/// whole protocol — `server_fn`'s `Http<PostUrl, Json>` treats any 2xx body as
/// the serialized `Ok` value and only reads status 400–599 as an error
/// (`server_fn-0.8.12/src/lib.rs`, `Protocol::run_client`).
///
/// # Using it
///
/// ```ignore
/// let server = stub_server_fns(&[
///     ("list_workspace_activities", "[]"),
///     ("list_teams", "[]"),
///     ("list_workspace_members", "[]"),
/// ]);
///
/// let handle = leptos::mount::mount_to(container.clone(), || view! { <ActivityPage/> });
/// TimeoutFuture::new(200).await;
///
/// assert_eq!(server.calls_to("list_workspace_activities"), 1);
/// assert!(server.unmatched().is_empty(), "{:?}", server.unmatched());
/// ```
///
/// Assert on [`StubbedServer::unmatched`]. A request this table has no answer
/// for is *not* failed here — it is answered with an undecodable body, because a
/// panic raised inside a `fetch` callback surfaces as a rejected promise, which
/// the page under test catches and logs, and the test then passes having proven
/// nothing. Recording it and letting the test assert on it is what makes a
/// missing route loud.
pub struct StubbedServer {
    /// The `fetch` that was installed before this value replaced it, restored on
    /// drop. Held as a `JsValue` rather than a function type because a page that
    /// never fetches is entitled to a global with no `fetch` at all.
    previous_fetch: JsValue,
    log: Rc<RefCell<CallLog>>,
    /// Kept alive because JS holds a reference to it. Dropping the `Closure`
    /// while `globalThis.fetch` still points at it would leave a dangling
    /// callback.
    _handler: Closure<dyn FnMut(JsValue) -> js_sys::Promise>,
}

#[derive(Default)]
struct CallLog {
    /// Every URL requested, in order, including unmatched ones.
    urls: Vec<String>,
    /// The URLs no route answered.
    unmatched: Vec<String>,
}

/// Answer `routes` for as long as the returned value is alive.
///
/// Each route is `(substring of the request URL, JSON response body)`. See
/// [`StubbedServer`] for what the body has to be and why the key is a substring.
pub fn stub_server_fns(routes: &[(&str, &str)]) -> StubbedServer {
    let routes: Vec<(String, String)> = routes
        .iter()
        .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
        .collect();

    let log = Rc::new(RefCell::new(CallLog::default()));
    let handler_log = Rc::clone(&log);

    let handler = Closure::wrap(Box::new(move |request: JsValue| -> js_sys::Promise {
        // `fetch` also accepts a URL string, but every caller that matters here
        // is `gloo-net`, which always passes a `Request`. Anything else means
        // the dispatch path changed underneath this harness, so it is recorded
        // as unmatched rather than guessed at.
        let url = match request.dyn_ref::<web_sys::Request>() {
            Some(request) => request.url(),
            None => "<fetch called with something other than a Request>".to_owned(),
        };

        let body = routes
            .iter()
            .find(|(path, _)| url.contains(path.as_str()))
            .map(|(_, body)| body.clone());

        {
            let mut log = handler_log.borrow_mut();
            log.urls.push(url.clone());
            if body.is_none() {
                log.unmatched.push(url);
            }
        }

        // An unmatched route falls back to the empty string, which is not a
        // fallback that lets a missing route pass for a working one: the empty
        // string is not valid JSON for any server function's output, so the
        // caller takes its error path immediately instead of waiting on a
        // request nothing will answer. That keeps the test failing on its own
        // assertion rather than on a timeout, and `unmatched` — which the caller
        // is told to assert on — is what names the cause.
        //
        // Spelled with `unwrap_or_default` rather than a `match` because
        // `clippy::manual_unwrap_or_default` rejects the `match`, and this
        // crate does not suppress lints.
        let body = body.unwrap_or_default();

        let response = web_sys::Response::new_with_opt_str(Some(&body))
            .expect("building the canned Response the stubbed fetch resolves to");
        js_sys::Promise::resolve(&JsValue::from(response))
    }) as Box<dyn FnMut(JsValue) -> js_sys::Promise>);

    let global = js_sys::global();
    let key = JsValue::from_str("fetch");
    let previous_fetch = js_sys::Reflect::get(&global, &key)
        .expect("reading the global `fetch` this stub is about to replace");
    js_sys::Reflect::set(&global, &key, handler.as_ref())
        .expect("installing the stubbed `fetch` on the global object");

    StubbedServer {
        previous_fetch,
        log,
        _handler: handler,
    }
}

impl StubbedServer {
    /// How many requests have been made to the server function whose path
    /// contains `path`.
    pub fn calls_to(&self, path: &str) -> usize {
        self.log
            .borrow()
            .urls
            .iter()
            .filter(|url| url.contains(path))
            .count()
    }

    /// Every request no route answered. Assert this is empty: a route that
    /// stopped matching turns a page-mount test into one that proves nothing.
    pub fn unmatched(&self) -> Vec<String> {
        self.log.borrow().unmatched.clone()
    }
}

impl Drop for StubbedServer {
    fn drop(&mut self) {
        // Every test in this crate shares one page and therefore one global
        // object, so a stub left installed would answer the next test's
        // requests. Restoring here rather than asking each test to is what keeps
        // that from being something anyone has to remember.
        //
        // This covers a test that returns, including one that fails an assertion
        // it reaches — not one that panics: the workspace sets `panic = "abort"`
        // and wasm32's own default is the same, so no destructor runs on that
        // path. The narrower guarantee is enough because a panicking test is
        // already a failing build, but it does mean a leaked stub is a possible
        // second symptom of a first failure rather than an independent one.
        let global = js_sys::global();
        let key = JsValue::from_str("fetch");
        js_sys::Reflect::set(&global, &key, &self.previous_fetch)
            .expect("restoring the global `fetch` this stub replaced");
    }
}

// ── Somewhere to mount ──────────────────────────────────────────────────────

/// A fresh `<div>` attached to the document, for `leptos::mount::mount_to`.
///
/// Attached rather than detached because a mounted subtree only lays out, and
/// only receives events, inside the live document.
///
/// The caller owns it: drop the mount handle and call `.remove()` when the test
/// is done, so the next test in the binary does not mount alongside this one's
/// leftovers.
pub fn mount_container() -> web_sys::HtmlElement {
    let document = web_sys::window()
        .expect("the browser test runner must provide a window")
        .document()
        .expect("the browser test runner must provide a document");
    let container: web_sys::HtmlElement = document
        .create_element("div")
        .expect("creating a container to mount into")
        .dyn_into()
        .expect("the container element is an HtmlElement");
    document
        .body()
        .expect("the document must have a body to attach the container to")
        .append_child(&container)
        .expect("attaching the container to the document body");
    container
}
