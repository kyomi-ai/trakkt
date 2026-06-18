// SPDX-License-Identifier: AGPL-3.0-or-later

//! PTY session lifecycle management for `trakkt-connect`.
//!
//! Each terminal session is a real PTY process (via `portable-pty`) with a
//! reader task that streams output back to the server over the WebSocket. PTY
//! sessions are keyed by `session_id` and survive WebSocket reconnects — they
//! are local processes managed independently of the server connection.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::{Mutex, mpsc};

use trakkt_connect_protocol::{AgentMessage, SessionEventKind, SessionInfo};

/// Configuration for the PTY manager.
pub struct PtyConfig {
    /// Default working directory for new sessions.
    pub working_dir: PathBuf,
    /// Commands allowed to be spawned (checked against the first element of
    /// the command vector).
    pub allowed_commands: Vec<String>,
    /// Maximum scrollback buffer size per session in bytes.
    pub scrollback_size: usize,
}

/// A single PTY session with its associated metadata and scrollback buffer.
struct PtySession {
    /// The command that was used to spawn this session.
    command: Vec<String>,
    /// Resolved working directory.
    working_dir: Option<String>,
    /// ISO 8601 timestamp of when the session started.
    started_at: String,
    /// Current terminal dimensions.
    cols: u16,
    rows: u16,
    /// OS process ID.
    pid: u32,
    /// Handle to the PTY master for writing input and resizing.
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Writer end of the PTY master (stdin to the child process).
    /// Wrapped in Option so it can be temporarily extracted for blocking writes.
    writer: Option<Box<dyn std::io::Write + Send>>,
    /// Scrollback ring buffer that accumulates PTY output.
    scrollback: ScrollbackBuffer,
    /// Handle for the child process, used for killing.
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Handle to the reader task so we can abort it on kill.
    reader_handle: tokio::task::JoinHandle<()>,
}

/// Ring buffer that stores the most recent `capacity` bytes of PTY output.
struct ScrollbackBuffer {
    buf: std::collections::VecDeque<u8>,
    capacity: usize,
}

impl ScrollbackBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buf: std::collections::VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push bytes into the ring buffer, evicting oldest data when full.
    fn push(&mut self, data: &[u8]) {
        if self.capacity == 0 {
            return;
        }
        for &byte in data {
            if self.buf.len() == self.capacity {
                self.buf.pop_front();
            }
            self.buf.push_back(byte);
        }
    }

    /// Return all accumulated scrollback bytes.
    fn dump(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }
}

/// Manages all PTY sessions for this agent instance.
pub struct PtyManager {
    sessions: Mutex<HashMap<String, PtySession>>,
    agent_tx: mpsc::Sender<AgentMessage>,
    config: PtyConfig,
}

impl PtyManager {
    /// Create a new PTY manager.
    pub fn new(agent_tx: mpsc::Sender<AgentMessage>, config: PtyConfig) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            agent_tx,
            config,
        }
    }

    /// Spawn a new PTY session.
    ///
    /// Validates the command against `allowed_commands`, spawns the process, and
    /// starts a reader task that streams output to the agent sender channel.
    pub async fn spawn(
        self: &Arc<Self>,
        session_id: String,
        command: Vec<String>,
        working_dir: Option<String>,
        env: HashMap<String, String>,
        cols: u16,
        rows: u16,
    ) {
        // Validate command
        if command.is_empty() {
            self.send_event(
                &session_id,
                SessionEventKind::SpawnFailed {
                    error: "empty command".to_string(),
                },
            )
            .await;
            return;
        }

        let cmd_name = &command[0];

        // Extract just the binary name for allowed_commands check
        let binary_name = std::path::Path::new(cmd_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd_name);

        if !self
            .config
            .allowed_commands
            .iter()
            .any(|c| c == binary_name)
        {
            self.send_event(
                &session_id,
                SessionEventKind::SpawnFailed {
                    error: format!("command not allowed: {cmd_name}"),
                },
            )
            .await;
            return;
        }

        // Check for duplicate session_id
        {
            let sessions = self.sessions.lock().await;
            if sessions.contains_key(&session_id) {
                self.send_event(
                    &session_id,
                    SessionEventKind::SpawnFailed {
                        error: format!("session already exists: {session_id}"),
                    },
                )
                .await;
                return;
            }
        }

        // Build PTY command
        let pty_system = NativePtySystem::default();
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = match pty_system.openpty(pty_size) {
            Ok(p) => p,
            Err(e) => {
                self.send_event(
                    &session_id,
                    SessionEventKind::SpawnFailed {
                        error: format!("failed to open PTY: {e}"),
                    },
                )
                .await;
                return;
            }
        };

        let mut cmd = CommandBuilder::new(&command[0]);
        for arg in &command[1..] {
            cmd.arg(arg);
        }

        // Set working directory
        let resolved_working_dir = working_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.config.working_dir.clone());
        cmd.cwd(&resolved_working_dir);

        // Set environment variables
        for (key, value) in &env {
            cmd.env(key, value);
        }

        // Ensure TERM is set
        if !env.contains_key("TERM") {
            cmd.env("TERM", "xterm-256color");
        }

        // Spawn the child process
        let child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                self.send_event(
                    &session_id,
                    SessionEventKind::SpawnFailed {
                        error: format!("failed to spawn process: {e}"),
                    },
                )
                .await;
                return;
            }
        };

        // Drop the slave — the child owns it now
        drop(pair.slave);

        let pid = child.process_id().unwrap_or(0);

        // Get a writer for stdin
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                self.send_event(
                    &session_id,
                    SessionEventKind::SpawnFailed {
                        error: format!("failed to get PTY writer: {e}"),
                    },
                )
                .await;
                return;
            }
        };

        // Get a reader for stdout
        let reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                self.send_event(
                    &session_id,
                    SessionEventKind::SpawnFailed {
                        error: format!("failed to get PTY reader: {e}"),
                    },
                )
                .await;
                return;
            }
        };

        let started_at = chrono::Utc::now().to_rfc3339();

        // Start the reader task
        let reader_handle = self.spawn_reader_task(session_id.clone(), reader);

        // Store the session
        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(
                session_id.clone(),
                PtySession {
                    command: command.clone(),
                    working_dir: Some(
                        resolved_working_dir.to_string_lossy().into_owned(),
                    ),
                    started_at,
                    cols,
                    rows,
                    pid,
                    master: pair.master,
                    writer: Some(writer),
                    scrollback: ScrollbackBuffer::new(self.config.scrollback_size),
                    child,
                    reader_handle,
                },
            );
        }

        // Notify server that the session started
        self.send_event(&session_id, SessionEventKind::Started)
            .await;

        tracing::info!(
            session_id,
            command = ?command,
            pid,
            "PTY session spawned"
        );
    }

    /// Write input data (base64-encoded) to a session's stdin.
    ///
    /// The actual write is dispatched to a blocking thread to avoid holding the
    /// sessions mutex during a potentially blocking PTY write.
    pub async fn write_input(&self, session_id: &str, data: &str) {
        let decoded = match BASE64.decode(data) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(session_id, error = %e, "Failed to base64-decode input");
                return;
            }
        };

        // Take the writer out from behind the mutex, write in a blocking
        // thread, then put it back. This prevents holding the mutex during
        // a potentially blocking PTY write.
        let writer = {
            let mut sessions = self.sessions.lock().await;
            let Some(session) = sessions.get_mut(session_id) else {
                tracing::warn!(session_id, "Session not found for input");
                return;
            };
            session.writer.take()
        };

        let Some(mut w) = writer else {
            tracing::warn!(session_id, "Session writer already in use or closed");
            return;
        };

        let sid = session_id.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let res = w.write_all(&decoded);
            (w, res)
        })
        .await;

        match result {
            Ok((w, write_result)) => {
                if let Err(e) = write_result {
                    tracing::warn!(session_id = sid, error = %e, "Failed to write to PTY");
                }
                // Put the writer back
                let mut sessions = self.sessions.lock().await;
                if let Some(session) = sessions.get_mut(&sid) {
                    session.writer = Some(w);
                }
            }
            Err(e) => {
                tracing::warn!(session_id = sid, error = %e, "Write task panicked");
            }
        }
    }

    /// Resize a session's PTY.
    pub async fn resize(&self, session_id: &str, cols: u16, rows: u16) {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(session_id) else {
            tracing::warn!(session_id, "Session not found for resize");
            return;
        };

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        if let Err(e) = session.master.resize(size) {
            tracing::warn!(session_id, error = %e, "Failed to resize PTY");
        } else {
            session.cols = cols;
            session.rows = rows;
        }
    }

    /// Kill a running session.
    ///
    /// Sends SIGKILL to the process (portable-pty's `kill` sends SIGKILL).
    /// If `force` is false, we first try SIGTERM via nix, then fall back to
    /// the portable-pty kill.
    pub async fn kill(&self, session_id: &str, force: bool) {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(session_id) else {
            tracing::warn!(session_id, "Session not found for kill");
            return;
        };

        let pid = session.pid;

        if !force {
            // Try SIGTERM first
            #[cfg(unix)]
            {
                use nix::sys::signal::{Signal, kill};
                use nix::unistd::Pid;

                if let Err(e) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                    tracing::warn!(session_id, pid, error = %e, "SIGTERM failed, using kill()");
                    if let Err(e) = session.child.kill() {
                        tracing::warn!(session_id, pid, error = %e, "Failed to kill process");
                    }
                }
            }
            #[cfg(not(unix))]
            {
                if let Err(e) = session.child.kill() {
                    tracing::warn!(session_id, pid, error = %e, "Failed to kill process");
                }
            }
        } else {
            // Force kill (SIGKILL)
            if let Err(e) = session.child.kill() {
                tracing::warn!(session_id, pid, error = %e, "Failed to force-kill process");
            }
        }

        tracing::info!(session_id, pid, force, "Kill signal sent to session");

        // Emit Killed event so the server knows this was a deliberate kill,
        // not a natural exit.
        let msg = AgentMessage::SessionEvent {
            session_id: session_id.to_string(),
            event: SessionEventKind::Killed,
        };
        // Release lock before async send
        drop(sessions);
        if let Err(e) = self.agent_tx.send(msg).await {
            tracing::warn!(session_id, error = %e, "Failed to send kill event");
        }
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions
            .iter()
            .map(|(id, s)| SessionInfo {
                session_id: id.clone(),
                command: s.command.clone(),
                working_dir: s.working_dir.clone(),
                started_at: s.started_at.clone(),
                cols: s.cols,
                rows: s.rows,
                pid: s.pid,
            })
            .collect()
    }

    /// Get the scrollback buffer contents for a session (base64-encoded).
    pub async fn get_scrollback(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .map(|s| BASE64.encode(s.scrollback.dump()))
    }

    /// Append data to a session's scrollback buffer.
    ///
    /// Called by the reader task when new output arrives.
    async fn append_scrollback(&self, session_id: &str, data: &[u8]) {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.scrollback.push(data);
        }
    }

    /// Remove a session from the map and clean up.
    async fn remove_session(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.remove(session_id) {
            session.reader_handle.abort();
            tracing::info!(session_id, "Session removed");
        }
    }

    /// Send a session event message to the server.
    async fn send_event(&self, session_id: &str, event: SessionEventKind) {
        let msg = AgentMessage::SessionEvent {
            session_id: session_id.to_string(),
            event,
        };
        if let Err(e) = self.agent_tx.send(msg).await {
            tracing::warn!(session_id, error = %e, "Failed to send session event");
        }
    }

    /// Spawn the blocking reader task for a PTY session.
    ///
    /// Reads from the PTY master fd in a blocking loop (via `spawn_blocking`),
    /// batches output at ~16ms intervals to avoid flooding the WebSocket, and
    /// detects process exit via EOF.
    fn spawn_reader_task(
        self: &Arc<Self>,
        session_id: String,
        mut reader: Box<dyn Read + Send>,
    ) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        let agent_tx = self.agent_tx.clone();

        tokio::task::spawn(async move {
            // Channel from the blocking reader thread to the async forwarder
            let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);

            let sid_blocking = session_id.clone();
            let blocking_handle = tokio::task::spawn_blocking(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                                break; // Receiver dropped
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                session_id = sid_blocking,
                                error = %e,
                                "PTY reader error (likely process exited)"
                            );
                            break;
                        }
                    }
                }
            });

            // Async forwarder: batches output from the blocking thread and sends
            // it to the server with ~16ms batching to reduce message count.
            let batch_interval = tokio::time::Duration::from_millis(16);
            let mut batch = Vec::new();

            loop {
                // Try to receive with a timeout for batching
                match tokio::time::timeout(batch_interval, output_rx.recv()).await {
                    Ok(Some(data)) => {
                        // Accumulate into the current batch
                        manager.append_scrollback(&session_id, &data).await;
                        batch.extend_from_slice(&data);

                        // Drain any additional immediately available data
                        while let Ok(more) = output_rx.try_recv() {
                            manager.append_scrollback(&session_id, &more).await;
                            batch.extend_from_slice(&more);
                        }

                        // Send the batch
                        if !batch.is_empty() {
                            let encoded = BASE64.encode(&batch);
                            let msg = AgentMessage::SessionOutput {
                                session_id: session_id.clone(),
                                data: encoded,
                            };
                            if agent_tx.send(msg).await.is_err() {
                                break; // Agent channel closed
                            }
                            batch.clear();
                        }
                    }
                    Ok(None) => {
                        // Channel closed — process exited. Flush any remaining batch.
                        if !batch.is_empty() {
                            let encoded = BASE64.encode(&batch);
                            let msg = AgentMessage::SessionOutput {
                                session_id: session_id.clone(),
                                data: encoded,
                            };
                            if let Err(e) = agent_tx.send(msg).await {
                                tracing::warn!(session_id, error = %e, "Failed to send final output flush");
                            }
                        }
                        break;
                    }
                    Err(_) => {
                        // Timeout — flush accumulated batch
                        if !batch.is_empty() {
                            let encoded = BASE64.encode(&batch);
                            let msg = AgentMessage::SessionOutput {
                                session_id: session_id.clone(),
                                data: encoded,
                            };
                            if agent_tx.send(msg).await.is_err() {
                                break;
                            }
                            batch.clear();
                        }
                    }
                }
            }

            // Wait for the blocking reader to finish
            let _ = blocking_handle.await;

            // Collect exit code. We must not hold the RwLockWriteGuard
            // across an await point because PtySession contains !Sync trait
            // objects, making the guard !Send.
            let first_try = {
                let mut sessions = manager.sessions.lock().await;
                if let Some(session) = sessions.get_mut(&session_id) {
                    match session.child.try_wait() {
                        Ok(Some(status)) => Some(status.exit_code() as i32),
                        Ok(None) => None, // Not exited yet
                        Err(_) => Some(-1),
                    }
                } else {
                    Some(-1)
                }
            };

            let exit_code = match first_try {
                Some(code) => code,
                None => {
                    // Process hasn't exited yet — wait briefly, then retry
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    let mut sessions = manager.sessions.lock().await;
                    if let Some(session) = sessions.get_mut(&session_id) {
                        session
                            .child
                            .try_wait()
                            .ok()
                            .flatten()
                            .map(|s| s.exit_code() as i32)
                            .unwrap_or(-1)
                    } else {
                        -1
                    }
                }
            };

            // Send exit event
            let msg = AgentMessage::SessionEvent {
                session_id: session_id.clone(),
                event: SessionEventKind::Exited { exit_code },
            };
            if let Err(e) = agent_tx.send(msg).await {
                tracing::warn!(session_id, error = %e, "Failed to send session exit event");
            }

            tracing::info!(session_id, exit_code, "PTY session exited");

            // Clean up
            manager.remove_session(&session_id).await;
        })
    }
}

/// Dispatch a [`ServerMessage`] to the appropriate [`PtyManager`] method.
///
/// This is the central dispatch function called by the WS reader loop.
pub async fn dispatch(
    manager: &Arc<PtyManager>,
    msg: trakkt_connect_protocol::ServerMessage,
    agent_tx: &mpsc::Sender<AgentMessage>,
) {
    use trakkt_connect_protocol::ServerMessage;

    match msg {
        ServerMessage::SpawnSession {
            session_id,
            command,
            working_dir,
            env,
            cols,
            rows,
        } => {
            manager
                .spawn(session_id, command, working_dir, env, cols, rows)
                .await;
        }
        ServerMessage::SessionInput { session_id, data } => {
            manager.write_input(&session_id, &data).await;
        }
        ServerMessage::SessionResize {
            session_id,
            cols,
            rows,
        } => {
            manager.resize(&session_id, cols, rows).await;
        }
        ServerMessage::SessionKill { session_id, force } => {
            manager.kill(&session_id, force).await;
        }
        ServerMessage::ScrollbackRequest { session_id } => {
            let data = manager
                .get_scrollback(&session_id)
                .await
                .unwrap_or_default();
            let msg = AgentMessage::ScrollbackDump { session_id, data };
            if let Err(e) = agent_tx.send(msg).await {
                tracing::warn!(error = %e, "Failed to send scrollback dump");
            }
        }
        ServerMessage::ListSessions => {
            let sessions = manager.list_sessions().await;
            let msg = AgentMessage::SessionList { sessions };
            if let Err(e) = agent_tx.send(msg).await {
                tracing::warn!(error = %e, "Failed to send session list");
            }
        }
        ServerMessage::Ping { ts } => {
            let msg = AgentMessage::Pong { ts };
            if let Err(e) = agent_tx.send(msg).await {
                tracing::warn!(error = %e, "Failed to send pong");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_buffer_basic() {
        let mut sb = ScrollbackBuffer::new(10);
        sb.push(b"hello");
        assert_eq!(sb.dump(), b"hello");
    }

    #[test]
    fn scrollback_buffer_wraps() {
        let mut sb = ScrollbackBuffer::new(5);
        sb.push(b"hello world");
        // Should keep last 5 bytes: "world"
        assert_eq!(sb.dump(), b"world");
    }

    #[test]
    fn scrollback_buffer_incremental_wrap() {
        let mut sb = ScrollbackBuffer::new(6);
        sb.push(b"abc");
        sb.push(b"defgh");
        // Total 8 bytes pushed, capacity 6: should keep "cdefgh"
        assert_eq!(sb.dump(), b"cdefgh");
    }

    #[test]
    fn scrollback_buffer_empty() {
        let sb = ScrollbackBuffer::new(10);
        assert!(sb.dump().is_empty());
    }

    #[test]
    fn scrollback_buffer_exact_capacity() {
        let mut sb = ScrollbackBuffer::new(5);
        sb.push(b"12345");
        assert_eq!(sb.dump(), b"12345");
    }

    #[test]
    fn scrollback_buffer_zero_capacity() {
        let mut sb = ScrollbackBuffer::new(0);
        sb.push(b"hello");
        assert!(sb.dump().is_empty());
    }
}
