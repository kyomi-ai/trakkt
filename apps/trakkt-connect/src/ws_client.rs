// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket client for bidirectional communication with the Trakkt server.
//!
//! Uses a concurrent reader/writer architecture:
//! - **Writer**: owns the WebSocket sink, drains outbound messages from two
//!   sources: an `mpsc::Receiver<AgentMessage>` (from PtyManager) that gets
//!   serialized to JSON, and WebSocket-level control frames (pongs).
//! - **Reader loop**: receives [`ServerMessage`]s and dispatches them to the
//!   [`PtyManager`] via [`pty_manager::dispatch`].
//!
//! The client reconnects automatically with exponential backoff (1s to 60s).
//! PTY sessions survive reconnects because they are local processes. The agent
//! message channel (`mpsc::Receiver<AgentMessage>`) is owned by `run_forever`
//! and persists across reconnects, so no output is lost during brief
//! disconnections up to the bounded channel capacity; scrollback supports recovery.

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
                    let connected_at = tokio::time::Instant::now();
                    ws_connected.store(true, Ordering::Relaxed);
                    tracing::info!("Connected to Trakkt server");

                    agent_rx = self
                        .run_session(ws_sender, ws_receiver, &pty_manager, &agent_tx, agent_rx)
                        .await;

                    ws_connected.store(false, Ordering::Relaxed);
                    // Authentication can reject a token after a successful upgrade.
                    // Only a sustained connection resets retries; every disconnect
                    // waits, so revoked credentials cannot cause a tight retry loop.
                    if connected_at.elapsed() >= Duration::from_secs(30) {
                        backoff = Duration::from_secs(1);
                    }
                    tracing::warn!(
                        delay_secs = backoff.as_secs(),
                        "Disconnected from Trakkt server, reconnecting..."
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        delay_secs = backoff.as_secs(),
                        "Failed to connect, retrying..."
                    );
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
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
        use tungstenite::client::IntoClientRequest;
        let mut request = self.url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {}", self.token).parse()?);

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
        const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);
        // Announce and restore routing BEFORE replaying buffered PTY events. The
        // processes live on the agent, including across a server restart.
        let bootstrap = [
            AgentMessage::Ready {
                agent_version: env!("CARGO_PKG_VERSION").into(),
                hostname: hostname(),
                os: std::env::consts::OS.into(),
            },
            AgentMessage::SessionList {
                sessions: pty_manager.list_sessions().await,
            },
        ];
        let mut ws_sender = ws_sender;
        for msg in bootstrap {
            if !send_message(&mut ws_sender, &msg).await {
                return agent_rx;
            }
        }
        let pty_manager = Arc::clone(pty_manager);
        let agent_tx = agent_tx.clone();
        let mut reader = tokio::spawn(async move {
            loop {
                match tokio::time::timeout(SILENCE_TIMEOUT, ws_receiver.next()).await {
                    Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(msg) => pty_manager::dispatch(&pty_manager, msg, &agent_tx).await,
                            Err(error) => tracing::warn!(%error, "Invalid Connect server message"),
                        }
                    }
                    Ok(Some(Ok(tungstenite::Message::Close(_))))
                    | Ok(None)
                    | Err(_)
                    | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(_))) => {}
                }
            }
        });
        let _reader_guard = AbortReader(reader.abort_handle());
        loop {
            tokio::select! {
                _ = &mut reader => { return agent_rx; },
                outgoing = agent_rx.recv() => {
                    let Some(msg) = outgoing else { break; };
                    if !send_message(&mut ws_sender, &msg).await { break; }
                }
            }
        }
        reader.abort();
        if let Err(error) = reader.await
            && !error.is_cancelled()
        {
            tracing::warn!(%error, "Connect reader failed");
        }
        agent_rx
    }
}

struct AbortReader(tokio::task::AbortHandle);
impl Drop for AbortReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn send_message<S>(sink: &mut S, msg: &AgentMessage) -> bool
where
    S: futures_util::Sink<tungstenite::Message> + Unpin,
{
    match serde_json::to_string(msg) {
        Ok(json) => matches!(
            tokio::time::timeout(
                Duration::from_secs(10),
                sink.send(tungstenite::Message::Text(json.into()))
            )
            .await,
            Ok(Ok(()))
        ),
        Err(error) => {
            tracing::warn!(%error, "Connect serialization failed");
            false
        }
    }
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

    #[tokio::test]
    async fn real_websocket_reconnect_restores_existing_pty_before_queued_output() {
        use crate::pty_manager::PtyConfig;
        use std::collections::HashMap;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let address = listener.local_addr().expect("loopback address");
        let (tx, rx) = mpsc::channel(64);
        let manager = Arc::new(PtyManager::new(
            tx.clone(),
            PtyConfig {
                working_dir: std::env::temp_dir(),
                allowed_commands: vec!["sh".into()],
                scrollback_size: 65536,
            },
        ));
        manager
            .spawn(
                "recovered".into(),
                vec![
                    "sh".into(),
                    "-c".into(),
                    "printf reconnect-marker; read line".into(),
                ],
                None,
                HashMap::new(),
                80,
                24,
            )
            .await;
        let client = WsClient::new(
            format!("ws://{address}/ws/connect/agent"),
            "fixture-not-secret".into(),
        );
        let manager_for_client = Arc::clone(&manager);
        let client_task = tokio::spawn(async move {
            client
                .run_forever(Arc::new(AtomicBool::new(false)), manager_for_client, tx, rx)
                .await
        });
        let trial = tokio::time::timeout(Duration::from_secs(10), async {
            for connection in 0..2 {
                let (socket, _) = listener.accept().await.expect("agent connects");
                let mut ws = tokio_tungstenite::accept_async(socket)
                    .await
                    .expect("WebSocket handshake");
                let first = ws
                    .next()
                    .await
                    .expect("Ready frame")
                    .expect("Ready transport");
                assert!(matches!(
                    serde_json::from_str::<AgentMessage>(first.to_text().expect("Ready text"))
                        .expect("Ready JSON"),
                    AgentMessage::Ready { .. }
                ));
                let second = ws
                    .next()
                    .await
                    .expect("SessionList frame")
                    .expect("SessionList transport");
                match serde_json::from_str::<AgentMessage>(second.to_text().expect("List text"))
                    .expect("List JSON")
                {
                    AgentMessage::SessionList { sessions } => {
                        assert!(sessions.iter().any(|s| s.session_id == "recovered"))
                    }
                    other => panic!("bootstrap list must precede queued events: {other:?}"),
                }
                if connection == 0 {
                    ws.send(tungstenite::Message::Text(
                        serde_json::to_string(&ServerMessage::ScrollbackRequest {
                            session_id: "recovered".into(),
                        })
                        .expect("command JSON")
                        .into(),
                    ))
                    .await
                    .expect("request scrollback");
                    loop {
                        let message = ws
                            .next()
                            .await
                            .expect("scrollback frame")
                            .expect("scrollback transport");
                        if let AgentMessage::ScrollbackDump { .. } =
                            serde_json::from_str(message.to_text().expect("event text"))
                                .expect("event JSON")
                        {
                            break;
                        }
                    }
                }
                ws.close(None).await.expect("close connection");
            }
        })
        .await;
        client_task.abort();
        manager.shutdown().await;
        trial.expect("two socket reconnects complete");
        assert!(manager.list_sessions().await.is_empty());
    }

    #[tokio::test]
    async fn successful_upgrades_rejected_by_auth_use_exponential_retry_delay() {
        use crate::pty_manager::PtyConfig;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind rejection fixture");
        let address = listener.local_addr().expect("fixture address");
        let (tx, rx) = mpsc::channel(8);
        let manager = Arc::new(PtyManager::new(
            tx.clone(),
            PtyConfig {
                working_dir: std::env::temp_dir(),
                allowed_commands: vec!["sh".into()],
                scrollback_size: 65536,
            },
        ));
        let client = WsClient::new(
            format!("ws://{address}/ws/connect/agent"),
            "revoked-fixture".into(),
        );
        let task = tokio::spawn(async move {
            client
                .run_forever(Arc::new(AtomicBool::new(false)), manager, tx, rx)
                .await
        });
        let trial = tokio::time::timeout(Duration::from_secs(8), async {
            let mut previous = None;
            for attempt in 0..3 {
                let (socket, _) = listener.accept().await.expect("retry connects");
                let now = tokio::time::Instant::now();
                if let Some(last) = previous {
                    let minimum = if attempt == 1 { 900 } else { 1900 };
                    assert!(
                        now.duration_since(last) >= Duration::from_millis(minimum),
                        "post-upgrade auth rejection must back off"
                    );
                }
                previous = Some(now);
                let mut ws = tokio_tungstenite::accept_async(socket)
                    .await
                    .expect("upgrade before auth rejection");
                ws.close(Some(tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::Library(4001),
                    reason: "invalid token".into(),
                }))
                .await
                .expect("send post-upgrade auth rejection");
            }
        })
        .await;
        task.abort();
        trial.expect("bounded retries finish");
    }

    #[test]
    fn hostname_returns_something() {
        let h = hostname();
        assert!(!h.is_empty());
    }
}
