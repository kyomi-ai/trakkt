// SPDX-License-Identifier: AGPL-3.0-or-later

//! Browser transport and DOM subscriptions for session-owned terminals.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use trakkt_connect_protocol::{AgentMessage, ServerMessage};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{MessageEvent, WebSocket};

use super::{Grid, sessions::Sessions, tab_manager::SessionTab};

type BrowserCallback = Closure<dyn FnMut(JsValue)>;

pub struct Client {
    sessions: Sessions,
    closing: HashSet<String>,
    socket: Option<WebSocket>,
    socket_callbacks: Vec<BrowserCallback>,
    timeout: Option<gloo_timers::callback::Timeout>,
    scroll_timeout: Option<gloo_timers::callback::Timeout>,
    follow_output: bool,
    attempts: u32,
    closed: bool,
    connecting: bool,
    cols: u16,
    rows: u16,
    tabs: RwSignal<Vec<SessionTab>>,
    active: RwSignal<Option<String>>,
    grid: RwSignal<Grid>,
    browser_connected: RwSignal<bool>,
    agent_connected: RwSignal<bool>,
    error: RwSignal<Option<String>>,
}

impl Client {
    fn publish(&self) {
        self.tabs.set(self.sessions.tabs());
        self.active.set(self.sessions.active.clone());
        self.grid.set(self.sessions.grid());
    }

    fn send(&self, message: &ServerMessage) -> bool {
        let result = self
            .socket
            .as_ref()
            .filter(|socket| socket.ready_state() == WebSocket::OPEN)
            .ok_or_else(|| {
                "The terminal connection is unavailable. Your session output is retained."
                    .to_string()
            })
            .and_then(|socket| {
                serde_json::to_string(message)
                    .map_err(|error| error.to_string())
                    .and_then(|json| {
                        socket
                            .send_with_str(&json)
                            .map_err(|error| format!("Sending terminal command failed: {error:?}"))
                    })
            });
        if let Err(error) = result {
            self.error.set(Some(error));
            return false;
        }
        true
    }

    fn close_socket(&mut self) {
        if let Some(socket) = self.socket.take() {
            socket.set_onopen(None);
            socket.set_onmessage(None);
            socket.set_onerror(None);
            socket.set_onclose(None);
            if let Err(error) = socket.close() {
                tracing::warn!(?error, "Closing terminal WebSocket failed");
            }
        }
        self.socket_callbacks.clear();
    }

    fn receive(&mut self, message: AgentMessage) {
        match message {
            AgentMessage::AgentStatus { connected } => {
                self.agent_connected.set(connected);
                if !connected {
                    self.sessions.disconnected();
                }
            }
            AgentMessage::Ready { .. } => {
                // A newly reconnected agent may have sessions the browser has
                // never seen; ask for its authoritative list.
                self.send(&ServerMessage::ListSessions);
            }
            AgentMessage::SessionList { sessions } => {
                for id in self
                    .sessions
                    .restore_list(&sessions, self.agent_connected.get_untracked())
                {
                    self.send(&ServerMessage::ScrollbackRequest { session_id: id });
                }
                self.publish();
            }
            AgentMessage::SessionEvent { session_id, event } => {
                if let Some(error) = self.sessions.event(&session_id, &event) {
                    self.error.set(Some(error));
                }
                if matches!(
                    event,
                    trakkt_connect_protocol::SessionEventKind::Killed
                        | trakkt_connect_protocol::SessionEventKind::Exited { .. }
                ) && self.closing.remove(&session_id)
                {
                    self.sessions.remove(&session_id);
                }
                self.publish();
            }
            AgentMessage::SessionOutput { session_id, data } => {
                self.output(session_id, data, false)
            }
            AgentMessage::ScrollbackDump { session_id, data } => {
                self.output(session_id, data, true)
            }
            AgentMessage::Pong { .. } => {}
        }
    }

    fn output(&mut self, id: String, data: String, scrollback: bool) {
        let bytes = match B64.decode(data) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error.set(Some(format!(
                    "Terminal output could not be decoded: {error}"
                )));
                return;
            }
        };
        let responses = self.sessions.output(&id, &bytes, scrollback);
        // Device-status queries (used by fullscreen CLIs) need replies from the
        // live terminal, but replaying historical queries must not send input.
        if !scrollback && !responses.is_empty() {
            self.send(&ServerMessage::SessionInput {
                session_id: id.clone(),
                data: B64.encode(responses),
            });
        }
        if scrollback {
            if let Some(session) = self.sessions.entries.get_mut(&id) {
                session.grid.resize(self.cols as usize, self.rows as usize);
            }
            self.send(&ServerMessage::SessionResize {
                session_id: id,
                cols: self.cols,
                rows: self.rows,
            });
        }
        self.publish();
    }
}

fn reconnect(client: &Rc<RefCell<Client>>) {
    let mut state = client.borrow_mut();
    if state.closed {
        return;
    }
    state.connecting = false;
    state.browser_connected.set(false);
    state.agent_connected.set(false);
    state.sessions.disconnected();
    let delay = 1000u32
        .saturating_mul(2u32.saturating_pow(state.attempts))
        .min(30_000);
    state.attempts = state.attempts.saturating_add(1);
    let weak = Rc::downgrade(client);
    state.timeout = Some(gloo_timers::callback::Timeout::new(delay, move || {
        if let Some(client) = weak.upgrade() {
            connect(&client);
        }
    }));
}

fn connect(client: &Rc<RefCell<Client>>) {
    {
        let mut state = client.borrow_mut();
        if state.closed || state.connecting {
            return;
        }
        state.timeout = None;
        state.connecting = true;
    }
    let client = client.clone();
    leptos::task::spawn_local(async move {
        let token = crate::server_fns::auth::get_ws_token().await;
        if client.borrow().closed {
            return;
        }
        let token = match token {
            Ok(token) => token,
            Err(error) => {
                client
                    .borrow()
                    .error
                    .set(Some(format!("Terminal authentication failed: {error}")));
                reconnect(&client);
                return;
            }
        };
        let location = web_sys::window().expect("browser window").location();
        let url = location.host().and_then(|host| {
            location.protocol().map(|protocol| {
                let protocol = if protocol == "https:" { "wss" } else { "ws" };
                format!(
                    "{protocol}://{host}/ws/connect/terminal?token={}",
                    percent_encoding::utf8_percent_encode(
                        &token,
                        percent_encoding::NON_ALPHANUMERIC
                    )
                )
            })
        });
        let socket = match url.and_then(|url| WebSocket::new(&url)) {
            Ok(socket) => socket,
            Err(error) => {
                client
                    .borrow()
                    .error
                    .set(Some(format!("Terminal connection failed: {error:?}")));
                reconnect(&client);
                return;
            }
        };
        let weak = Rc::downgrade(&client);
        let open = callback(&weak, |client, _| {
            let mut state = client.borrow_mut();
            state.connecting = false;
            state.attempts = 0;
            state.browser_connected.set(true);
            state.error.set(None);
            state.send(&ServerMessage::ListSessions);
        });
        let message = callback(&weak, |client, value| {
            let event: MessageEvent = value.unchecked_into();
            if let Some(text) = event.data().as_string() {
                match serde_json::from_str(&text) {
                    Ok(message) => client.borrow_mut().receive(message),
                    Err(error) => client
                        .borrow()
                        .error
                        .set(Some(format!("Invalid terminal message: {error}"))),
                }
            }
        });
        let error = callback(&weak, |client, _| {
            client.borrow().error.set(Some(
                "Terminal connection interrupted. Reconnecting…".into(),
            ));
        });
        let close = callback(&weak, |client, _| reconnect(client));
        socket.set_onopen(Some(open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
        socket.set_onerror(Some(error.as_ref().unchecked_ref()));
        socket.set_onclose(Some(close.as_ref().unchecked_ref()));
        let mut state = client.borrow_mut();
        state.close_socket();
        state.socket = Some(socket);
        state.socket_callbacks = vec![open, message, error, close];
    });
}

fn callback(
    weak: &Weak<RefCell<Client>>,
    handler: impl Fn(&Rc<RefCell<Client>>, JsValue) + 'static,
) -> BrowserCallback {
    let weak = weak.clone();
    Closure::new(move |value| {
        if let Some(client) = weak.upgrade() {
            let closed = client.borrow().closed;
            if !closed {
                handler(&client, value);
            }
        }
    })
}

pub struct Actions {
    pub new_shell: Callback<()>,
    pub new_claude: Callback<()>,
    pub select: Callback<String>,
    pub close: Callback<String>,
}

pub fn start(
    tabs: RwSignal<Vec<SessionTab>>,
    active: RwSignal<Option<String>>,
    grid: RwSignal<Grid>,
    browser_connected: RwSignal<bool>,
    agent_connected: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    terminal_ref: NodeRef<leptos::html::Div>,
) -> Actions {
    let client = Rc::new(RefCell::new(Client {
        sessions: Sessions::default(),
        closing: HashSet::new(),
        socket: None,
        socket_callbacks: Vec::new(),
        timeout: None,
        scroll_timeout: None,
        follow_output: true,
        attempts: 0,
        closed: false,
        connecting: false,
        cols: 80,
        rows: 24,
        tabs,
        active,
        grid,
        browser_connected,
        agent_connected,
        error,
    }));
    let client_for_cleanup = SendWrapper::new(client.clone());
    on_cleanup(move || {
        let mut state = client_for_cleanup.borrow_mut();
        state.closed = true;
        state.timeout = None;
        state.scroll_timeout = None;
        state.close_socket();
    });
    wire_input(&client, terminal_ref);
    connect(&client);

    let new_session = |command: &'static str| {
        let client = SendWrapper::new(client.clone());
        Callback::new(move |()| {
            let mut state = client.borrow_mut();
            if !state.agent_connected.get_untracked() {
                state
                    .error
                    .set(Some("Connect an agent before starting a session.".into()));
                return;
            }
            let id = match web_sys::window().and_then(|window| window.crypto().ok()) {
                Some(crypto) => {
                    let mut bytes = [0u8; 16];
                    if let Err(error) = crypto.get_random_values_with_u8_array(&mut bytes) {
                        state
                            .error
                            .set(Some(format!("Generating a session ID failed: {error:?}")));
                        return;
                    }
                    bytes[6] = (bytes[6] & 0x0f) | 0x40;
                    bytes[8] = (bytes[8] & 0x3f) | 0x80;
                    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                    format!(
                        "{}-{}-{}-{}-{}",
                        &hex[..8],
                        &hex[8..12],
                        &hex[12..16],
                        &hex[16..20],
                        &hex[20..]
                    )
                }
                None => {
                    state
                        .error
                        .set(Some("Browser secure randomness is unavailable.".into()));
                    return;
                }
            };
            let (cols, rows) = (state.cols, state.rows);
            if !state.send(&ServerMessage::SpawnSession {
                session_id: id.clone(),
                command: vec![command.into()],
                working_dir: None,
                env: Default::default(),
                cols,
                rows,
            }) {
                return;
            }
            state.error.set(None);
            state.sessions.add(id, command.into(), cols, rows);
            state.follow_output = true;
            state.publish();
            if let Some(terminal) = terminal_ref.get()
                && let Err(error) = terminal.focus()
            {
                tracing::warn!(?error, "Focusing terminal failed");
            }
        })
    };
    let select_client = SendWrapper::new(client.clone());
    let close_client = SendWrapper::new(client.clone());
    Actions {
        new_shell: new_session("sh"),
        new_claude: new_session("claude"),
        select: Callback::new(move |id: String| {
            let mut state = select_client.borrow_mut();
            if state.sessions.entries.contains_key(&id) {
                state.sessions.active = Some(id);
                state.follow_output = true;
                state.publish();
                if let Some(terminal) = terminal_ref.get()
                    && let Err(error) = terminal.focus()
                {
                    tracing::warn!(?error, "Focusing terminal failed");
                }
            }
        }),
        close: Callback::new(move |id: String| {
            let mut state = close_client.borrow_mut();
            let running = state
                .sessions
                .entries
                .get(&id)
                .is_some_and(|session| session.running);
            if running && !state.agent_connected.get_untracked() {
                state.error.set(Some(
                    "Reconnect your agent before closing this live session. Its output remains available.".into(),
                ));
                return;
            }
            if running
                && !state.send(&ServerMessage::SessionKill {
                    session_id: id.clone(),
                    force: true,
                })
            {
                return;
            }
            if running {
                state.closing.insert(id);
            } else {
                state.sessions.remove(&id);
                state.publish();
            }
        }),
    }
}

fn wire_input(client: &Rc<RefCell<Client>>, terminal_ref: NodeRef<leptos::html::Div>) {
    let weak = Rc::downgrade(client);
    let key = callback(&weak, |client, value| {
        let event: web_sys::KeyboardEvent = value.unchecked_into();
        let state = client.borrow();
        let Some(id) = state.sessions.active.as_ref() else {
            return;
        };
        let Some(session) = state
            .sessions
            .entries
            .get(id)
            .filter(|session| session.running)
        else {
            return;
        };
        if let Some(bytes) =
            super::input::translate_key(&event, session.grid.modes.application_cursor_keys)
        {
            // Terminal control keys belong to the PTY, not global app shortcuts.
            event.stop_propagation();
            state.send(&ServerMessage::SessionInput {
                session_id: id.clone(),
                data: B64.encode(bytes),
            });
        }
    });
    let paste = callback(&weak, |client, value| {
        let event: web_sys::ClipboardEvent = value.unchecked_into();
        let state = client.borrow();
        let Some(id) = state.sessions.active.as_ref() else {
            return;
        };
        let Some(session) = state
            .sessions
            .entries
            .get(id)
            .filter(|session| session.running)
        else {
            return;
        };
        if let Some(data) = event.clipboard_data() {
            match data.get_data("text/plain") {
                Ok(text) => {
                    event.prevent_default();
                    let bytes = if session.grid.modes.bracketed_paste {
                        format!("\x1b[200~{text}\x1b[201~").into_bytes()
                    } else {
                        super::input::handle_paste(&text)
                    };
                    state.send(&ServerMessage::SessionInput {
                        session_id: id.clone(),
                        data: B64.encode(bytes),
                    });
                }
                Err(error) => state
                    .error
                    .set(Some(format!("Clipboard text unavailable: {error:?}"))),
            }
        }
    });
    let scroll = callback(&weak, |client, value| {
        let event: web_sys::Event = value.unchecked_into();
        if let Some(target) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
        {
            client.borrow_mut().follow_output =
                target.scroll_height() - target.client_height() - target.scroll_top() < 36;
        }
    });
    let resize_weak = weak.clone();
    let resize = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
        let Some(client) = resize_weak.upgrade() else {
            return;
        };
        if client.borrow().closed || entries.length() == 0 {
            return;
        }
        let entry: web_sys::ResizeObserverEntry = entries.get(0).unchecked_into();
        let rect = entry.content_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        // Renderer explicitly uses 14px monospace cells and 18px line height.
        let cols = ((rect.width() - 8.0) / 8.4).floor().clamp(2.0, 500.0) as u16;
        let rows = ((rect.height() - 8.0) / 18.0).floor().clamp(2.0, 200.0) as u16;
        let mut state = client.borrow_mut();
        if (state.cols, state.rows) == (cols, rows) {
            return;
        }
        state.cols = cols;
        state.rows = rows;
        for session in state.sessions.entries.values_mut() {
            session.grid.resize(cols as usize, rows as usize);
        }
        for (id, session) in &state.sessions.entries {
            if session.running {
                state.send(&ServerMessage::SessionResize {
                    session_id: id.clone(),
                    cols,
                    rows,
                });
            }
        }
        state.publish();
    });
    let callbacks = SendWrapper::new(Rc::new((key, paste, resize, scroll)));
    Effect::new(move |_| {
        let Some(element) = terminal_ref.get() else {
            return;
        };
        let element: web_sys::HtmlElement = (*element).clone().unchecked_into();
        for (name, callback) in [
            ("keydown", &callbacks.0),
            ("paste", &callbacks.1),
            ("scroll", &callbacks.3),
        ] {
            if let Err(error) =
                element.add_event_listener_with_callback(name, callback.as_ref().unchecked_ref())
            {
                tracing::warn!(?error, name, "Registering terminal listener failed");
            }
        }
        let observer = match web_sys::ResizeObserver::new(callbacks.2.as_ref().unchecked_ref()) {
            Ok(observer) => {
                observer.observe(&element);
                Some(observer)
            }
            Err(error) => {
                tracing::warn!(?error, "Observing terminal size failed");
                None
            }
        };
        let cleanup_callbacks = callbacks.clone();
        let cleanup_element = SendWrapper::new(element);
        let cleanup_observer = SendWrapper::new(observer);
        on_cleanup(move || {
            for (name, callback) in [
                ("keydown", &cleanup_callbacks.0),
                ("paste", &cleanup_callbacks.1),
                ("scroll", &cleanup_callbacks.3),
            ] {
                if let Err(error) = cleanup_element
                    .remove_event_listener_with_callback(name, callback.as_ref().unchecked_ref())
                {
                    tracing::warn!(?error, name, "Removing terminal listener failed");
                }
            }
            if let Some(observer) = cleanup_observer.as_ref() {
                observer.disconnect();
            }
        });
    });
    let scroll_client = SendWrapper::new(Rc::downgrade(client));
    let grid = client.borrow().grid;
    Effect::new(move |_| {
        grid.track();
        let Some(element) = terminal_ref.get() else {
            return;
        };
        if let Some(client) = scroll_client.upgrade() {
            if !client.borrow().follow_output {
                return;
            }
            let weak = Rc::downgrade(&client);
            client.borrow_mut().scroll_timeout =
                Some(gloo_timers::callback::Timeout::new(0, move || {
                    if let Some(client) = weak.upgrade()
                        && !client.borrow().closed
                        && client.borrow().follow_output
                    {
                        element.set_scroll_top(element.scroll_height());
                    }
                }));
        }
    });
}
