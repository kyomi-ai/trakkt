// SPDX-License-Identifier: AGPL-3.0-or-later

//! Centralized WebSocket client for the Trakkt sync engine.
//!
//! Handles WebSocket connection lifecycle for the sync protocol only
//! (no chat, no message deduplication, no subscriber system).
//!
//! The client provides:
//! - `ConnectionState` — reactive signal tracking the connection lifecycle
//! - `WebSocketClient` — handle to the live connection, with `send()` and
//!   `on_message()` for the sync engine to hook into
//! - Automatic reconnection with exponential backoff (1s to 30s max)
//!
//! All types here use `web_sys::WebSocket` directly (browser-native API).
//! This module is gated to `wasm32` at the call site in `cache/mod.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use trakkt_types::sync::SyncResponse;

/// WebSocket connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connecting => write!(f, "connecting"),
            Self::Connected => write!(f, "connected"),
            Self::Reconnecting => write!(f, "reconnecting"),
        }
    }
}

/// Callback type for incoming sync messages.
type MessageCallback = Box<dyn Fn(SyncResponse)>;

/// Base reconnect delay in milliseconds.
const BASE_RECONNECT_DELAY_MS: u32 = 1000;

/// Maximum reconnect delay in milliseconds (30 seconds).
const MAX_RECONNECT_DELAY_MS: u32 = 30_000;

/// Shared mutable state for the WebSocket connection.
///
/// Wrapped in `Rc<RefCell<>>` because WASM is single-threaded and we need
/// interior mutability from multiple closures (event handlers, send).
struct WsState {
    /// The live WebSocket connection, if any.
    ws: Option<WebSocket>,
    /// Event handler closures — stored to prevent GC.
    _closures: Vec<JsValue>,
    /// Callback invoked on every incoming message.
    on_message: Option<MessageCallback>,
    /// Whether a `connect()` call is in progress (prevents concurrent connects).
    connecting: bool,
    /// Whether close was intentional (disconnect called).
    intentional_close: bool,
    /// Current reconnect attempt count.
    reconnect_attempts: u32,
    /// Handle to the pending reconnect timeout (if any).
    reconnect_timeout: Option<SendWrapper<gloo_timers::callback::Timeout>>,
}

impl WsState {
    fn new() -> Self {
        Self {
            ws: None,
            _closures: Vec::new(),
            on_message: None,
            connecting: false,
            intentional_close: false,
            reconnect_attempts: 0,
            reconnect_timeout: None,
        }
    }
}

/// Inner non-Send state handle. Wrapped in `SendWrapper` inside
/// `WebSocketClient` so Leptos context's `Send + Sync` bounds are satisfied
/// (WASM is single-threaded, so `SendWrapper` is a transparent no-op).
struct WsHandle {
    state: Rc<RefCell<WsState>>,
}

/// Handle to the WebSocket connection.
///
/// Cheaply `Clone`-able — the actual data lives behind a `StoredValue`.
/// Provide via context at the Layout level and access with
/// `expect_context::<WebSocketClient>()`.
#[derive(Clone)]
pub struct WebSocketClient {
    inner: StoredValue<SendWrapper<WsHandle>>,
    /// Reactive connection state signal.
    pub connection_state: ArcRwSignal<ConnectionState>,
}

impl WebSocketClient {
    /// Send a JSON message through the WebSocket.
    ///
    /// Returns `true` if sent successfully, `false` if not connected.
    pub fn send(&self, message: serde_json::Value) -> bool {
        self.inner.with_value(|handle| {
            let s = handle.state.borrow();
            if let Some(ref ws) = s.ws
                && ws.ready_state() == WebSocket::OPEN
                && let Ok(json) = serde_json::to_string(&message)
            {
                return ws.send_with_str(&json).is_ok();
            }
            false
        })
    }

    /// Register the message callback. Only one callback is supported — the
    /// sync engine is the sole consumer of incoming WebSocket messages.
    pub fn set_on_message(&self, callback: impl Fn(SyncResponse) + 'static) {
        self.inner.with_value(|handle| {
            handle.state.borrow_mut().on_message = Some(Box::new(callback));
        });
    }

    /// Reconnect with a new token (e.g., after fetching a JWT asynchronously).
    ///
    /// Closes the existing connection and opens a new one with the provided
    /// credentials. The `on_message` callback is preserved across reconnects.
    pub fn reconnect(&self, user_id: &str, workspace_id: &str, token: &str) {
        let state = self.inner.with_value(|handle| handle.state.clone());
        let conn_state = self.connection_state.clone();

        // Close existing connection without triggering auto-reconnect.
        {
            let mut s = state.borrow_mut();
            if let Some(ref ws) = s.ws {
                ws.set_onclose(None);
                ws.set_onerror(None);
                ws.set_onopen(None);
                let _ = ws.close();
            }
            s.ws = None;
            s._closures.clear();
            s.connecting = false;
            s.reconnect_attempts = 0;
        }

        do_connect(state, conn_state, user_id, workspace_id, token);
    }

    /// Access the raw `Rc<RefCell<WsState>>` for internal operations.
    /// Only used within this module by `disconnect()`.
    fn with_state<R>(&self, f: impl FnOnce(&Rc<RefCell<WsState>>) -> R) -> R {
        self.inner.with_value(|handle| f(&handle.state))
    }
}

/// Build a WebSocket URL from the current page location.
///
/// Format: `ws[s]://host:port/ws/{workspace_id}_{user_id}?token={token}`
fn build_ws_url(user_id: &str, workspace_id: &str, token: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window")?;
    let location = window.location();

    let protocol = location.protocol().map_err(|_| "no protocol")?;
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };

    let host = location.host().map_err(|_| "no host")?;

    Ok(format!(
        "{ws_protocol}//{host}/ws/{workspace_id}_{user_id}?token={token}"
    ))
}

/// Connect to the WebSocket server and return a client handle.
///
/// Wires up event handlers for open, message, error, and close. On close,
/// schedules automatic reconnection with exponential backoff unless
/// `disconnect()` was called intentionally.
pub fn connect(user_id: &str, workspace_id: &str, token: &str) -> WebSocketClient {
    web_sys::console::log_1(&format!("[trakkt-sync] connect({user_id}, {workspace_id})").into());
    let connection_state = ArcRwSignal::new(ConnectionState::Disconnected);
    let state = Rc::new(RefCell::new(WsState::new()));

    let client = WebSocketClient {
        inner: StoredValue::new(SendWrapper::new(WsHandle {
            state: state.clone(),
        })),
        connection_state: connection_state.clone(),
    };

    let uid = user_id.to_owned();
    let wid = workspace_id.to_owned();
    let tok = token.to_owned();

    web_sys::console::log_1(&"[trakkt-sync] calling do_connect".into());
    do_connect(state, connection_state, &uid, &wid, &tok);

    client
}

/// Intentionally disconnect the WebSocket.
///
/// Nulls event handlers before closing to prevent spurious reconnect
/// triggers. Cancels any pending reconnect timeout.
pub fn disconnect(client: &WebSocketClient) {
    client.with_state(|state| {
        let mut s = state.borrow_mut();
        s.intentional_close = true;

        // Cancel any pending reconnect timeout
        s.reconnect_timeout = None;

        // Close the WebSocket
        if let Some(ref ws) = s.ws {
            ws.set_onclose(None);
            ws.set_onerror(None);
            ws.set_onmessage(None);
            ws.set_onopen(None);
            let _ = ws.close();
        }
        s.ws = None;
        s._closures.clear();
    });
    client.connection_state.set(ConnectionState::Disconnected);
}

/// Internal: perform the actual WebSocket connection.
fn do_connect(
    state: Rc<RefCell<WsState>>,
    connection_state: ArcRwSignal<ConnectionState>,
    user_id: &str,
    workspace_id: &str,
    token: &str,
) {
    // Guard against concurrent connect() calls.
    {
        let s = state.borrow();
        if s.connecting {
            tracing::info!("connect(): already connecting, skipping");
            return;
        }
        if let Some(ref ws) = s.ws {
            if ws.ready_state() == WebSocket::OPEN {
                tracing::info!("connect(): already OPEN, skipping");
                return;
            }
        }
    }

    // Set the guard and clean up any stale non-OPEN connection.
    {
        let mut s = state.borrow_mut();
        s.connecting = true;
        if let Some(ref ws) = s.ws {
            ws.set_onclose(None);
            ws.set_onerror(None);
            ws.set_onmessage(None);
            ws.set_onopen(None);
            let _ = ws.close();
        }
        s.ws = None;
        s._closures.clear();
    }

    connection_state.set(ConnectionState::Connecting);

    let url = match build_ws_url(user_id, workspace_id, token) {
        Ok(u) => {
            web_sys::console::log_1(&format!("[trakkt-sync] WS URL: {u}").into());
            u
        }
        Err(e) => {
            web_sys::console::error_1(&format!("[trakkt-sync] Failed to build WS URL: {e}").into());
            state.borrow_mut().connecting = false;
            connection_state.set(ConnectionState::Disconnected);
            schedule_reconnect(
                state,
                connection_state,
                user_id.to_owned(),
                workspace_id.to_owned(),
                token.to_owned(),
            );
            return;
        }
    };

    let ws = match WebSocket::new(&url) {
        Ok(ws) => {
            web_sys::console::log_1(&"[trakkt-sync] WebSocket::new succeeded".into());
            ws
        }
        Err(e) => {
            web_sys::console::error_1(&format!("[trakkt-sync] WebSocket::new FAILED: {:?}", e).into());
            state.borrow_mut().connecting = false;
            connection_state.set(ConnectionState::Disconnected);
            schedule_reconnect(
                state,
                connection_state,
                user_id.to_owned(),
                workspace_id.to_owned(),
                token.to_owned(),
            );
            return;
        }
    };

    let mut closures: Vec<JsValue> = Vec::new();

    // -- onopen ---------------------------------------------------------------
    let onopen_state = state.clone();
    let onopen_conn = connection_state.clone();
    let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
        web_sys::console::log_1(&"[trakkt-sync] WS OPEN - connected!".into());
        onopen_conn.set(ConnectionState::Connected);
        onopen_state.borrow_mut().reconnect_attempts = 0;
    });
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    closures.push(on_open.into_js_value());

    // -- onmessage ------------------------------------------------------------
    let onmsg_state = state.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(text) = event.data().as_string() else {
            return;
        };

        let raw: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return,
        };
        let msg_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(msg_type, "sync_action" | "sync_complete" | "sync_reset") {
            return;
        }

        let msg: SyncResponse = match serde_json::from_value(raw) {
            Ok(m) => m,
            Err(e) => {
                web_sys::console::warn_1(&format!("[trakkt-sync] parse error: {e}").into());
                return;
            }
        };

        let s = onmsg_state.borrow();
        if let Some(ref cb) = s.on_message {
            cb(msg);
        }
    });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    closures.push(on_message.into_js_value());

    // -- onerror --------------------------------------------------------------
    let onerr_conn = connection_state.clone();
    let on_error = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
        web_sys::console::error_1(&format!("[trakkt-sync] WS ERROR: {:?}", e).into());
        onerr_conn.set(ConnectionState::Disconnected);
    });
    ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    closures.push(on_error.into_js_value());

    // -- onclose --------------------------------------------------------------
    let onclose_state = state.clone();
    let onclose_conn = connection_state.clone();
    let uid_close = user_id.to_owned();
    let wid_close = workspace_id.to_owned();
    let tok_close = token.to_owned();
    let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
        let code = event.code();
        let reason = event.reason();
        web_sys::console::log_1(&format!("[trakkt-sync] WS CLOSED code={code} reason={reason}").into());

        onclose_conn.set(ConnectionState::Disconnected);
        onclose_state.borrow_mut().ws = None;

        // Only attempt reconnection if not intentionally closed.
        let intentional = onclose_state.borrow().intentional_close;
        if !intentional {
            schedule_reconnect(
                onclose_state.clone(),
                onclose_conn.clone(),
                uid_close.clone(),
                wid_close.clone(),
                tok_close.clone(),
            );
        }
    });
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    closures.push(on_close.into_js_value());

    // Store the connection and clear the connecting guard.
    let mut s = state.borrow_mut();
    s.ws = Some(ws);
    s._closures = closures;
    s.connecting = false;
}

/// Schedule a reconnect with exponential backoff.
///
/// Delay formula: `min(1000 * 2^attempt, 30000)`.
fn schedule_reconnect(
    state: Rc<RefCell<WsState>>,
    connection_state: ArcRwSignal<ConnectionState>,
    user_id: String,
    workspace_id: String,
    token: String,
) {
    let (intentional, attempts) = {
        let s = state.borrow();
        (s.intentional_close, s.reconnect_attempts)
    };

    if intentional {
        return;
    }

    let delay_ms = std::cmp::min(
        BASE_RECONNECT_DELAY_MS.saturating_mul(2u32.saturating_pow(attempts)),
        MAX_RECONNECT_DELAY_MS,
    );

    {
        let mut s = state.borrow_mut();
        s.reconnect_attempts = attempts + 1;
    }

    connection_state.set(ConnectionState::Reconnecting);

    tracing::info!(
        "WebSocket reconnect attempt {} in {}ms",
        attempts + 1,
        delay_ms,
    );

    let reconnect_state = state.clone();
    let reconnect_conn = connection_state.clone();
    let timeout = gloo_timers::callback::Timeout::new(delay_ms, move || {
        do_connect(
            reconnect_state,
            reconnect_conn,
            &user_id,
            &workspace_id,
            &token,
        );
    });

    state.borrow_mut().reconnect_timeout = Some(SendWrapper::new(timeout));
}
