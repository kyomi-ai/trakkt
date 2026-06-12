// SPDX-License-Identifier: AGPL-3.0-or-later

//! ConnectManager — central registry for Connect agents, terminal sessions,
//! and browser subscribers.
//!
//! The server never executes commands; it is purely a relay. The manager
//! routes messages between agents and browsers:
//!
//! - **Agent registration**: agents connect via WebSocket and register here.
//! - **Session routing**: each session maps to an agent; browser input is
//!   forwarded to the owning agent.
//! - **Browser fan-out**: multiple browsers can watch the same session;
//!   agent output is broadcast to all subscribers.
//!
//! Pattern mirrors [`crate::websocket::WebSocketManager`] — `Arc<Inner>` with
//! `DashMap` for lock-free concurrent access.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;

/// Capacity of the bounded mpsc channel between the manager and each
/// connection's outbound task. Terminal output can be bursty, so we use
/// a generous buffer.
const CHANNEL_CAPACITY: usize = 2048;

/// Monotonically increasing ID for browser connection deduplication.
static NEXT_BROWSER_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

/// Generate a unique browser connection ID.
pub fn next_browser_connection_id() -> u64 {
    NEXT_BROWSER_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

/// A connected agent's send channel and metadata.
struct AgentConnection {
    workspace_id: String,
    user_id: String,
    sender: mpsc::Sender<String>,
}

/// A browser subscriber watching a terminal session.
struct BrowserConnection {
    connection_id: u64,
    sender: mpsc::Sender<String>,
}

/// Internal shared state behind `Arc` (cheap clones).
struct ConnectManagerInner {
    /// Connected agents: agent_id -> AgentConnection.
    agents: DashMap<String, AgentConnection>,
    /// Session routing: session_id -> agent_id.
    sessions: DashMap<String, String>,
    /// Browser connections watching sessions: session_id -> Vec<BrowserConnection>.
    browsers: DashMap<String, Vec<BrowserConnection>>,
}

/// Central registry for connected agents, active sessions, and browser
/// subscribers. Cheaply cloneable (inner `Arc`).
#[derive(Clone)]
pub struct ConnectManager {
    inner: Arc<ConnectManagerInner>,
}

impl ConnectManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ConnectManagerInner {
                agents: DashMap::new(),
                sessions: DashMap::new(),
                browsers: DashMap::new(),
            }),
        }
    }

    /// Register a connected agent.
    ///
    /// Returns an `mpsc::Receiver<String>` that the caller should drain and
    /// forward to the agent's WebSocket sink.
    pub fn register_agent(
        &self,
        agent_id: &str,
        workspace_id: &str,
        user_id: &str,
    ) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        self.inner.agents.insert(
            agent_id.to_string(),
            AgentConnection {
                workspace_id: workspace_id.to_string(),
                user_id: user_id.to_string(),
                sender: tx,
            },
        );
        tracing::info!(
            agent_id,
            workspace_id,
            user_id,
            "Connect agent registered"
        );
        rx
    }

    /// Unregister an agent and clean up all its sessions.
    ///
    /// Removes the agent from the registry and drops all session mappings
    /// that pointed to this agent.
    pub fn unregister_agent(&self, agent_id: &str) {
        self.inner.agents.remove(agent_id);

        // Collect session IDs owned by this agent, then remove them.
        let owned_sessions: Vec<String> = self
            .inner
            .sessions
            .iter()
            .filter(|entry| entry.value() == agent_id)
            .map(|entry| entry.key().clone())
            .collect();

        for session_id in &owned_sessions {
            self.inner.sessions.remove(session_id);
            // Notify any watching browsers that the session is gone by
            // dropping their entries (senders will close naturally).
            self.inner.browsers.remove(session_id);
        }

        tracing::info!(
            agent_id,
            sessions_cleaned = owned_sessions.len(),
            "Connect agent unregistered"
        );
    }

    /// Register a session-to-agent mapping.
    pub fn register_session(&self, session_id: &str, agent_id: &str) {
        self.inner
            .sessions
            .insert(session_id.to_string(), agent_id.to_string());
        tracing::debug!(session_id, agent_id, "Session registered");
    }

    /// Remove a session mapping and its browser subscribers.
    pub fn unregister_session(&self, session_id: &str) {
        self.inner.sessions.remove(session_id);
        self.inner.browsers.remove(session_id);
        tracing::debug!(session_id, "Session unregistered");
    }

    /// Subscribe a browser to session output.
    ///
    /// Returns an `mpsc::Receiver<String>` that the caller should drain and
    /// forward to the browser's WebSocket sink.
    pub fn subscribe_browser(&self, session_id: &str, connection_id: u64) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        self.inner
            .browsers
            .entry(session_id.to_string())
            .or_default()
            .push(BrowserConnection {
                connection_id,
                sender: tx,
            });
        tracing::debug!(session_id, connection_id, "Browser subscribed to session");
        rx
    }

    /// Unsubscribe a browser from session output.
    pub fn unsubscribe_browser(&self, session_id: &str, connection_id: u64) {
        if let Some(mut browsers) = self.inner.browsers.get_mut(session_id) {
            browsers.retain(|b| b.connection_id != connection_id);
        }
        // Clean up empty entries.
        self.inner
            .browsers
            .remove_if(session_id, |_, browsers| browsers.is_empty());
        tracing::debug!(session_id, connection_id, "Browser unsubscribed from session");
    }

    /// Route a message to the agent owning a session.
    ///
    /// Returns `true` if the message was queued, `false` if the session or
    /// agent is unknown or the agent's channel is full/closed.
    pub fn send_to_agent(&self, session_id: &str, message: &str) -> bool {
        let agent_id = match self.inner.sessions.get(session_id) {
            Some(entry) => entry.value().clone(),
            None => {
                tracing::warn!(session_id, "send_to_agent: unknown session");
                return false;
            }
        };

        match self.inner.agents.get(&agent_id) {
            Some(conn) => match conn.sender.try_send(message.to_string()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(agent_id, session_id, "Agent send buffer full, dropping message");
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::warn!(agent_id, session_id, "Agent channel closed");
                    false
                }
            },
            None => {
                tracing::warn!(agent_id, session_id, "send_to_agent: agent not found");
                false
            }
        }
    }

    /// Broadcast a message to all browser subscribers of a session.
    ///
    /// Uses `try_send` — if a browser's channel is full, the message is
    /// dropped for that browser (terminal output is best-effort for slow
    /// consumers). Stale (closed) connections are pruned.
    pub fn broadcast_to_browsers(&self, session_id: &str, message: &str) {
        let mut stale_ids: Vec<u64> = Vec::new();

        if let Some(browsers) = self.inner.browsers.get(session_id) {
            for browser in browsers.value().iter() {
                match browser.sender.try_send(message.to_string()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        stale_ids.push(browser.connection_id);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(
                            session_id,
                            connection_id = browser.connection_id,
                            "Browser send buffer full, dropping terminal output"
                        );
                    }
                }
            }
        }

        if !stale_ids.is_empty()
            && let Some(mut browsers) = self.inner.browsers.get_mut(session_id)
        {
            browsers.retain(|b| !stale_ids.contains(&b.connection_id));
        }
    }

    /// Get a clone of an agent's send channel.
    pub fn get_agent_sender(&self, agent_id: &str) -> Option<mpsc::Sender<String>> {
        self.inner
            .agents
            .get(agent_id)
            .map(|conn| conn.sender.clone())
    }

    /// Find any connected agent in the given workspace.
    ///
    /// Returns the `agent_id` of the first match. If multiple agents are
    /// connected for the same workspace, the selection is arbitrary.
    pub fn find_agent_for_workspace(&self, workspace_id: &str) -> Option<String> {
        self.inner
            .agents
            .iter()
            .find(|entry| entry.value().workspace_id == workspace_id)
            .map(|entry| entry.key().clone())
    }

    /// Get the workspace_id for a connected agent.
    pub fn get_agent_workspace(&self, agent_id: &str) -> Option<String> {
        self.inner
            .agents
            .get(agent_id)
            .map(|conn| conn.workspace_id.clone())
    }

    /// Get the user_id for a connected agent.
    pub fn get_agent_user(&self, agent_id: &str) -> Option<String> {
        self.inner
            .agents
            .get(agent_id)
            .map(|conn| conn.user_id.clone())
    }

    /// Get the workspace_id for a session by resolving through the agent.
    ///
    /// Returns `None` if the session or its agent is unknown.
    pub fn get_session_workspace(&self, session_id: &str) -> Option<String> {
        let agent_id = self.inner.sessions.get(session_id)?.value().clone();
        self.get_agent_workspace(&agent_id)
    }
}

impl Default for ConnectManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ConnectManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectManager")
            .field("agents", &self.inner.agents.len())
            .field("sessions", &self.inner.sessions.len())
            .field(
                "browser_subscriptions",
                &self
                    .inner
                    .browsers
                    .iter()
                    .map(|e| e.value().len())
                    .sum::<usize>(),
            )
            .finish()
    }
}
