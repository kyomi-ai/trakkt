// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cross-tab sync leadership and the follower broadcast channel.
//!
//! Every tab of the same browser shares one `trakkt-sync` IndexedDB database.
//! When each tab ran its own sync engine they raced each other over the shared
//! entities *and* the shared per-workspace cursor, and because cache writes are
//! last-write-wins by arrival, a throttled tab flushing older queued writes
//! could overwrite a live tab's newer ones — leaving stale entities sitting
//! under a newer cursor, which `sync_delta` never re-sends.
//!
//! The fix is an election. Exactly one tab per (browser, workspace) holds an
//! exclusive Web Lock named `trakkt-sync-leader:{workspace_id}` and is the only
//! one that opens a WebSocket, runs the sync engine and writes the cache.
//! Follower tabs hydrate from the shared cache and receive applied actions over
//! a `BroadcastChannel`, updating their in-memory store only.
//!
//! Promotion is event-driven and needs no polling: a follower's lock request
//! simply sits in the browser's queue, and the moment the leader tab closes
//! (or crashes — the browser releases the lock either way) the request is
//! granted and that tab promotes itself.
//!
//! ## Why the Web Locks binding is hand-written
//!
//! `web-sys` 0.3.98 does have `Lock`, `LockManager` and `Navigator::locks()`,
//! but every one of them is additionally gated behind `#[cfg(
//! web_sys_unstable_apis)]` — the Cargo features alone do not expose them.
//! Enabling that cfg workspace-wide is not an option here: it also switches
//! *stable* web-sys signatures (`HtmlElement::scroll_top` becomes `f64`), which
//! breaks the `kode-leptos` path dependency this workspace builds against.
//!
//! So the four lines of `extern "C"` below bind `navigator.locks.request`
//! directly. This is wasm-bindgen's documented extension point for APIs
//! web-sys has not stabilised, not a workaround for a bug. Web Locks has been
//! Baseline since 2022, so the shape is settled.
//!
//! **Do not "simplify" this into a `web_sys::` import** — it will not compile,
//! and making it compile breaks an unrelated crate.

use serde::{Deserialize, Serialize};

use trakkt_types::sync::SyncAction;

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::{BroadcastChannel, MessageEvent};

// ── Wire format (all targets) ───────────────────────────────────────────────

/// One record in the shared entity cache.
///
/// The cache is keyed by `(entity_type, workspace_id, entity_id)` and the
/// channel is already scoped to a single workspace, so these two fields name a
/// row uniquely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedEntity {
    pub entity_type: String,
    pub entity_id: String,
}

impl CachedEntity {
    pub fn new(entity_type: &str, entity_id: &str) -> Self {
        Self {
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_owned(),
        }
    }
}

/// A message published between the tabs sharing one workspace's cache.
///
/// Serialized as JSON with a `type` discriminator, matching the shape of the
/// server's own [`trakkt_types::sync::SyncResponse`] envelope. The format is a
/// contract between tabs of the *same* build only — it never crosses the
/// network — but keeping it explicit means a stale tab that fails to decode a
/// message logs a warning instead of silently mis-applying it.
///
/// Almost every variant travels leader→follower and reports a write the leader
/// has **already** committed. [`SyncBroadcastMessage::CacheDelete`] is the one
/// that travels the other way: it *requests* a write, because a follower tab
/// has no cache writer of its own to perform it with.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncBroadcastMessage {
    /// The leader applied this action and its cache writes have committed.
    Action(SyncAction),
    /// The leader finished a bootstrap or delta stream and recorded the cursor.
    Complete { last_sync_id: i64 },
    /// The leader is wiping the shared cache and re-bootstrapping. Followers
    /// drop their in-memory state; the re-bootstrap stream refills it.
    Reset,
    /// A tab that does not own the cache asks the leader to delete these
    /// records from it.
    ///
    /// Only the leader acts on this — it enqueues the deletes on the same FIFO
    /// writer the sync stream uses, so a UI-initiated delete is ordered against
    /// the stream's writes to the same object store instead of racing them.
    /// Tabs that are not the leader ignore it. See
    /// [`crate::cache::delete_route`].
    CacheDelete { entities: Vec<CachedEntity> },
}

impl SyncBroadcastMessage {
    /// Encode for the wire, or `None` if the message cannot be serialized.
    pub fn encode(&self) -> Option<String> {
        match serde_json::to_string(self) {
            Ok(json) => Some(json),
            Err(e) => {
                tracing::warn!("sync broadcast: failed to encode message: {e}");
                None
            }
        }
    }

    /// Decode a message received from another tab, or `None` if it is not one.
    pub fn decode(raw: &str) -> Option<Self> {
        match serde_json::from_str(raw) {
            Ok(message) => Some(message),
            Err(e) => {
                tracing::warn!("sync broadcast: failed to decode message: {e}");
                None
            }
        }
    }
}

/// Name of the exclusive Web Lock that designates the sync leader.
///
/// Per workspace, so two tabs on different workspaces both sync — they write
/// different cache databases and different cursors, so they do not race.
pub fn leader_lock_name(workspace_id: &str) -> String {
    format!("trakkt-sync-leader:{workspace_id}")
}

/// Name of the `BroadcastChannel` the leader publishes applied actions on.
pub fn broadcast_channel_name(workspace_id: &str) -> String {
    format!("trakkt-sync:{workspace_id}")
}

// ── Web Locks binding (wasm32) ──────────────────────────────────────────────

// See the module docs: web-sys gates these behind `--cfg=web_sys_unstable_apis`,
// which cannot be enabled in this workspace.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
unsafe extern "C" {
    /// The one property of `navigator` this module needs.
    ///
    /// Declared locally rather than as a method on `web_sys::Navigator`, which
    /// the orphan rule forbids. Obtained by casting the real navigator, so it
    /// is the same object either way.
    type SyncNavigator;

    /// `navigator.locks` — https://developer.mozilla.org/docs/Web/API/LockManager
    type LockManager;

    #[wasm_bindgen(method, getter, js_name = "locks")]
    fn locks(this: &SyncNavigator) -> LockManager;

    /// `LockManager.request(name, options, callback)`.
    ///
    /// The returned promise settles only when `callback`'s promise settles, so
    /// a callback that never resolves holds the lock until the page goes away.
    #[wasm_bindgen(method, js_name = "request", catch)]
    fn request(
        this: &LockManager,
        name: &str,
        options: &js_sys::Object,
        callback: &js_sys::Function,
    ) -> Result<js_sys::Promise, JsValue>;
}

/// The callback the browser invokes when this tab is granted the lock. It
/// returns a promise that never settles, which is what holds the lock.
#[cfg(target_arch = "wasm32")]
type LockGrantCallback = Closure<dyn FnMut(JsValue) -> js_sys::Promise>;

/// Listener for messages published by the leader tab.
#[cfg(target_arch = "wasm32")]
type BroadcastListener = Closure<dyn FnMut(MessageEvent)>;

/// Does this object expose the Web Locks API?
///
/// Probed at the JS level rather than through the Rust type system: the binding
/// above returns a `LockManager` unconditionally, so there is no `Option` to
/// check. Browsers without Web Locks simply have no `locks` property.
#[cfg(target_arch = "wasm32")]
fn has_locks(navigator: &JsValue) -> bool {
    match js_sys::Reflect::has(navigator, &JsValue::from_str("locks")) {
        Ok(present) => present,
        Err(e) => {
            tracing::warn!("sync leadership: could not probe navigator.locks: {e:?}");
            false
        }
    }
}

/// Outcome of requesting sync leadership.
#[cfg(target_arch = "wasm32")]
pub enum Leadership {
    /// The request is queued with the browser. `on_granted` fires when this tab
    /// becomes leader — immediately if no other tab holds the lock, or later
    /// when the incumbent's tab closes.
    ///
    /// The returned handle owns the grant callback and must be kept alive for
    /// as long as the page may still be promoted, which in practice means the
    /// lifetime of the tab.
    Requested(LeadershipRequest),
    /// This browser cannot elect a leader, so the caller should sync as if it
    /// were alone. The documented capability fallback for pre-2022 browsers
    /// (and for opaque origins, where the API exists but refuses requests).
    Unsupported,
}

/// Keeps a pending or granted leadership request alive.
///
/// Dropping this frees the grant callback. Do that before the grant arrives and
/// the browser is left holding a dangling reference, so hold it for the page
/// lifetime.
#[cfg(target_arch = "wasm32")]
pub struct LeadershipRequest {
    _on_granted: LockGrantCallback,
}

/// Request the sync leadership lock for `workspace_id`.
///
/// `on_granted` runs exactly once, on the tab that holds the lock. It is never
/// called for a tab that stays a follower.
#[cfg(target_arch = "wasm32")]
pub fn acquire_leadership(
    workspace_id: &str,
    on_granted: impl FnOnce() + 'static,
) -> Leadership {
    let Some(window) = web_sys::window() else {
        tracing::warn!("sync leadership: no window object — syncing without an election");
        return Leadership::Unsupported;
    };
    let navigator = window.navigator();
    if !has_locks(navigator.as_ref()) {
        return Leadership::Unsupported;
    }

    let mut on_granted = Some(on_granted);
    let callback = LockGrantCallback::new(move |_lock: JsValue| -> js_sys::Promise {
            match on_granted.take() {
                Some(callback) => callback(),
                None => tracing::warn!("sync leadership: lock granted twice for one request"),
            }
            // Hold the lock for the lifetime of the page. The browser releases
            // it when this promise settles — and it never does — or when the
            // tab goes away, which is what lets a follower be promoted.
            js_sys::Promise::new(&mut |_resolve, _reject| {})
        },
    );

    let options = js_sys::Object::new();
    if let Err(e) = js_sys::Reflect::set(
        &options,
        &JsValue::from_str("mode"),
        &JsValue::from_str("exclusive"),
    ) {
        tracing::warn!("sync leadership: could not set lock mode: {e:?}");
        return Leadership::Unsupported;
    }

    let name = leader_lock_name(workspace_id);
    let locks = navigator.unchecked_ref::<SyncNavigator>().locks();
    match locks.request(&name, &options, callback.as_ref().unchecked_ref())
    {
        Ok(pending) => {
            // The request promise settles only on failure (the success path is
            // the never-resolving callback promise above), so anything that
            // arrives here is worth reporting.
            wasm_bindgen_futures::spawn_local(async move {
                match wasm_bindgen_futures::JsFuture::from(pending).await {
                    Ok(_) => tracing::warn!(
                        "sync leadership: lock released unexpectedly — this tab is no longer leader"
                    ),
                    Err(e) => tracing::warn!("sync leadership: lock request failed: {e:?}"),
                }
            });
            Leadership::Requested(LeadershipRequest {
                _on_granted: callback,
            })
        }
        Err(e) => {
            // The API is present but refused outright — opaque origins throw
            // SecurityError. Treat it as an unelectable browser rather than
            // leaving this tab permanently unsynced.
            tracing::warn!("sync leadership: navigator.locks.request threw: {e:?}");
            Leadership::Unsupported
        }
    }
}

// ── Broadcast channel (wasm32) ──────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
struct SyncBroadcastInner {
    channel: BroadcastChannel,
    /// Owns the message listener so it is dropped with the channel rather than
    /// leaked with `Closure::forget`.
    listener: RefCell<Option<BroadcastListener>>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for SyncBroadcastInner {
    fn drop(&mut self) {
        self.channel.set_onmessage(None);
        self.channel.close();
    }
}

/// The leader's publish channel and the followers' subscription to it.
///
/// A `BroadcastChannel` never delivers to the object that posted, so the leader
/// does not receive its own actions back.
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub struct SyncBroadcast {
    inner: Rc<SyncBroadcastInner>,
}

#[cfg(target_arch = "wasm32")]
impl SyncBroadcast {
    /// Open the channel for `workspace_id`.
    pub fn open(workspace_id: &str) -> Result<Self, JsValue> {
        let channel = BroadcastChannel::new(&broadcast_channel_name(workspace_id))?;
        Ok(Self {
            inner: Rc::new(SyncBroadcastInner {
                channel,
                listener: RefCell::new(None),
            }),
        })
    }

    /// Publish a message to every other tab on this channel.
    pub fn post(&self, message: &SyncBroadcastMessage) {
        let Some(payload) = message.encode() else {
            return;
        };
        if let Err(e) = self.inner.channel.post_message(&JsValue::from_str(&payload)) {
            tracing::warn!("sync broadcast: failed to post message: {e:?}");
        }
    }

    /// Handle messages published by the leader tab.
    ///
    /// Replaces any previously registered handler.
    pub fn set_on_message(&self, handler: impl Fn(SyncBroadcastMessage) + 'static) {
        let listener = BroadcastListener::new(move |event: MessageEvent| {
            let Some(raw) = event.data().as_string() else {
                tracing::warn!("sync broadcast: received a non-string message — ignoring");
                return;
            };
            if let Some(message) = SyncBroadcastMessage::decode(&raw) {
                handler(message);
            }
        });
        self.inner
            .channel
            .set_onmessage(Some(listener.as_ref().unchecked_ref()));
        *self.inner.listener.borrow_mut() = Some(listener);
    }
}

// ── Real browser tests (wasm32) ─────────────────────────────────────────────

/// Tests against the browser's real Web Locks and BroadcastChannel.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use std::cell::Cell;

    use gloo_timers::future::TimeoutFuture;
    use trakkt_types::sync::SyncActionType;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Let the event loop run so lock grants and channel messages can land.
    async fn settle() {
        for _ in 0..10 {
            TimeoutFuture::new(1).await;
        }
    }

    fn sample_action() -> SyncAction {
        SyncAction {
            sync_id: 42,
            entity_type: "issue".to_owned(),
            entity_id: "issue-1".to_owned(),
            workspace_id: "ws-broadcast".to_owned(),
            action: SyncActionType::Update,
            data: Some(serde_json::json!({"issue_id": "issue-1", "title": "hello"})),
            timestamp: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    /// Take the leader lock with a callback promise the test can settle, so it
    /// can hand leadership over on demand — which is what a closing tab does.
    type ReleaseSlot = Rc<RefCell<Option<js_sys::Function>>>;

    fn hold_lock(workspace_id: &str) -> (LockGrantCallback, ReleaseSlot) {
        let release: ReleaseSlot = Rc::new(RefCell::new(None));
        let release_slot = Rc::clone(&release);
        let callback = LockGrantCallback::new(move |_lock: JsValue| -> js_sys::Promise {
                let slot = Rc::clone(&release_slot);
                js_sys::Promise::new(&mut |resolve, _reject| {
                    *slot.borrow_mut() = Some(resolve);
                })
            },
        );

        let navigator = web_sys::window().expect("window").navigator();
        let options = js_sys::Object::new();
        js_sys::Reflect::set(
            &options,
            &JsValue::from_str("mode"),
            &JsValue::from_str("exclusive"),
        )
        .expect("set mode");
        // The request promise settles only when the lock is released, which the
        // test drives through `release` rather than by awaiting this.
        let _pending = navigator
            .unchecked_ref::<SyncNavigator>()
            .locks()
            .request(
                &leader_lock_name(workspace_id),
                &options,
                callback.as_ref().unchecked_ref(),
            )
            .expect("lock request");

        (callback, release)
    }

    #[wasm_bindgen_test]
    async fn leadership_is_granted_when_no_other_tab_holds_the_lock() {
        let granted = Rc::new(Cell::new(false));
        let flag = Rc::clone(&granted);

        let request = acquire_leadership("ws-election-free", move || flag.set(true));
        assert!(
            matches!(request, Leadership::Requested(_)),
            "a browser with Web Locks must queue the request, not report it unsupported"
        );

        settle().await;
        assert!(granted.get(), "the first tab must become leader");
    }

    #[wasm_bindgen_test]
    async fn only_one_tab_is_granted_leadership_at_a_time() {
        // Two tabs standing for election through the production path — the
        // whole point of the lock. A non-exclusive request would let both in,
        // which is the multi-writer race this ticket exists to remove.
        let first = Rc::new(Cell::new(false));
        let flag = Rc::clone(&first);
        let _first_request = acquire_leadership("ws-election-exclusive", move || flag.set(true));

        let second = Rc::new(Cell::new(false));
        let flag = Rc::clone(&second);
        let _second_request = acquire_leadership("ws-election-exclusive", move || flag.set(true));

        settle().await;

        assert!(first.get(), "the first tab must be granted leadership");
        assert!(
            !second.get(),
            "a second tab must never hold the leadership lock at the same time"
        );
    }

    #[wasm_bindgen_test]
    async fn leadership_waits_for_the_incumbent_then_promotes_on_release() {
        let workspace = "ws-election-contended";

        // An incumbent leader holds the lock.
        let (_incumbent, release) = hold_lock(workspace);
        settle().await;
        assert!(
            release.borrow().is_some(),
            "the incumbent should have been granted the lock first"
        );

        // A second tab requests leadership through the production path.
        let granted = Rc::new(Cell::new(false));
        let flag = Rc::clone(&granted);
        let _request = acquire_leadership(workspace, move || flag.set(true));

        settle().await;
        assert!(
            !granted.get(),
            "a second tab must not be granted leadership while the lock is held"
        );

        // The incumbent's tab goes away: its callback promise settles and the
        // browser hands the lock to the waiter.
        let resolve = release.borrow_mut().take().expect("release fn");
        resolve.call0(&JsValue::NULL).expect("resolve");

        settle().await;
        assert!(
            granted.get(),
            "the waiting tab must be promoted once the incumbent releases"
        );
    }

    #[wasm_bindgen_test]
    fn the_locks_probe_distinguishes_a_navigator_without_the_api() {
        let navigator = web_sys::window().expect("window").navigator();
        assert!(
            has_locks(navigator.as_ref()),
            "this browser is expected to support Web Locks"
        );

        let without = js_sys::Object::new();
        assert!(
            !has_locks(without.as_ref()),
            "an object with no `locks` property must probe as unsupported"
        );
    }

    #[wasm_bindgen_test]
    async fn an_action_round_trips_between_two_channels() {
        let leader = SyncBroadcast::open("ws-broadcast").expect("open leader channel");
        let follower = SyncBroadcast::open("ws-broadcast").expect("open follower channel");

        let received: Rc<RefCell<Option<SyncBroadcastMessage>>> = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&received);
        follower.set_on_message(move |message| *slot.borrow_mut() = Some(message));

        let sent = sample_action();
        leader.post(&SyncBroadcastMessage::Action(sent.clone()));
        settle().await;

        let got = received.borrow();
        let Some(SyncBroadcastMessage::Action(action)) = got.as_ref() else {
            panic!("the follower channel received {got:?}, expected an action");
        };
        assert_eq!(
            serde_json::to_value(action).expect("re-encode received"),
            serde_json::to_value(&sent).expect("re-encode sent"),
            "the action must survive the round trip unchanged"
        );
    }

    #[wasm_bindgen_test]
    async fn reset_and_complete_markers_round_trip() {
        let leader = SyncBroadcast::open("ws-broadcast-markers").expect("open leader channel");
        let follower = SyncBroadcast::open("ws-broadcast-markers").expect("open follower channel");

        let received: Rc<RefCell<Vec<SyncBroadcastMessage>>> = Rc::new(RefCell::new(Vec::new()));
        let slot = Rc::clone(&received);
        follower.set_on_message(move |message| slot.borrow_mut().push(message));

        leader.post(&SyncBroadcastMessage::Reset);
        leader.post(&SyncBroadcastMessage::Complete { last_sync_id: 77 });
        settle().await;

        let got = received.borrow();
        assert!(
            matches!(got.first(), Some(SyncBroadcastMessage::Reset)),
            "expected the reset marker first, got {got:?}"
        );
        assert!(
            matches!(
                got.get(1),
                Some(SyncBroadcastMessage::Complete { last_sync_id: 77 })
            ),
            "expected the completion marker second, got {got:?}"
        );
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use trakkt_types::sync::SyncActionType;

    use super::*;

    fn sample_action() -> SyncAction {
        SyncAction {
            sync_id: 7,
            entity_type: "issue".to_owned(),
            entity_id: "issue-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            action: SyncActionType::Update,
            data: Some(serde_json::json!({"issue_id": "issue-1", "title": "hello"})),
            timestamp: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn an_action_survives_encode_decode() {
        let action = sample_action();
        let encoded = SyncBroadcastMessage::Action(action.clone())
            .encode()
            .expect("encode");

        let Some(SyncBroadcastMessage::Action(decoded)) = SyncBroadcastMessage::decode(&encoded)
        else {
            panic!("decoded to the wrong variant");
        };

        assert_eq!(
            serde_json::to_value(&decoded).expect("re-encode decoded"),
            serde_json::to_value(&action).expect("re-encode original"),
            "a follower must apply exactly what the leader applied"
        );
    }

    #[test]
    fn markers_survive_encode_decode() {
        let encoded = SyncBroadcastMessage::Complete { last_sync_id: 99 }
            .encode()
            .expect("encode");
        assert!(matches!(
            SyncBroadcastMessage::decode(&encoded),
            Some(SyncBroadcastMessage::Complete { last_sync_id: 99 })
        ));

        let encoded = SyncBroadcastMessage::Reset.encode().expect("encode");
        assert!(matches!(
            SyncBroadcastMessage::decode(&encoded),
            Some(SyncBroadcastMessage::Reset)
        ));
    }

    #[test]
    fn the_wire_format_is_type_tagged() {
        let encoded = SyncBroadcastMessage::Reset.encode().expect("encode");
        assert_eq!(encoded, r#"{"type":"reset"}"#);

        let encoded = SyncBroadcastMessage::Complete { last_sync_id: 5 }
            .encode()
            .expect("encode");
        assert_eq!(encoded, r#"{"type":"complete","last_sync_id":5}"#);
    }

    #[test]
    fn garbage_decodes_to_none_rather_than_panicking() {
        assert!(SyncBroadcastMessage::decode("not json").is_none());
        assert!(SyncBroadcastMessage::decode(r#"{"type":"from_a_newer_build"}"#).is_none());
    }

    #[test]
    fn coordination_names_are_scoped_per_workspace() {
        assert_eq!(leader_lock_name("ws-1"), "trakkt-sync-leader:ws-1");
        assert_eq!(broadcast_channel_name("ws-1"), "trakkt-sync:ws-1");
        assert_ne!(leader_lock_name("ws-1"), leader_lock_name("ws-2"));
        assert_ne!(
            broadcast_channel_name("ws-1"),
            broadcast_channel_name("ws-2")
        );
    }
}
