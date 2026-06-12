// SPDX-License-Identifier: AGPL-3.0-or-later

//! Connect page — browser-based terminal for Trakkt Connect agent sessions.
//!
//! Opens a WebSocket to the server's `/ws/connect/terminal` endpoint and
//! relays PTY I/O between the browser terminal emulator and the agent.
//! Multiple sessions are supported via a tab bar.

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::components::terminal::renderer::TerminalRenderer;
use crate::components::terminal::tab_manager::{SessionTab, TabBar};

/// Default terminal dimensions used before the first resize observation.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

#[cfg(target_arch = "wasm32")]
const CELL_WIDTH_PX: f64 = 8.4;
#[cfg(target_arch = "wasm32")]
const CELL_HEIGHT_PX: f64 = 18.0;

/// Connect page — terminal UI for Trakkt Connect agent sessions.
#[component]
pub fn ConnectPage() -> impl IntoView {
    // -- Reactive state -------------------------------------------------------
    let tabs: RwSignal<Vec<SessionTab>> = RwSignal::new(Vec::new());
    let active_session: RwSignal<Option<String>> = RwSignal::new(None);
    let agent_connected: RwSignal<bool> = RwSignal::new(false);
    let grid_signal: RwSignal<crate::components::terminal::Grid> =
        RwSignal::new(crate::components::terminal::Grid::new(
            DEFAULT_COLS as usize,
            DEFAULT_ROWS as usize,
        ));

    // NodeRef for the terminal container (resize observation + focus).
    let terminal_ref = NodeRef::<leptos::html::Div>::new();

    // -- Client-only WebSocket + input wiring ---------------------------------
    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::rc::Rc;

        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;
        use web_sys::{CloseEvent, MessageEvent, WebSocket};

        use trakkt_connect_protocol::{AgentMessage, ServerMessage, SessionEventKind};

        // Per-session terminal state: each session owns a Grid + vte::Parser.
        type SessionState = HashMap<String, (crate::components::terminal::Grid, vte::Parser)>;
        let session_grids: Rc<RefCell<SessionState>> = Rc::new(RefCell::new(HashMap::new()));

        // WebSocket handle — shared across event closures.
        let ws_handle: Rc<RefCell<Option<WebSocket>>> = Rc::new(RefCell::new(None));
        // Stored closures — prevent GC.
        let ws_closures: Rc<RefCell<Vec<JsValue>>> = Rc::new(RefCell::new(Vec::new()));
        // Current terminal dimensions (updated by ResizeObserver).
        let current_cols: Rc<RefCell<u16>> = Rc::new(RefCell::new(DEFAULT_COLS));
        let current_rows: Rc<RefCell<u16>> = Rc::new(RefCell::new(DEFAULT_ROWS));
        // Intentional close flag.
        let intentional_close: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
        // Reconnect timeout handle.
        let reconnect_timeout: Rc<RefCell<Option<SendWrapper<gloo_timers::callback::Timeout>>>> =
            Rc::new(RefCell::new(None));
        let reconnect_attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));

        // ── Helper: send a ServerMessage through the WebSocket ──────────────
        let ws_for_send = ws_handle.clone();
        let send_msg = Rc::new(move |msg: &ServerMessage| {
            let ws_ref = ws_for_send.borrow();
            if let Some(ref ws) = *ws_ref {
                if ws.ready_state() == WebSocket::OPEN {
                    if let Ok(json) = serde_json::to_string(msg) {
                        let _ = ws.send_with_str(&json);
                    }
                }
            }
        });

        // ── Helper: sync active session's grid into the signal ──────────────
        let grids_for_sync = session_grids.clone();
        let sync_grid = Rc::new(move |session_id: &str, grid_signal: RwSignal<crate::components::terminal::Grid>| {
            let grids = grids_for_sync.borrow();
            if let Some((grid, _)) = grids.get(session_id) {
                // We need to clone the grid data into the signal. Since Grid
                // doesn't implement Clone (it contains HashSet, Vec<Vec<Cell>>),
                // we build a fresh Grid and copy the cell data row by row.
                let cols = grid.cols;
                let rows = grid.rows;
                let mut new_grid = crate::components::terminal::Grid::new(cols, rows);
                // Copy cells.
                for r in 0..rows {
                    for c in 0..cols {
                        new_grid.cells[r][c] = grid.cells[r][c].clone();
                    }
                }
                // Copy cursor state.
                new_grid.cursor.row = grid.cursor.row;
                new_grid.cursor.col = grid.cursor.col;
                // Copy modes.
                new_grid.modes.cursor_visible = grid.modes.cursor_visible;
                new_grid.modes.application_cursor_keys = grid.modes.application_cursor_keys;
                new_grid.modes.auto_wrap = grid.modes.auto_wrap;
                new_grid.modes.origin_mode = grid.modes.origin_mode;
                new_grid.modes.insert_mode = grid.modes.insert_mode;
                new_grid.modes.alternate_screen = grid.modes.alternate_screen;
                new_grid.modes.bracketed_paste = grid.modes.bracketed_paste;
                grid_signal.set(new_grid);
            }
        });

        // ── Build WS URL ────────────────────────────────────────────────────
        fn build_connect_ws_url(token: &str) -> Result<String, String> {
            let window = web_sys::window().ok_or("no window")?;
            let location = window.location();
            let protocol = location.protocol().map_err(|_| "no protocol")?;
            let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
            let host = location.host().map_err(|_| "no host")?;
            Ok(format!(
                "{ws_protocol}//{host}/ws/connect/terminal?token={token}"
            ))
        }

        // ── Connect function ────────────────────────────────────────────────
        let ws_for_connect = ws_handle.clone();
        let closures_for_connect = ws_closures.clone();
        let grids_for_connect = session_grids.clone();
        let sync_for_connect = sync_grid.clone();
        let intentional_for_connect = intentional_close.clone();
        let reconnect_timeout_for_connect = reconnect_timeout.clone();
        let reconnect_attempts_for_connect = reconnect_attempts.clone();

        let do_connect: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let do_connect_inner = do_connect.clone();

        let connect_fn = Rc::new({
            let ws_for_connect = ws_for_connect.clone();
            let closures_for_connect = closures_for_connect.clone();
            let grids_for_connect = grids_for_connect.clone();
            let sync_for_connect = sync_for_connect.clone();
            let intentional_for_connect = intentional_for_connect.clone();
            let reconnect_timeout_for_connect = reconnect_timeout_for_connect.clone();
            let reconnect_attempts_for_connect = reconnect_attempts_for_connect.clone();
            let do_connect_for_reconnect = do_connect.clone();
            let send_msg_for_connect = send_msg.clone();

            move || {
                // Mark as non-intentional close for this new attempt.
                *intentional_for_connect.borrow_mut() = false;

                let ws_handle = ws_for_connect.clone();
                let closures_store = closures_for_connect.clone();
                let grids = grids_for_connect.clone();
                let sync = sync_for_connect.clone();
                let intentional = intentional_for_connect.clone();
                let reconnect_to = reconnect_timeout_for_connect.clone();
                let reconnect_att = reconnect_attempts_for_connect.clone();
                let do_connect_ref = do_connect_for_reconnect.clone();
                let send_msg_inner = send_msg_for_connect.clone();

                leptos::task::spawn_local(async move {
                    // Fetch JWT token.
                    let token = match crate::server_fns::auth::get_ws_token().await {
                        Ok(t) if !t.is_empty() => t,
                        _ => {
                            web_sys::console::warn_1(
                                &"[trakkt-connect] Failed to fetch WS token".into(),
                            );
                            agent_connected.set(false);
                            // Schedule reconnect.
                            schedule_reconnect(
                                reconnect_att.clone(),
                                reconnect_to.clone(),
                                intentional.clone(),
                                do_connect_ref.clone(),
                                agent_connected,
                            );
                            return;
                        }
                    };

                    let url = match build_connect_ws_url(&token) {
                        Ok(u) => u,
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("[trakkt-connect] WS URL error: {e}").into(),
                            );
                            return;
                        }
                    };

                    // Close any existing connection.
                    {
                        let mut ws_ref = ws_handle.borrow_mut();
                        if let Some(ref ws) = *ws_ref {
                            ws.set_onclose(None);
                            ws.set_onerror(None);
                            ws.set_onmessage(None);
                            ws.set_onopen(None);
                            let _ = ws.close();
                        }
                        *ws_ref = None;
                        closures_store.borrow_mut().clear();
                    }

                    let ws = match WebSocket::new(&url) {
                        Ok(ws) => ws,
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("[trakkt-connect] WebSocket::new failed: {e:?}").into(),
                            );
                            schedule_reconnect(
                                reconnect_att,
                                reconnect_to,
                                intentional,
                                do_connect_ref,
                                agent_connected,
                            );
                            return;
                        }
                    };

                    let mut closures: Vec<JsValue> = Vec::new();

                    // -- onopen ---------------------------------------------------
                    let on_open = Closure::<dyn FnMut(JsValue)>::new({
                        let reconnect_att = reconnect_att.clone();
                        let send_on_open = send_msg_inner.clone();
                        move |_: JsValue| {
                            web_sys::console::log_1(
                                &"[trakkt-connect] WebSocket connected".into(),
                            );
                            agent_connected.set(true);
                            *reconnect_att.borrow_mut() = 0;
                            send_on_open(&ServerMessage::ListSessions);
                        }
                    });
                    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
                    closures.push(on_open.into_js_value());

                    // -- onmessage ------------------------------------------------
                    let on_message = Closure::<dyn FnMut(MessageEvent)>::new({
                        let grids = grids.clone();
                        let sync = sync.clone();
                        move |event: MessageEvent| {
                            let Some(text) = event.data().as_string() else {
                                return;
                            };
                            let msg: AgentMessage = match serde_json::from_str(&text) {
                                Ok(m) => m,
                                Err(e) => {
                                    web_sys::console::warn_1(
                                        &format!("[trakkt-connect] parse error: {e}").into(),
                                    );
                                    return;
                                }
                            };

                            match msg {
                                AgentMessage::SessionOutput { session_id, data } => {
                                    // Base64-decode the PTY output.
                                    let bytes = match B64.decode(&data) {
                                        Ok(b) => b,
                                        Err(_) => return,
                                    };
                                    // Feed through VTE parser into this session's Grid.
                                    let mut grids_ref = grids.borrow_mut();
                                    if let Some((grid, parser)) = grids_ref.get_mut(&session_id) {
                                        for &byte in &bytes {
                                            let mut handler =
                                                crate::components::terminal::TerminalHandler::new(
                                                    grid,
                                                );
                                            parser.advance(&mut handler, byte);
                                        }
                                        // If this is the active session, sync to the signal.
                                        if active_session.get_untracked().as_deref()
                                            == Some(session_id.as_str())
                                        {
                                            sync(&session_id, grid_signal);
                                        }
                                    }
                                }
                                AgentMessage::SessionEvent { session_id, event } => {
                                    match event {
                                        SessionEventKind::Started => {
                                            // Session is running — no-op, already tracked.
                                        }
                                        SessionEventKind::Exited { .. }
                                        | SessionEventKind::Killed
                                        | SessionEventKind::SpawnFailed { .. } => {
                                            // Remove session.
                                            grids.borrow_mut().remove(&session_id);
                                            tabs.update(|t| {
                                                t.retain(|tab| tab.session_id != session_id);
                                            });
                                            // If this was the active session, switch to another.
                                            if active_session.get_untracked().as_deref()
                                                == Some(session_id.as_str())
                                            {
                                                let new_active = tabs
                                                    .get_untracked()
                                                    .first()
                                                    .map(|t| t.session_id.clone());
                                                active_session.set(new_active.clone());
                                                if let Some(ref id) = new_active {
                                                    sync(id, grid_signal);
                                                }
                                            }
                                        }
                                    }
                                }
                                AgentMessage::Ready { .. } => {
                                    agent_connected.set(true);
                                }
                                AgentMessage::SessionList { sessions } => {
                                    let new_tabs: Vec<SessionTab> = sessions
                                        .iter()
                                        .map(|s| SessionTab {
                                            session_id: s.session_id.clone(),
                                            label: s
                                                .command
                                                .first()
                                                .cloned()
                                                .unwrap_or_else(|| "session".into()),
                                        })
                                        .collect();
                                    tabs.set(new_tabs);
                                    // Ensure each session has a Grid.
                                    let mut grids_ref = grids.borrow_mut();
                                    for s in &sessions {
                                        grids_ref
                                            .entry(s.session_id.clone())
                                            .or_insert_with(|| {
                                                (
                                                    crate::components::terminal::Grid::new(
                                                        s.cols as usize,
                                                        s.rows as usize,
                                                    ),
                                                    vte::Parser::new(),
                                                )
                                            });
                                    }
                                    // Set active to first if none active.
                                    if active_session.get_untracked().is_none() {
                                        if let Some(first) = sessions.first() {
                                            active_session.set(Some(first.session_id.clone()));
                                            sync(&first.session_id, grid_signal);
                                        }
                                    }
                                }
                                AgentMessage::Pong { .. } | AgentMessage::ScrollbackDump { .. } => {
                                    // Ignored for now.
                                }
                            }
                        }
                    });
                    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
                    closures.push(on_message.into_js_value());

                    // -- onerror --------------------------------------------------
                    let on_error = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
                        web_sys::console::error_1(
                            &format!("[trakkt-connect] WS error: {e:?}").into(),
                        );
                        agent_connected.set(false);
                    });
                    ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
                    closures.push(on_error.into_js_value());

                    // -- onclose --------------------------------------------------
                    let on_close = Closure::<dyn FnMut(CloseEvent)>::new({
                        let intentional = intentional.clone();
                        let reconnect_att = reconnect_att.clone();
                        let reconnect_to = reconnect_to.clone();
                        let do_connect_ref = do_connect_ref.clone();
                        move |event: CloseEvent| {
                            web_sys::console::log_1(
                                &format!(
                                    "[trakkt-connect] WS closed code={} reason={}",
                                    event.code(),
                                    event.reason()
                                )
                                .into(),
                            );
                            agent_connected.set(false);
                            if !*intentional.borrow() {
                                schedule_reconnect(
                                    reconnect_att.clone(),
                                    reconnect_to.clone(),
                                    intentional.clone(),
                                    do_connect_ref.clone(),
                                    agent_connected,
                                );
                            }
                        }
                    });
                    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
                    closures.push(on_close.into_js_value());

                    // Store.
                    *ws_handle.borrow_mut() = Some(ws);
                    *closures_store.borrow_mut() = closures;
                });
            }
        });

        // Store the connect function for reconnect callbacks.
        *do_connect_inner.borrow_mut() = Some(connect_fn.clone());

        // ── Schedule reconnect helper (module-level fn) ─────────────────────
        fn schedule_reconnect(
            attempts: Rc<RefCell<u32>>,
            timeout_handle: Rc<RefCell<Option<SendWrapper<gloo_timers::callback::Timeout>>>>,
            intentional: Rc<RefCell<bool>>,
            connect_fn: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
            agent_connected: RwSignal<bool>,
        ) {
            if *intentional.borrow() {
                return;
            }
            let att = *attempts.borrow();
            let delay_ms =
                std::cmp::min(1000u32.saturating_mul(2u32.saturating_pow(att)), 30_000u32);
            *attempts.borrow_mut() = att + 1;
            agent_connected.set(false);

            let timeout = gloo_timers::callback::Timeout::new(delay_ms, move || {
                if let Some(ref f) = *connect_fn.borrow() {
                    f();
                }
            });
            *timeout_handle.borrow_mut() = Some(SendWrapper::new(timeout));
        }

        // ── Initial connect ─────────────────────────────────────────────────
        connect_fn();

        // ── Cleanup on unmount ──────────────────────────────────────────────
        let ws_for_cleanup = SendWrapper::new(ws_handle.clone());
        let closures_for_cleanup = SendWrapper::new(ws_closures.clone());
        let intentional_for_cleanup = SendWrapper::new(intentional_close.clone());
        let timeout_for_cleanup = SendWrapper::new(reconnect_timeout.clone());
        on_cleanup(move || {
            *intentional_for_cleanup.borrow_mut() = true;
            *timeout_for_cleanup.borrow_mut() = None;
            let mut ws_ref = ws_for_cleanup.borrow_mut();
            if let Some(ref ws) = *ws_ref {
                ws.set_onclose(None);
                ws.set_onerror(None);
                ws.set_onmessage(None);
                ws.set_onopen(None);
                let _ = ws.close();
            }
            *ws_ref = None;
            closures_for_cleanup.borrow_mut().clear();
        });

        // ── Keyboard input handler ──────────────────────────────────────────
        let send_for_key = send_msg.clone();
        let grids_for_key = session_grids.clone();
        let keydown_handler = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let Some(session_id) = active_session.get_untracked() else {
                return;
            };

            // Read application_cursor_keys from the session's grid.
            let app_cursor = grids_for_key
                .borrow()
                .get(&session_id)
                .map(|(g, _)| g.modes.application_cursor_keys)
                .unwrap_or(false);

            if let Some(bytes) = crate::components::terminal::input::translate_key(&ev, app_cursor) {
                let data = B64.encode(&bytes);
                send_for_key(&ServerMessage::SessionInput {
                    session_id,
                    data,
                });
            }
        });

        // ── Paste handler ───────────────────────────────────────────────────
        let send_for_paste = send_msg.clone();
        let grids_for_paste = session_grids.clone();
        let paste_handler = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
            let Some(session_id) = active_session.get_untracked() else {
                return;
            };
            let ev: web_sys::ClipboardEvent = ev.unchecked_into();
            if let Some(dt) = ev.clipboard_data() {
                if let Ok(text) = dt.get_data("text/plain") {
                    ev.prevent_default();

                    let bracketed = grids_for_paste
                        .borrow()
                        .get(&session_id)
                        .map(|(g, _)| g.modes.bracketed_paste)
                        .unwrap_or(false);

                    let mut bytes = Vec::new();
                    if bracketed {
                        bytes.extend_from_slice(b"\x1b[200~");
                    }
                    bytes.extend_from_slice(&crate::components::terminal::input::handle_paste(&text));
                    if bracketed {
                        bytes.extend_from_slice(b"\x1b[201~");
                    }

                    let data = B64.encode(&bytes);
                    send_for_paste(&ServerMessage::SessionInput {
                        session_id,
                        data,
                    });
                }
            }
        });

        // ── Attach keyboard + paste to terminal container ───────────────────
        // Pattern from layout.rs: wrap closures in SendWrapper, register
        // cleanup inside the Effect so both closures stay in the same scope.
        let keydown_wrapper = SendWrapper::new(keydown_handler);
        let paste_wrapper = SendWrapper::new(paste_handler);

        Effect::new(move |_| {
            let Some(el) = terminal_ref.get() else {
                return;
            };
            let el: &web_sys::HtmlElement = &el;

            let _ = el.add_event_listener_with_callback(
                "keydown",
                keydown_wrapper.as_ref().unchecked_ref(),
            );
            let _ = el.add_event_listener_with_callback(
                "paste",
                paste_wrapper.as_ref().unchecked_ref(),
            );

            let kd = SendWrapper::new(keydown_wrapper.as_ref().unchecked_ref::<js_sys::Function>().clone());
            let pa = SendWrapper::new(paste_wrapper.as_ref().unchecked_ref::<js_sys::Function>().clone());
            on_cleanup(move || {
                if let Some(el) = terminal_ref.get() {
                    let el: &web_sys::HtmlElement = &el;
                    let _ = el.remove_event_listener_with_callback("keydown", &kd);
                    let _ = el.remove_event_listener_with_callback("paste", &pa);
                }
            });
        });

        // ── ResizeObserver ──────────────────────────────────────────────────
        let send_for_resize = send_msg.clone();
        let cols_for_resize = current_cols.clone();
        let rows_for_resize = current_rows.clone();
        let grids_for_resize = session_grids.clone();
        let sync_for_resize = sync_grid.clone();

        let resize_cb = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let entry: web_sys::ResizeObserverEntry = entries.get(0).unchecked_into();
            let content_rect = entry.content_rect();
            let width = content_rect.width();
            let height = content_rect.height();

            if width <= 0.0 || height <= 0.0 {
                return;
            }

            let new_cols = (width / CELL_WIDTH_PX).floor().max(1.0) as u16;
            let new_rows = (height / CELL_HEIGHT_PX).floor().max(1.0) as u16;

            let prev_cols = *cols_for_resize.borrow();
            let prev_rows = *rows_for_resize.borrow();

            if new_cols == prev_cols && new_rows == prev_rows {
                return;
            }

            *cols_for_resize.borrow_mut() = new_cols;
            *rows_for_resize.borrow_mut() = new_rows;

            // Resize all session grids.
            let mut grids_ref = grids_for_resize.borrow_mut();
            for (_, (grid, _)) in grids_ref.iter_mut() {
                grid.resize(new_cols as usize, new_rows as usize);
            }

            // Sync the active session grid to the signal.
            if let Some(ref id) = active_session.get_untracked() {
                sync_for_resize(id, grid_signal);
            }

            // Send resize to all active sessions.
            let session_ids: Vec<String> = grids_ref.keys().cloned().collect();
            drop(grids_ref);
            for session_id in session_ids {
                send_for_resize(&ServerMessage::SessionResize {
                    session_id,
                    cols: new_cols,
                    rows: new_rows,
                });
            }
        });

        let resize_cb_wrapper = SendWrapper::new(resize_cb);
        let observer_handle: Rc<RefCell<Option<SendWrapper<web_sys::ResizeObserver>>>> =
            Rc::new(RefCell::new(None));

        let observer_for_effect = observer_handle.clone();
        Effect::new(move |_| {
            let Some(el) = terminal_ref.get() else {
                return;
            };
            let el: &web_sys::HtmlElement = &el;
            if let Ok(observer) =
                web_sys::ResizeObserver::new(resize_cb_wrapper.as_ref().unchecked_ref())
            {
                observer.observe(el);
                *observer_for_effect.borrow_mut() = Some(SendWrapper::new(observer));
            }
        });

        let observer_for_cleanup = SendWrapper::new(observer_handle.clone());
        on_cleanup(move || {
            if let Some(obs) = observer_for_cleanup.borrow_mut().take() {
                obs.disconnect();
            }
        });

        // ── Tab callbacks ───────────────────────────────────────────────────
        let send_for_new = SendWrapper::new(send_msg.clone());
        let grids_for_new = SendWrapper::new(session_grids.clone());
        let cols_for_new = SendWrapper::new(current_cols.clone());
        let rows_for_new = SendWrapper::new(current_rows.clone());
        let sync_for_new = SendWrapper::new(sync_grid.clone());

        let on_new_session = Callback::new(move |()| {
            let cols = *cols_for_new.borrow();
            let rows = *rows_for_new.borrow();
            let session_id = format!(
                "sess-{}-{:.0}",
                js_sys::Date::now() as u64,
                js_sys::Math::random() * 1_000_000.0,
            );

            // Send spawn request.
            send_for_new(&ServerMessage::SpawnSession {
                session_id: session_id.clone(),
                command: vec!["claude".into()],
                working_dir: None,
                env: std::collections::HashMap::new(),
                cols,
                rows,
            });

            // Create local grid.
            grids_for_new.borrow_mut().insert(
                session_id.clone(),
                (
                    crate::components::terminal::Grid::new(cols as usize, rows as usize),
                    vte::Parser::new(),
                ),
            );

            // Add tab and set active.
            tabs.update(|t| {
                t.push(SessionTab {
                    session_id: session_id.clone(),
                    label: "claude".into(),
                });
            });
            active_session.set(Some(session_id.clone()));
            sync_for_new(&session_id, grid_signal);

            // Focus the terminal.
            if let Some(el) = terminal_ref.get() {
                let _ = el.focus();
            }
        });

        let send_for_close = SendWrapper::new(send_msg.clone());
        let grids_for_close = SendWrapper::new(session_grids.clone());
        let sync_for_close = SendWrapper::new(sync_grid.clone());

        let on_close_session = Callback::new(move |session_id: String| {
            // Send kill.
            send_for_close(&ServerMessage::SessionKill {
                session_id: session_id.clone(),
                force: false,
            });

            // Remove locally.
            grids_for_close.borrow_mut().remove(&session_id);
            tabs.update(|t| {
                t.retain(|tab| tab.session_id != session_id);
            });

            // Switch active if needed.
            if active_session.get_untracked().as_deref() == Some(session_id.as_str()) {
                let new_active = tabs.get_untracked().first().map(|t| t.session_id.clone());
                active_session.set(new_active.clone());
                if let Some(ref id) = new_active {
                    sync_for_close(id, grid_signal);
                }
            }
        });

        let sync_for_select = SendWrapper::new(sync_grid.clone());
        let on_select_session = Callback::new(move |session_id: String| {
            active_session.set(Some(session_id.clone()));
            sync_for_select(&session_id, grid_signal);

            // Focus the terminal.
            if let Some(el) = terminal_ref.get() {
                let _ = el.focus();
            }
        });

        // ── View ────────────────────────────────────────────────────────────
        let tabs_signal = Signal::derive(move || tabs.get());
        let active_signal = Signal::derive(move || active_session.get());
        let has_sessions = Signal::derive(move || !tabs.get().is_empty());

        view! {
            <div class="flex flex-col h-full">
                // Page header
                <div class="flex items-center justify-between px-6 py-3 border-b border-border bg-background">
                    <h1 class="text-lg font-semibold text-foreground">"Connect"</h1>
                    <div class="flex items-center gap-2 text-sm">
                        <span
                            class=move || {
                                if agent_connected.get() {
                                    "inline-block w-2 h-2 rounded-full bg-[color:var(--color-success-foreground)]"
                                } else {
                                    "inline-block w-2 h-2 rounded-full bg-[color:var(--color-error-foreground)]"
                                }
                            }
                        />
                        <span class="text-muted-foreground">
                            {move || {
                                if agent_connected.get() {
                                    "Agent connected"
                                } else {
                                    "Agent disconnected"
                                }
                            }}
                        </span>
                    </div>
                </div>

                // Tab bar
                <TabBar
                    tabs=tabs_signal
                    active_session=active_signal
                    on_select=on_select_session
                    on_new=on_new_session
                    on_close=on_close_session
                />

                // Terminal viewport
                <div
                    node_ref=terminal_ref
                    tabindex="0"
                    class="flex-1 overflow-hidden outline-none"
                    style="background-color: #1e1e1e;"
                >
                    <Show
                        when=move || has_sessions.get()
                        fallback=|| view! {
                            <div class="flex items-center justify-center h-full">
                                <p class="text-muted-foreground text-sm">
                                    "No active sessions. Click + to start a new Claude session."
                                </p>
                            </div>
                        }
                    >
                        <TerminalRenderer grid=grid_signal/>
                    </Show>
                </div>
            </div>
        }
        .into_any()
    }

    // ── SSR fallback (no terminal on server-side render) ─────────────────
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = &grid_signal;
        let tabs_signal = Signal::derive(move || tabs.get());
        let active_signal = Signal::derive(move || active_session.get());

        view! {
            <div class="flex flex-col h-full">
                <div class="flex items-center justify-between px-6 py-3 border-b border-border bg-background">
                    <h1 class="text-lg font-semibold text-foreground">"Connect"</h1>
                    <div class="flex items-center gap-2 text-sm">
                        <span
                            class=move || {
                                if agent_connected.get() {
                                    "inline-block w-2 h-2 rounded-full bg-[color:var(--color-success-foreground)]"
                                } else {
                                    "inline-block w-2 h-2 rounded-full bg-[color:var(--color-error-foreground)]"
                                }
                            }
                        />
                        <span class="text-muted-foreground">
                            {move || {
                                if agent_connected.get() {
                                    "Agent connected"
                                } else {
                                    "Agent disconnected"
                                }
                            }}
                        </span>
                    </div>
                </div>
                <TabBar
                    tabs=tabs_signal
                    active_session=active_signal
                    on_select=Callback::new(|_: String| {})
                    on_new=Callback::new(|()| {})
                    on_close=Callback::new(|_: String| {})
                />
                <div
                    node_ref=terminal_ref
                    tabindex="0"
                    class="flex-1 overflow-hidden outline-none"
                    style="background-color: #1e1e1e;"
                >
                    <div class="flex items-center justify-center h-full">
                        <p class="text-muted-foreground text-sm">
                            "No active sessions. Click + to start a new Claude session."
                        </p>
                    </div>
                </div>
            </div>
        }
        .into_any()
    }
}
