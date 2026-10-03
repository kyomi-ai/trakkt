// SPDX-License-Identifier: AGPL-3.0-or-later
//! Atomic, owner-scoped relay registry. No OS commands execute on the server.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::mpsc;
use trakkt_connect_protocol::{AgentMessage, ServerMessage, SessionEventKind, SessionInfo};

const CHANNEL_CAPACITY: usize = 64;
const MAX_SESSIONS_PER_AGENT: usize = 32;
static NEXT_BROWSER_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);
pub fn next_browser_connection_id() -> u64 {
    NEXT_BROWSER_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}
#[derive(Clone, PartialEq, Eq)]
struct Owner {
    workspace: String,
    user: String,
}
struct Agent {
    owner: Owner,
    sender: mpsc::Sender<String>,
}
struct Browser {
    owner: Owner,
    sender: mpsc::Sender<String>,
    sessions: HashSet<String>,
}
#[derive(Default)]
struct Registry {
    agents: HashMap<String, Agent>,
    sessions: HashMap<String, String>,
    infos: HashMap<String, SessionInfo>,
    browsers: HashMap<u64, Browser>,
}
#[derive(Clone, Default)]
pub struct ConnectManager {
    inner: Arc<Mutex<Registry>>,
}
impl ConnectManager {
    pub fn new() -> Self {
        Self::default()
    }
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn register_agent(&self, id: &str, workspace: &str, user: &str) -> mpsc::Receiver<String> {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let owner = Owner {
            workspace: workspace.into(),
            user: user.into(),
        };
        let mut r = self.registry();
        r.agents.insert(
            id.into(),
            Agent {
                owner: owner.clone(),
                sender,
            },
        );
        Self::notify_owner(
            &mut r,
            &owner,
            &AgentMessage::AgentStatus { connected: true },
        );
        receiver
    }
    pub fn unregister_agent(&self, id: &str) {
        let mut r = self.registry();
        if let Some(agent) = r.agents.remove(id) {
            let removed: HashSet<_> = r
                .sessions
                .iter()
                .filter(|(_, agent_id)| *agent_id == id)
                .map(|(session, _)| session.clone())
                .collect();
            r.sessions.retain(|_, agent_id| agent_id != id);
            r.infos.retain(|session, _| !removed.contains(session));
            for browser in r.browsers.values_mut() {
                browser
                    .sessions
                    .retain(|session| !removed.contains(session));
            }
            let connected = r.agents.values().any(|a| a.owner == agent.owner);
            Self::notify_owner(
                &mut r,
                &agent.owner,
                &AgentMessage::AgentStatus { connected },
            );
            let sessions = Self::owner_sessions(&r, &agent.owner);
            Self::notify_owner(
                &mut r,
                &agent.owner,
                &AgentMessage::SessionList { sessions },
            );
        }
    }
    /// One bounded outbound channel per browser; disconnect drops every subscription.
    pub fn register_browser(&self, id: u64, workspace: &str, user: &str) -> mpsc::Receiver<String> {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let owner = Owner {
            workspace: workspace.into(),
            user: user.into(),
        };
        let mut r = self.registry();
        let connected = r.agents.values().any(|a| a.owner == owner);
        Self::send(&sender, &AgentMessage::AgentStatus { connected });
        r.browsers.insert(
            id,
            Browser {
                owner,
                sender,
                sessions: HashSet::new(),
            },
        );
        receiver
    }
    pub fn unregister_browser(&self, id: u64) {
        self.registry().browsers.remove(&id);
    }
    fn send(sender: &mpsc::Sender<String>, msg: &AgentMessage) -> bool {
        match serde_json::to_string(msg) {
            Ok(json) => sender.try_send(json).is_ok(),
            Err(error) => {
                tracing::warn!(%error, "Connect message serialization failed");
                false
            }
        }
    }
    fn notify_owner(r: &mut Registry, owner: &Owner, msg: &AgentMessage) {
        r.browsers
            .retain(|_, browser| browser.owner != *owner || Self::send(&browser.sender, msg));
    }
    /// Authorization, session reservation and queueing share one lock. A duplicate
    /// ID never replaces a live session, even under concurrent spawn requests.
    pub fn browser_command(&self, id: u64, msg: &ServerMessage) -> Result<(), &'static str> {
        let mut r = self.registry();
        let owner = r
            .browsers
            .get(&id)
            .ok_or("Browser disconnected")?
            .owner
            .clone();
        let json = serde_json::to_string(msg).map_err(|_| "Invalid command")?;
        match msg {
            ServerMessage::SpawnSession {
                session_id,
                command,
                working_dir,
                env,
                cols,
                rows,
            } => {
                if command.is_empty()
                    || command.len() > 16
                    || command.iter().map(String::len).sum::<usize>() > 1024
                    || working_dir.as_ref().is_some_and(|path| path.len() > 1024)
                    || env.len() > 128
                    || env
                        .iter()
                        .map(|(key, value)| key.len() + value.len())
                        .sum::<usize>()
                        > 16 * 1024
                    || *cols == 0
                    || *rows == 0
                    || *cols > 500
                    || *rows > 500
                {
                    return Err("Invalid command or terminal dimensions");
                }
                if session_id.is_empty() || session_id.len() > 128 {
                    return Err("Invalid session ID");
                }
                if r.sessions.contains_key(session_id) {
                    return Err("Session ID already exists");
                }
                let agent_id = r
                    .agents
                    .iter()
                    .find(|(_, a)| a.owner == owner)
                    .map(|(id, _)| id.clone())
                    .ok_or("No agent connected")?;
                if r.sessions.values().filter(|a| **a == agent_id).count() >= MAX_SESSIONS_PER_AGENT
                {
                    return Err("Session limit reached");
                }
                r.agents[&agent_id]
                    .sender
                    .try_send(json)
                    .map_err(|_| "Agent unavailable or busy")?;
                r.sessions.insert(session_id.clone(), agent_id);
                if let Some(browser) = r.browsers.get_mut(&id) {
                    browser.sessions.insert(session_id.clone());
                }
            }
            ServerMessage::SessionInput { session_id, .. }
            | ServerMessage::SessionResize { session_id, .. }
            | ServerMessage::SessionKill { session_id, .. }
            | ServerMessage::ScrollbackRequest { session_id } => {
                let agent_id = r.sessions.get(session_id).ok_or("Unknown session")?.clone();
                let agent = r.agents.get(&agent_id).ok_or("Agent disconnected")?;
                if agent.owner != owner {
                    return Err("Session access denied");
                }
                agent
                    .sender
                    .try_send(json)
                    .map_err(|_| "Agent unavailable or busy")?;
                if matches!(msg, ServerMessage::ScrollbackRequest { .. })
                    && let Some(browser) = r.browsers.get_mut(&id)
                {
                    browser.sessions.insert(session_id.clone());
                }
            }
            ServerMessage::ListSessions => {
                let mut sent = false;
                for agent in r.agents.values().filter(|a| a.owner == owner) {
                    agent
                        .sender
                        .try_send(json.clone())
                        .map_err(|_| "Agent unavailable or busy")?;
                    sent = true;
                }
                if !sent && let Some(browser) = r.browsers.get(&id) {
                    Self::send(
                        &browser.sender,
                        &AgentMessage::SessionList { sessions: vec![] },
                    );
                }
            }
            ServerMessage::Ping { .. } => {}
        }
        Ok(())
    }
    /// Agent events cannot forge another agent's output, exit, or routing table.
    pub fn agent_message(&self, id: &str, msg: &AgentMessage) {
        let mut r = self.registry();
        let Some(owner) = r.agents.get(id).map(|a| a.owner.clone()) else {
            return;
        };
        match msg {
            AgentMessage::SessionList { sessions } => {
                let mut accepted: Vec<SessionInfo> = Vec::new();
                for info in sessions.iter().take(MAX_SESSIONS_PER_AGENT) {
                    if info.session_id.is_empty() || info.session_id.len() > 128 {
                        continue;
                    }
                    if r.sessions
                        .get(&info.session_id)
                        .is_some_and(|agent| agent != id)
                    {
                        continue;
                    }
                    r.sessions.insert(info.session_id.clone(), id.into());
                    r.infos.insert(info.session_id.clone(), info.clone());
                    accepted.push(info.clone());
                }
                let ids: HashSet<_> = accepted.iter().map(|s| s.session_id.as_str()).collect();
                // A list requested before Spawn may arrive after its reservation.
                // Only prune sessions previously acknowledged by a list; pending
                // commands are completed by Started/SpawnFailed/exit events.
                let acknowledged: HashSet<_> = r.infos.keys().cloned().collect();
                r.sessions.retain(|session, agent| {
                    agent != id || ids.contains(session.as_str()) || !acknowledged.contains(session)
                });
                let valid: HashSet<_> = r.sessions.keys().cloned().collect();
                r.infos.retain(|session, _| valid.contains(session));
                for browser in r.browsers.values_mut() {
                    browser.sessions.retain(|session| valid.contains(session));
                }
                let sessions = Self::owner_sessions(&r, &owner);
                Self::notify_owner(&mut r, &owner, &AgentMessage::SessionList { sessions });
            }
            AgentMessage::SessionOutput { session_id, .. }
            | AgentMessage::ScrollbackDump { session_id, .. }
            | AgentMessage::SessionEvent { session_id, .. } => {
                if r.sessions.get(session_id).is_none_or(|agent| agent != id) {
                    return;
                }
                r.browsers.retain(|_, b| {
                    b.owner != owner
                        || !b.sessions.contains(session_id)
                        || Self::send(&b.sender, msg)
                });
                if matches!(
                    msg,
                    AgentMessage::SessionEvent {
                        event: SessionEventKind::Exited { .. }
                            | SessionEventKind::Killed
                            | SessionEventKind::SpawnFailed { .. },
                        ..
                    }
                ) {
                    r.sessions.remove(session_id);
                    r.infos.remove(session_id);
                    for browser in r.browsers.values_mut() {
                        browser.sessions.remove(session_id);
                    }
                }
            }
            AgentMessage::Ready { .. } => Self::notify_owner(&mut r, &owner, msg),
            AgentMessage::Pong { .. } | AgentMessage::AgentStatus { .. } => {}
        }
    }
    fn owner_sessions(r: &Registry, owner: &Owner) -> Vec<SessionInfo> {
        r.infos
            .values()
            .filter(|info| {
                r.sessions
                    .get(&info.session_id)
                    .and_then(|agent| r.agents.get(agent))
                    .is_some_and(|agent| agent.owner == *owner)
            })
            .cloned()
            .collect()
    }
    pub fn browser_error(&self, id: u64, session_id: &str, error: &str) {
        let r = self.registry();
        if let Some(b) = r.browsers.get(&id) {
            Self::send(
                &b.sender,
                &AgentMessage::SessionEvent {
                    session_id: session_id.into(),
                    event: SessionEventKind::SpawnFailed {
                        error: error.into(),
                    },
                },
            );
        }
    }
}
impl std::fmt::Debug for ConnectManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = self.registry();
        f.debug_struct("ConnectManager")
            .field("agents", &r.agents.len())
            .field("sessions", &r.sessions.len())
            .field("browsers", &r.browsers.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spawn(id: &str) -> ServerMessage {
        ServerMessage::SpawnSession {
            session_id: id.into(),
            command: vec!["sh".into()],
            working_dir: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        }
    }
    fn info(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            command: vec!["sh".into()],
            working_dir: None,
            started_at: "fixture".into(),
            cols: 80,
            rows: 24,
            pid: 42,
        }
    }
    #[test]
    fn concurrent_duplicate_reservation_has_exactly_one_winner() {
        let m = ConnectManager::new();
        let _a = m.register_agent("a", "w", "u");
        let _b1 = m.register_browser(1, "w", "u");
        let _b2 = m.register_browser(2, "w", "u");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (1..=2)
            .map(|id| {
                let manager = m.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    manager.browser_command(id, &spawn("same")).is_ok()
                })
            })
            .collect();
        let count = handles
            .into_iter()
            .map(|h| usize::from(h.join().expect("reservation thread")))
            .sum::<usize>();
        assert_eq!(count, 1);
        assert_eq!(m.registry().sessions.len(), 1);
    }
    #[test]
    fn lists_and_presence_are_owner_scoped_and_exit_cleans_subscriptions() {
        let m = ConnectManager::new();
        let _a = m.register_agent("a", "w", "u");
        let mut owner = m.register_browser(1, "w", "u");
        let mut other = m.register_browser(2, "w", "other");
        assert!(owner.try_recv().expect("presence").contains("true"));
        assert!(other.try_recv().expect("presence").contains("false"));
        assert!(m.browser_command(1, &spawn("s")).is_ok());
        m.agent_message(
            "a",
            &AgentMessage::SessionList {
                sessions: vec![info("s")],
            },
        );
        assert!(owner.try_recv().expect("list").contains("\"s\""));
        assert!(other.try_recv().is_err());
        m.agent_message(
            "a",
            &AgentMessage::SessionEvent {
                session_id: "s".into(),
                event: SessionEventKind::Exited { exit_code: 0 },
            },
        );
        assert!(m.registry().sessions.is_empty());
        assert!(m.registry().browsers[&1].sessions.is_empty());
        m.unregister_browser(1);
        assert!(!m.registry().browsers.contains_key(&1));
    }
    #[test]
    fn earlier_list_response_does_not_erase_pending_spawn() {
        let m = ConnectManager::new();
        let mut agent = m.register_agent("a", "w", "u");
        let _browser = m.register_browser(1, "w", "u");
        assert!(m.browser_command(1, &ServerMessage::ListSessions).is_ok());
        assert!(m.browser_command(1, &spawn("pending")).is_ok());
        assert!(
            agent
                .try_recv()
                .expect("first command")
                .contains("list_sessions")
        );
        assert!(
            agent
                .try_recv()
                .expect("second command")
                .contains("spawn_session")
        );
        m.agent_message("a", &AgentMessage::SessionList { sessions: vec![] });
        assert!(
            m.browser_command(
                1,
                &ServerMessage::SessionInput {
                    session_id: "pending".into(),
                    data: "YQ==".into()
                }
            )
            .is_ok()
        );
    }
    #[test]
    fn duplicate_spawn_does_not_reassign_existing_owner() {
        let m = ConnectManager::new();
        let mut a = m.register_agent("a", "w1", "u1");
        let _b = m.register_agent("b", "w2", "u2");
        let _one = m.register_browser(1, "w1", "u1");
        let _two = m.register_browser(2, "w2", "u2");
        assert!(m.browser_command(1, &spawn("s")).is_ok());
        assert!(m.browser_command(2, &spawn("s")).is_err());
        assert_eq!(
            m.registry().sessions.get("s").map(String::as_str),
            Some("a")
        );
        assert!(a.try_recv().is_ok());
        assert!(a.try_recv().is_err());
    }
    #[test]
    fn same_workspace_other_user_and_other_workspace_cannot_control_session() {
        let m = ConnectManager::new();
        let mut agent = m.register_agent("a", "w1", "u1");
        let _one = m.register_browser(1, "w1", "u1");
        let _two = m.register_browser(2, "w1", "u2");
        let _three = m.register_browser(3, "w2", "u1");
        assert!(m.browser_command(1, &spawn("s")).is_ok());
        assert!(agent.try_recv().is_ok());
        for browser in [2, 3] {
            for command in [
                ServerMessage::SessionInput {
                    session_id: "s".into(),
                    data: "YQ==".into(),
                },
                ServerMessage::SessionResize {
                    session_id: "s".into(),
                    cols: 1,
                    rows: 1,
                },
                ServerMessage::SessionKill {
                    session_id: "s".into(),
                    force: true,
                },
                ServerMessage::ScrollbackRequest {
                    session_id: "s".into(),
                },
            ] {
                assert!(m.browser_command(browser, &command).is_err());
            }
        }
        assert!(agent.try_recv().is_err());
    }
    #[test]
    fn forged_agent_events_and_lists_cannot_modify_or_observe_other_sessions() {
        let m = ConnectManager::new();
        let _a = m.register_agent("a", "w1", "u1");
        let _b = m.register_agent("b", "w2", "u2");
        let mut browser = m.register_browser(1, "w1", "u1");
        assert!(browser.try_recv().is_ok());
        assert!(m.browser_command(1, &spawn("s")).is_ok());
        for msg in [
            AgentMessage::SessionOutput {
                session_id: "s".into(),
                data: "forged".into(),
            },
            AgentMessage::ScrollbackDump {
                session_id: "s".into(),
                data: "forged".into(),
            },
            AgentMessage::SessionEvent {
                session_id: "s".into(),
                event: SessionEventKind::Killed,
            },
            AgentMessage::SessionList {
                sessions: vec![info("s")],
            },
        ] {
            m.agent_message("b", &msg);
        }
        assert!(browser.try_recv().is_err());
        assert_eq!(
            m.registry().sessions.get("s").map(String::as_str),
            Some("a")
        );
        m.agent_message(
            "a",
            &AgentMessage::SessionOutput {
                session_id: "s".into(),
                data: "valid".into(),
            },
        );
        assert!(browser.try_recv().expect("owner output").contains("valid"));
    }
    #[test]
    fn no_agent_or_closed_queue_leaves_no_session_reservation() {
        let m = ConnectManager::new();
        let _browser = m.register_browser(1, "w", "u");
        assert!(m.browser_command(1, &spawn("s")).is_err());
        let rx = m.register_agent("a", "w", "u");
        drop(rx);
        assert!(m.browser_command(1, &spawn("s")).is_err());
        assert!(m.registry().sessions.is_empty());
    }
    #[test]
    fn reconnect_and_server_restart_recover_routes_and_owner_lists() {
        let m = ConnectManager::new();
        let _a = m.register_agent("old", "w", "u");
        let mut browser = m.register_browser(1, "w", "u");
        assert!(browser.try_recv().is_ok());
        assert!(m.browser_command(1, &spawn("s")).is_ok());
        m.unregister_agent("old");
        assert!(browser.try_recv().expect("presence").contains("false"));
        assert!(
            browser
                .try_recv()
                .expect("empty list")
                .contains("session_list")
        );
        let _b = m.register_agent("new", "w", "u");
        assert!(browser.try_recv().expect("presence").contains("true"));
        m.agent_message(
            "new",
            &AgentMessage::SessionList {
                sessions: vec![info("s")],
            },
        );
        assert!(
            browser
                .try_recv()
                .expect("recovered list")
                .contains("\"s\"")
        );
        assert!(
            m.browser_command(
                1,
                &ServerMessage::ScrollbackRequest {
                    session_id: "s".into()
                }
            )
            .is_ok()
        );
        let fresh = ConnectManager::new();
        let _a = fresh.register_agent("new", "w", "u");
        m.agent_message(
            "old",
            &AgentMessage::SessionEvent {
                session_id: "s".into(),
                event: SessionEventKind::Killed,
            },
        );
        assert!(m.registry().sessions.contains_key("s"));
        fresh.agent_message(
            "new",
            &AgentMessage::SessionList {
                sessions: vec![info("s")],
            },
        );
        let _browser = fresh.register_browser(1, "w", "u");
        assert!(
            fresh
                .browser_command(
                    1,
                    &ServerMessage::ScrollbackRequest {
                        session_id: "s".into()
                    }
                )
                .is_ok()
        );
    }
}
