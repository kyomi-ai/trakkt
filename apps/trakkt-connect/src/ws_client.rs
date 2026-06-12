// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket client for bidirectional communication with the Trakkt server.
//!
//! Uses a concurrent reader/writer architecture:
//! - **Writer task**: owns the WebSocket sink, drains outbound messages from two
//!   sources: an `mpsc::Receiver<AgentMessage>` (from PtyManager) that gets
//!   serialized to JSON, and WebSocket-level control frames (pongs).
//! - **Reader loop**: receives [`ServerMessage`]s and dispatches them to the
//!   [`PtyManager`] via [`pty_manager::dispatch`].
//!
//! The client reconnects automatically with exponential backoff (1s to 60s).
//! PTY sessions survive reconnects because they are local processes. The agent
//! message channel (`mpsc::Receiver<AgentMessage>`) is owned by `run_forever`
//! and persists across reconnects, so no output is lost during brief
//! disconnections (messages queue in the channel).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

use trakkt_connect_protocol::{AgentMessage, ServerMessage};

use crate::pty_manager::{self, PtyManager};

/// WebSocket client that maintains a persistent connection to the Trakkt server.
pub struct WsClient {
    url: String,
    token: String,
}

impl WsClient {
    pub fn new(url: String, token: String) -> Self {
        Self { url, token }
    }

    /// Main loop: connect, process messages, reconnect on drop.
    /// Never returns -- runs until the process exits.
    ///
    /// - `agent_rx`: receives `AgentMessage`s from the `PtyManager` and forwards
    ///   them as JSON text frames over the WebSocket.
    /// - `agent_tx`: used to send protocol-level responses (pong, session list,
    ///   scrollback dump) back through the same channel.
    pub async fn run_forever(
        &self,
        ws_connected: Arc<AtomicBool>,
        pty_manager: Arc<PtyManager>,
        agent_tx: mpsc::Sender<AgentMessage>,
        mut agent_rx: mpsc::Receiver<AgentMessage>,
    ) -> ! {
        let mut backoff = Duration::from_secs(1);

        loop {
            match self.connect().await {
                Ok((ws_sender, ws_receiver)) => {
                    backoff = Duration::from_secs(1);
                    ws_connected.store(true, Ordering::Relaxed);
                    tracing::info!("Connected to Trakkt server");

                    // Send Ready message
                    let ready = AgentMessage::Ready {
                        agent_version: env!("CARGO_PKG_VERSION").to_string(),
                        hostname: hostname(),
                        os: std::env::consts::OS.to_string(),
                    };
                    if let Err(e) = agent_tx.send(ready).await {
                        tracing::warn!(error = %e, "Failed to queue Ready message");
                    }

                    agent_rx = self
                        .run_session(
                            ws_sender,
                            ws_receiver,
                            &pty_manager,
                            &agent_tx,
                            agent_rx,
                        )
                        .await;

                    ws_connected.store(false, Ordering::Relaxed);
                    tracing::warn!("Disconnected from Trakkt server, reconnecting...");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        delay_secs = backoff.as_secs(),
                        "Failed to connect, retrying..."
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    /// Establish WebSocket connection with Authorization header.
    async fn connect(
        &self,
    ) -> anyhow::Result<(
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tungstenite::Message,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    )> {
        let request = http::Request::builder()
            .uri(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Host", extract_host(&self.url).unwrap_or("localhost"))
            .body(())
            .map_err(|e| anyhow::anyhow!("Failed to build request: {e}"))?;

        let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| anyhow::anyhow!("WebSocket connection failed: {e}"))?;

        Ok(ws_stream.split())
    }

    /// Run a single WebSocket session with concurrent reader/writer tasks.
    ///
    /// Returns the `agent_rx` back to the caller so it can be reused across
    /// reconnects. This ensures no messages are lost during brief disconnections.
    async fn run_session(
        &self,
        ws_sender: futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tungstenite::Message,
        >,
        mut ws_receiver: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        pty_manager: &Arc<PtyManager>,
        agent_tx: &mpsc::Sender<AgentMessage>,
        mut agent_rx: mpsc::Receiver<AgentMessage>,
    ) -> mpsc::Receiver<AgentMessage> {
        /// If no message (ping, command, anything) arrives within this duration,
        /// assume the connection is dead. The server sends pings every 30s, so
        /// 60s means two consecutive pings were missed.
        const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);

        // Channel for outbound WebSocket frames (text + pong).
        // Both the agent message forwarder and the reader loop (for pongs) send
        // through this channel; the writer task drains it.
        let (ws_tx, ws_rx) = mpsc::channel::<tungstenite::Message>(64);

        // Spawn the writer task (owns the WebSocket sink)
        let writer_handle = tokio::spawn(writer_task(ws_sender, ws_rx));

        // Spawn the agent message forwarder: drains agent_rx (AgentMessages
        // from PtyManager) and serializes them into WebSocket text frames.
        let ws_tx_agent = ws_tx.clone();
        let (agent_rx_return_tx, agent_rx_return_rx) =
            tokio::sync::oneshot::channel::<mpsc::Receiver<AgentMessage>>();

        let forwarder_handle = tokio::spawn(async move {
            loop {
                match agent_rx.recv().await {
                    Some(msg) => {
                        let json = match serde_json::to_string(&msg) {
                            Ok(j) => j,
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to serialize AgentMessage");
                                continue;
                            }
                        };
                        if ws_tx_agent
                            .send(tungstenite::Message::Text(json.into()))
                            .await
                            .is_err()
                        {
                            // Writer closed — return agent_rx so it can be reused
                            let _ = agent_rx_return_tx.send(agent_rx);
                            return;
                        }
                    }
                    None => {
                        // agent_tx was dropped — this shouldn't happen in normal operation
                        let _ = agent_rx_return_tx.send(agent_rx);
                        return;
                    }
                }
            }
        });

        // Reader loop: deserialize ServerMessages and dispatch to PtyManager
        loop {
            let msg = match tokio::time::timeout(SILENCE_TIMEOUT, ws_receiver.next()).await {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    // Stream ended (server closed cleanly)
                    break;
                }
                Err(_) => {
                    tracing::warn!(
                        timeout_secs = SILENCE_TIMEOUT.as_secs(),
                        "No message from server in {SILENCE_TIMEOUT:?}, assuming connection dead"
                    );
                    break;
                }
            };

            match msg {
                Ok(tungstenite::Message::Text(text)) => {
                    let server_msg: ServerMessage = match serde_json::from_str(&text) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to parse ServerMessage");
                            continue;
                        }
                    };

                    pty_manager::dispatch(pty_manager, server_msg, agent_tx).await;
                }
                Ok(tungstenite::Message::Ping(data)) => {
                    if ws_tx
                        .send(tungstenite::Message::Pong(data))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(tungstenite::Message::Close(_)) => {
                    tracing::info!("Server sent close frame");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "WebSocket error");
                    break;
                }
            }
        }

        // Drop ws_tx so the forwarder's ws_tx_agent.send() fails and it
        // returns agent_rx via the oneshot naturally. No abort needed.
        drop(ws_tx);

        // Wait for the forwarder to exit and return agent_rx
        let agent_rx = match agent_rx_return_rx.await {
            Ok(rx) => rx,
            Err(_) => {
                // Should not happen — the forwarder always sends agent_rx back
                // before exiting. If it does, we have a bug.
                tracing::error!("Forwarder exited without returning agent_rx — this is a bug");
                unreachable!("forwarder must return agent_rx via oneshot before exiting");
            }
        };

        // Wait for both tasks to finish
        let _ = forwarder_handle.await;
        let _ = writer_handle.await;

        agent_rx
    }
}

/// Writer task: drains the mpsc channel and sends messages over the WebSocket.
async fn writer_task(
    mut ws_sender: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tungstenite::Message,
    >,
    mut rx: mpsc::Receiver<tungstenite::Message>,
) {
    while let Some(msg) = rx.recv().await {
        if let Err(e) = ws_sender.send(msg).await {
            tracing::warn!(error = %e, "Writer: failed to send, closing");
            break;
        }
    }
}

/// Extract host from a URL string.
fn extract_host(url: &str) -> Option<&str> {
    url.strip_prefix("wss://")
        .or_else(|| url.strip_prefix("ws://"))
        .and_then(|rest| rest.split('/').next())
}

/// Get the system hostname.
fn hostname() -> String {
    gethostname().unwrap_or_else(|| "unknown".to_string())
}

/// Platform-specific hostname retrieval.
fn gethostname() -> Option<String> {
    #[cfg(unix)]
    {
        nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_wss() {
        assert_eq!(
            extract_host("wss://app.trakkt.dev/api/connect/ws"),
            Some("app.trakkt.dev")
        );
    }

    #[test]
    fn extract_host_ws() {
        assert_eq!(
            extract_host("ws://localhost:3100/api/connect/ws"),
            Some("localhost:3100")
        );
    }

    #[test]
    fn extract_host_no_scheme() {
        assert_eq!(extract_host("app.trakkt.dev/api/connect/ws"), None);
    }

    #[test]
    fn hostname_returns_something() {
        let h = hostname();
        assert!(!h.is_empty());
    }
}
