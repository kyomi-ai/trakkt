// SPDX-License-Identifier: AGPL-3.0-or-later

//! Wire protocol types for the Trakkt Connect terminal session relay.
//!
//! Two top-level enums define the bidirectional message flow:
//!
//! - [`ServerMessage`]: commands sent from the Trakkt server to the agent.
//! - [`AgentMessage`]: events sent from the agent back to the server.
//!
//! Both use internally-tagged JSON (`#[serde(tag = "type")]`) with snake_case
//! variant names, so the `type` field doubles as a human-readable discriminator
//! in dev-tools and logs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ServerMessage -- server -> agent
// ---------------------------------------------------------------------------

/// Commands sent from the Trakkt server to the Connect agent.
///
/// Each variant maps to a JSON object with `"type": "<variant_name>"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Spawn a new PTY session with the given command.
    SpawnSession {
        session_id: String,
        command: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
        cols: u16,
        rows: u16,
    },
    /// Send input bytes (base64-encoded) to a running session.
    SessionInput {
        session_id: String,
        /// Base64-encoded input bytes.
        data: String,
    },
    /// Resize the PTY of a running session.
    SessionResize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    /// Kill a running session. When `force` is true, send SIGKILL instead of
    /// SIGTERM.
    SessionKill {
        session_id: String,
        #[serde(default)]
        force: bool,
    },
    /// Request the scrollback buffer for a session.
    ScrollbackRequest { session_id: String },
    /// List all active sessions.
    ListSessions,
    /// Keepalive ping. The agent should reply with [`AgentMessage::Pong`].
    Ping { ts: u64 },
}

// ---------------------------------------------------------------------------
// AgentMessage -- agent -> server
// ---------------------------------------------------------------------------

/// Events sent from the Connect agent to the Trakkt server.
///
/// Each variant maps to a JSON object with `"type": "<variant_name>"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// PTY output bytes (base64-encoded) from a running session.
    SessionOutput {
        session_id: String,
        /// Base64-encoded output bytes.
        data: String,
    },
    /// Lifecycle event for a session.
    SessionEvent {
        session_id: String,
        event: SessionEventKind,
    },
    /// Scrollback buffer dump (base64-encoded) for a session.
    ScrollbackDump {
        session_id: String,
        /// Base64-encoded scrollback bytes.
        data: String,
    },
    /// Response to [`ServerMessage::ListSessions`].
    SessionList { sessions: Vec<SessionInfo> },
    /// Sent once after the WebSocket is established. Signals the agent is
    /// ready to accept commands.
    Ready {
        agent_version: String,
        hostname: String,
        os: String,
    },
    /// Keepalive pong, echoing the server's timestamp.
    Pong { ts: u64 },
}

// ---------------------------------------------------------------------------
// SessionEventKind
// ---------------------------------------------------------------------------

/// Lifecycle events for a terminal session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEventKind {
    /// The session process started successfully.
    Started,
    /// The session process exited normally.
    Exited { exit_code: i32 },
    /// The session process was killed (by the agent or OS).
    Killed,
    /// The agent failed to spawn the session process.
    SpawnFailed { error: String },
}

// ---------------------------------------------------------------------------
// SessionInfo
// ---------------------------------------------------------------------------

/// Metadata about a running session, returned in [`AgentMessage::SessionList`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub command: Vec<String>,
    /// Resolved working directory of the session. `None` if the agent used its
    /// configured default and didn't resolve the actual path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// ISO 8601 timestamp of when the session started.
    pub started_at: String,
    pub cols: u16,
    pub rows: u16,
    pub pid: u32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // ServerMessage serialization
    // -----------------------------------------------------------------------

    #[test]
    fn server_spawn_session_serializes_correctly() {
        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());

        let msg = ServerMessage::SpawnSession {
            session_id: "sess-1".into(),
            command: vec!["bash".into(), "-l".into()],
            working_dir: Some("/home/user".into()),
            env,
            cols: 120,
            rows: 40,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "spawn_session");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["command"], json!(["bash", "-l"]));
        assert_eq!(json["working_dir"], "/home/user");
        assert_eq!(json["env"]["TERM"], "xterm-256color");
        assert_eq!(json["cols"], 120);
        assert_eq!(json["rows"], 40);
    }

    #[test]
    fn server_spawn_session_omits_empty_optional_fields() {
        let msg = ServerMessage::SpawnSession {
            session_id: "sess-2".into(),
            command: vec!["sh".into()],
            working_dir: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("working_dir").is_none());
        assert!(json.get("env").is_none());
    }

    #[test]
    fn server_session_input_serializes_correctly() {
        let msg = ServerMessage::SessionInput {
            session_id: "sess-1".into(),
            data: "bHMgLWxhCg==".into(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_input");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["data"], "bHMgLWxhCg==");
    }

    #[test]
    fn server_session_resize_serializes_correctly() {
        let msg = ServerMessage::SessionResize {
            session_id: "sess-1".into(),
            cols: 200,
            rows: 50,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_resize");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["cols"], 200);
        assert_eq!(json["rows"], 50);
    }

    #[test]
    fn server_session_kill_serializes_correctly() {
        let msg = ServerMessage::SessionKill {
            session_id: "sess-1".into(),
            force: true,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_kill");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["force"], true);
    }

    #[test]
    fn server_session_kill_force_defaults_to_false() {
        let raw = r#"{"type": "session_kill", "session_id": "sess-1"}"#;
        let msg: ServerMessage = serde_json::from_str(raw).unwrap();
        match msg {
            ServerMessage::SessionKill { force, .. } => assert!(!force),
            other => panic!("expected SessionKill, got {other:?}"),
        }
    }

    #[test]
    fn server_scrollback_request_serializes_correctly() {
        let msg = ServerMessage::ScrollbackRequest {
            session_id: "sess-1".into(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "scrollback_request");
        assert_eq!(json["session_id"], "sess-1");
    }

    #[test]
    fn server_list_sessions_serializes_correctly() {
        let msg = ServerMessage::ListSessions;

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "list_sessions");
    }

    #[test]
    fn server_ping_serializes_correctly() {
        let msg = ServerMessage::Ping { ts: 1718200000000 };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "ping");
        assert_eq!(json["ts"], 1718200000000u64);
    }

    // -----------------------------------------------------------------------
    // ServerMessage roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn server_spawn_session_roundtrip() {
        let mut env = HashMap::new();
        env.insert("HOME".into(), "/root".into());
        env.insert("SHELL".into(), "/bin/bash".into());

        let msg = ServerMessage::SpawnSession {
            session_id: "rt-1".into(),
            command: vec!["bash".into(), "-c".into(), "echo hello".into()],
            working_dir: Some("/tmp".into()),
            env,
            cols: 100,
            rows: 30,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SpawnSession {
                session_id,
                command,
                working_dir,
                env,
                cols,
                rows,
            } => {
                assert_eq!(session_id, "rt-1");
                assert_eq!(command, vec!["bash", "-c", "echo hello"]);
                assert_eq!(working_dir.as_deref(), Some("/tmp"));
                assert_eq!(env.get("HOME").map(|s| s.as_str()), Some("/root"));
                assert_eq!(env.get("SHELL").map(|s| s.as_str()), Some("/bin/bash"));
                assert_eq!(cols, 100);
                assert_eq!(rows, 30);
            }
            other => panic!("expected SpawnSession, got {other:?}"),
        }
    }

    #[test]
    fn server_session_input_roundtrip() {
        let msg = ServerMessage::SessionInput {
            session_id: "rt-2".into(),
            data: "dGVzdA==".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SessionInput { session_id, data } => {
                assert_eq!(session_id, "rt-2");
                assert_eq!(data, "dGVzdA==");
            }
            other => panic!("expected SessionInput, got {other:?}"),
        }
    }

    #[test]
    fn server_session_resize_roundtrip() {
        let msg = ServerMessage::SessionResize {
            session_id: "rt-3".into(),
            cols: 160,
            rows: 48,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SessionResize {
                session_id,
                cols,
                rows,
            } => {
                assert_eq!(session_id, "rt-3");
                assert_eq!(cols, 160);
                assert_eq!(rows, 48);
            }
            other => panic!("expected SessionResize, got {other:?}"),
        }
    }

    #[test]
    fn server_session_kill_roundtrip() {
        let msg = ServerMessage::SessionKill {
            session_id: "rt-4".into(),
            force: true,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SessionKill { session_id, force } => {
                assert_eq!(session_id, "rt-4");
                assert!(force);
            }
            other => panic!("expected SessionKill, got {other:?}"),
        }
    }

    #[test]
    fn server_scrollback_request_roundtrip() {
        let msg = ServerMessage::ScrollbackRequest {
            session_id: "rt-5".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::ScrollbackRequest { session_id } => {
                assert_eq!(session_id, "rt-5");
            }
            other => panic!("expected ScrollbackRequest, got {other:?}"),
        }
    }

    #[test]
    fn server_list_sessions_roundtrip() {
        let msg = ServerMessage::ListSessions;

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ServerMessage::ListSessions));
    }

    #[test]
    fn server_ping_roundtrip() {
        let msg = ServerMessage::Ping {
            ts: 1718200000000,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::Ping { ts } => assert_eq!(ts, 1718200000000),
            other => panic!("expected Ping, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // ServerMessage raw JSON deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn server_spawn_session_from_raw_json() {
        let raw = r#"{
            "type": "spawn_session",
            "session_id": "raw-1",
            "command": ["zsh"],
            "cols": 80,
            "rows": 24
        }"#;
        let msg: ServerMessage = serde_json::from_str(raw).unwrap();
        match msg {
            ServerMessage::SpawnSession {
                session_id,
                command,
                working_dir,
                env,
                cols,
                rows,
            } => {
                assert_eq!(session_id, "raw-1");
                assert_eq!(command, vec!["zsh"]);
                assert!(working_dir.is_none());
                assert!(env.is_empty());
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
            }
            other => panic!("expected SpawnSession, got {other:?}"),
        }
    }

    #[test]
    fn server_list_sessions_from_raw_json() {
        let raw = r#"{"type": "list_sessions"}"#;
        let msg: ServerMessage = serde_json::from_str(raw).unwrap();
        assert!(matches!(msg, ServerMessage::ListSessions));
    }

    #[test]
    fn server_ping_from_raw_json() {
        let raw = r#"{"type": "ping", "ts": 42}"#;
        let msg: ServerMessage = serde_json::from_str(raw).unwrap();
        match msg {
            ServerMessage::Ping { ts } => assert_eq!(ts, 42),
            other => panic!("expected Ping, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // ServerMessage unknown type rejection
    // -----------------------------------------------------------------------

    #[test]
    fn server_unknown_type_tag_fails() {
        let raw = r#"{"type": "drop_tables"}"#;
        let result: Result<ServerMessage, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "expected deserialization to fail for unknown type tag");
    }

    #[test]
    fn server_missing_type_tag_fails() {
        let raw = r#"{"session_id": "no-type"}"#;
        let result: Result<ServerMessage, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "expected deserialization to fail for missing type tag");
    }

    // -----------------------------------------------------------------------
    // AgentMessage serialization
    // -----------------------------------------------------------------------

    #[test]
    fn agent_session_output_serializes_correctly() {
        let msg = AgentMessage::SessionOutput {
            session_id: "sess-1".into(),
            data: "SGVsbG8gV29ybGQ=".into(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_output");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["data"], "SGVsbG8gV29ybGQ=");
    }

    #[test]
    fn agent_session_event_started_serializes_correctly() {
        let msg = AgentMessage::SessionEvent {
            session_id: "sess-1".into(),
            event: SessionEventKind::Started,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_event");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["event"]["kind"], "started");
    }

    #[test]
    fn agent_session_event_exited_serializes_correctly() {
        let msg = AgentMessage::SessionEvent {
            session_id: "sess-1".into(),
            event: SessionEventKind::Exited { exit_code: 0 },
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_event");
        assert_eq!(json["event"]["kind"], "exited");
        assert_eq!(json["event"]["exit_code"], 0);
    }

    #[test]
    fn agent_session_event_killed_serializes_correctly() {
        let msg = AgentMessage::SessionEvent {
            session_id: "sess-1".into(),
            event: SessionEventKind::Killed,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["event"]["kind"], "killed");
    }

    #[test]
    fn agent_session_event_spawn_failed_serializes_correctly() {
        let msg = AgentMessage::SessionEvent {
            session_id: "sess-1".into(),
            event: SessionEventKind::SpawnFailed {
                error: "command not found: zsh".into(),
            },
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["event"]["kind"], "spawn_failed");
        assert_eq!(json["event"]["error"], "command not found: zsh");
    }

    #[test]
    fn agent_scrollback_dump_serializes_correctly() {
        let msg = AgentMessage::ScrollbackDump {
            session_id: "sess-1".into(),
            data: "c2Nyb2xsYmFjaw==".into(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "scrollback_dump");
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["data"], "c2Nyb2xsYmFjaw==");
    }

    #[test]
    fn agent_session_list_serializes_correctly() {
        let msg = AgentMessage::SessionList {
            sessions: vec![SessionInfo {
                session_id: "sess-1".into(),
                command: vec!["bash".into()],
                working_dir: Some("/home/user".into()),
                started_at: "2026-06-12T10:00:00Z".into(),
                cols: 120,
                rows: 40,
                pid: 12345,
            }],
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_list");
        assert_eq!(json["sessions"][0]["session_id"], "sess-1");
        assert_eq!(json["sessions"][0]["command"], json!(["bash"]));
        assert_eq!(json["sessions"][0]["working_dir"], "/home/user");
        assert_eq!(json["sessions"][0]["started_at"], "2026-06-12T10:00:00Z");
        assert_eq!(json["sessions"][0]["cols"], 120);
        assert_eq!(json["sessions"][0]["rows"], 40);
        assert_eq!(json["sessions"][0]["pid"], 12345);
    }

    #[test]
    fn agent_session_list_empty_serializes_correctly() {
        let msg = AgentMessage::SessionList {
            sessions: vec![],
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "session_list");
        assert_eq!(json["sessions"], json!([]));
    }

    #[test]
    fn agent_ready_serializes_correctly() {
        let msg = AgentMessage::Ready {
            agent_version: "0.1.0".into(),
            hostname: "agent-host".into(),
            os: "linux".into(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["agent_version"], "0.1.0");
        assert_eq!(json["hostname"], "agent-host");
        assert_eq!(json["os"], "linux");
    }

    #[test]
    fn agent_pong_serializes_correctly() {
        let msg = AgentMessage::Pong { ts: 1718200000000 };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "pong");
        assert_eq!(json["ts"], 1718200000000u64);
    }

    // -----------------------------------------------------------------------
    // AgentMessage roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn agent_session_output_roundtrip() {
        let msg = AgentMessage::SessionOutput {
            session_id: "rt-1".into(),
            data: "YWJj".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionOutput { session_id, data } => {
                assert_eq!(session_id, "rt-1");
                assert_eq!(data, "YWJj");
            }
            other => panic!("expected SessionOutput, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_event_started_roundtrip() {
        let msg = AgentMessage::SessionEvent {
            session_id: "rt-2".into(),
            event: SessionEventKind::Started,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionEvent { session_id, event } => {
                assert_eq!(session_id, "rt-2");
                assert!(matches!(event, SessionEventKind::Started));
            }
            other => panic!("expected SessionEvent, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_event_exited_roundtrip() {
        let msg = AgentMessage::SessionEvent {
            session_id: "rt-3".into(),
            event: SessionEventKind::Exited { exit_code: 127 },
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionEvent { session_id, event } => {
                assert_eq!(session_id, "rt-3");
                match event {
                    SessionEventKind::Exited { exit_code } => assert_eq!(exit_code, 127),
                    other => panic!("expected Exited, got {other:?}"),
                }
            }
            other => panic!("expected SessionEvent, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_event_killed_roundtrip() {
        let msg = AgentMessage::SessionEvent {
            session_id: "rt-4".into(),
            event: SessionEventKind::Killed,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionEvent { event, .. } => {
                assert!(matches!(event, SessionEventKind::Killed));
            }
            other => panic!("expected SessionEvent, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_event_spawn_failed_roundtrip() {
        let msg = AgentMessage::SessionEvent {
            session_id: "rt-5".into(),
            event: SessionEventKind::SpawnFailed {
                error: "permission denied".into(),
            },
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionEvent { session_id, event } => {
                assert_eq!(session_id, "rt-5");
                match event {
                    SessionEventKind::SpawnFailed { error } => {
                        assert_eq!(error, "permission denied");
                    }
                    other => panic!("expected SpawnFailed, got {other:?}"),
                }
            }
            other => panic!("expected SessionEvent, got {other:?}"),
        }
    }

    #[test]
    fn agent_scrollback_dump_roundtrip() {
        let msg = AgentMessage::ScrollbackDump {
            session_id: "rt-6".into(),
            data: "ZHVtcA==".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::ScrollbackDump { session_id, data } => {
                assert_eq!(session_id, "rt-6");
                assert_eq!(data, "ZHVtcA==");
            }
            other => panic!("expected ScrollbackDump, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_list_roundtrip() {
        let info = SessionInfo {
            session_id: "s-1".into(),
            command: vec!["python3".into(), "app.py".into()],
            working_dir: Some("/srv/app".into()),
            started_at: "2026-06-12T08:30:00Z".into(),
            cols: 80,
            rows: 24,
            pid: 9876,
        };
        let msg = AgentMessage::SessionList {
            sessions: vec![info],
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionList { sessions } => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].session_id, "s-1");
                assert_eq!(sessions[0].command, vec!["python3", "app.py"]);
                assert_eq!(sessions[0].working_dir, Some("/srv/app".into()));
                assert_eq!(sessions[0].started_at, "2026-06-12T08:30:00Z");
                assert_eq!(sessions[0].cols, 80);
                assert_eq!(sessions[0].rows, 24);
                assert_eq!(sessions[0].pid, 9876);
            }
            other => panic!("expected SessionList, got {other:?}"),
        }
    }

    #[test]
    fn agent_ready_roundtrip() {
        let msg = AgentMessage::Ready {
            agent_version: "1.2.3".into(),
            hostname: "prod-worker-01".into(),
            os: "linux".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::Ready {
                agent_version,
                hostname,
                os,
            } => {
                assert_eq!(agent_version, "1.2.3");
                assert_eq!(hostname, "prod-worker-01");
                assert_eq!(os, "linux");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn agent_pong_roundtrip() {
        let msg = AgentMessage::Pong {
            ts: 1718200000000,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::Pong { ts } => assert_eq!(ts, 1718200000000),
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // AgentMessage raw JSON deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn agent_session_output_from_raw_json() {
        let raw = r#"{"type": "session_output", "session_id": "raw-1", "data": "AAAA"}"#;
        let msg: AgentMessage = serde_json::from_str(raw).unwrap();
        match msg {
            AgentMessage::SessionOutput { session_id, data } => {
                assert_eq!(session_id, "raw-1");
                assert_eq!(data, "AAAA");
            }
            other => panic!("expected SessionOutput, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_event_from_raw_json() {
        let raw = r#"{
            "type": "session_event",
            "session_id": "raw-2",
            "event": {"kind": "exited", "exit_code": 1}
        }"#;
        let msg: AgentMessage = serde_json::from_str(raw).unwrap();
        match msg {
            AgentMessage::SessionEvent { session_id, event } => {
                assert_eq!(session_id, "raw-2");
                match event {
                    SessionEventKind::Exited { exit_code } => assert_eq!(exit_code, 1),
                    other => panic!("expected Exited, got {other:?}"),
                }
            }
            other => panic!("expected SessionEvent, got {other:?}"),
        }
    }

    #[test]
    fn agent_ready_from_raw_json() {
        let raw = r#"{
            "type": "ready",
            "agent_version": "0.1.0",
            "hostname": "test-box",
            "os": "macos"
        }"#;
        let msg: AgentMessage = serde_json::from_str(raw).unwrap();
        match msg {
            AgentMessage::Ready {
                agent_version,
                hostname,
                os,
            } => {
                assert_eq!(agent_version, "0.1.0");
                assert_eq!(hostname, "test-box");
                assert_eq!(os, "macos");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn agent_pong_from_raw_json() {
        let raw = r#"{"type": "pong", "ts": 99}"#;
        let msg: AgentMessage = serde_json::from_str(raw).unwrap();
        match msg {
            AgentMessage::Pong { ts } => assert_eq!(ts, 99),
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_list_from_raw_json() {
        let raw = r#"{
            "type": "session_list",
            "sessions": [{
                "session_id": "s-raw",
                "command": ["node", "server.js"],
                "working_dir": "/app",
                "started_at": "2026-01-01T00:00:00Z",
                "cols": 80,
                "rows": 24,
                "pid": 1
            }]
        }"#;
        let msg: AgentMessage = serde_json::from_str(raw).unwrap();
        match msg {
            AgentMessage::SessionList { sessions } => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].session_id, "s-raw");
                assert_eq!(sessions[0].pid, 1);
            }
            other => panic!("expected SessionList, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // AgentMessage unknown type rejection
    // -----------------------------------------------------------------------

    #[test]
    fn agent_unknown_type_tag_fails() {
        let raw = r#"{"type": "unknown_event"}"#;
        let result: Result<AgentMessage, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "expected deserialization to fail for unknown type tag");
    }

    #[test]
    fn agent_missing_type_tag_fails() {
        let raw = r#"{"session_id": "no-type"}"#;
        let result: Result<AgentMessage, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "expected deserialization to fail for missing type tag");
    }

    // -----------------------------------------------------------------------
    // SessionEventKind roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn session_event_kind_started_roundtrip() {
        let kind = SessionEventKind::Started;
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: SessionEventKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SessionEventKind::Started));
    }

    #[test]
    fn session_event_kind_exited_roundtrip() {
        let kind = SessionEventKind::Exited { exit_code: -1 };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: SessionEventKind = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEventKind::Exited { exit_code } => assert_eq!(exit_code, -1),
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[test]
    fn session_event_kind_killed_roundtrip() {
        let kind = SessionEventKind::Killed;
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: SessionEventKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SessionEventKind::Killed));
    }

    #[test]
    fn session_event_kind_spawn_failed_roundtrip() {
        let kind = SessionEventKind::SpawnFailed {
            error: "No such file".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: SessionEventKind = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEventKind::SpawnFailed { error } => {
                assert_eq!(error, "No such file");
            }
            other => panic!("expected SpawnFailed, got {other:?}"),
        }
    }

    #[test]
    fn session_event_kind_unknown_fails() {
        let raw = r#"{"kind": "crashed"}"#;
        let result: Result<SessionEventKind, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "expected deserialization to fail for unknown kind");
    }

    // -----------------------------------------------------------------------
    // SessionInfo roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn session_info_roundtrip() {
        let info = SessionInfo {
            session_id: "si-1".into(),
            command: vec!["cargo".into(), "run".into()],
            working_dir: Some("/home/user/project".into()),
            started_at: "2026-06-12T12:00:00Z".into(),
            cols: 120,
            rows: 40,
            pid: 54321,
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "si-1");
        assert_eq!(parsed.command, vec!["cargo", "run"]);
        assert_eq!(parsed.working_dir, Some("/home/user/project".into()));
        assert_eq!(parsed.started_at, "2026-06-12T12:00:00Z");
        assert_eq!(parsed.cols, 120);
        assert_eq!(parsed.rows, 40);
        assert_eq!(parsed.pid, 54321);
    }

    #[test]
    fn session_info_from_raw_json() {
        let raw = r#"{
            "session_id": "si-raw",
            "command": ["sh", "-c", "sleep 10"],
            "working_dir": "/",
            "started_at": "2026-01-01T00:00:00Z",
            "cols": 80,
            "rows": 24,
            "pid": 1
        }"#;
        let info: SessionInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.session_id, "si-raw");
        assert_eq!(info.command, vec!["sh", "-c", "sleep 10"]);
        assert_eq!(info.working_dir, Some("/".into()));
        assert_eq!(info.pid, 1);
    }

    #[test]
    fn session_info_without_working_dir() {
        let raw = r#"{
            "session_id": "si-no-wd",
            "command": ["bash"],
            "started_at": "2026-01-01T00:00:00Z",
            "cols": 80,
            "rows": 24,
            "pid": 42
        }"#;
        let info: SessionInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.session_id, "si-no-wd");
        assert!(info.working_dir.is_none());
    }

    #[test]
    fn session_info_missing_required_field_fails() {
        // Missing session_id
        let raw = r#"{
            "command": ["bash"],
            "started_at": "2026-01-01T00:00:00Z",
            "cols": 80,
            "rows": 24,
            "pid": 1
        }"#;
        let result: Result<SessionInfo, _> = serde_json::from_str(raw);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn server_spawn_session_with_empty_command() {
        let msg = ServerMessage::SpawnSession {
            session_id: "edge-1".into(),
            command: vec![],
            working_dir: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SpawnSession { command, .. } => {
                assert!(command.is_empty());
            }
            other => panic!("expected SpawnSession, got {other:?}"),
        }
    }

    #[test]
    fn server_session_input_with_empty_data() {
        let msg = ServerMessage::SessionInput {
            session_id: "edge-2".into(),
            data: String::new(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SessionInput { data, .. } => {
                assert!(data.is_empty());
            }
            other => panic!("expected SessionInput, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_output_with_empty_data() {
        let msg = AgentMessage::SessionOutput {
            session_id: "edge-3".into(),
            data: String::new(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionOutput { data, .. } => {
                assert!(data.is_empty());
            }
            other => panic!("expected SessionOutput, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_list_empty_roundtrip() {
        let msg = AgentMessage::SessionList {
            sessions: vec![],
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionList { sessions } => {
                assert!(sessions.is_empty());
            }
            other => panic!("expected SessionList, got {other:?}"),
        }
    }

    #[test]
    fn session_event_exited_with_negative_exit_code() {
        let kind = SessionEventKind::Exited { exit_code: -9 };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: SessionEventKind = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEventKind::Exited { exit_code } => assert_eq!(exit_code, -9),
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[test]
    fn session_event_spawn_failed_with_empty_error() {
        let kind = SessionEventKind::SpawnFailed {
            error: String::new(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: SessionEventKind = serde_json::from_str(&json).unwrap();
        match parsed {
            SessionEventKind::SpawnFailed { error } => {
                assert!(error.is_empty());
            }
            other => panic!("expected SpawnFailed, got {other:?}"),
        }
    }

    #[test]
    fn server_spawn_session_with_empty_session_id() {
        let msg = ServerMessage::SpawnSession {
            session_id: String::new(),
            command: vec!["bash".into()],
            working_dir: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SpawnSession { session_id, .. } => {
                assert!(session_id.is_empty());
            }
            other => panic!("expected SpawnSession, got {other:?}"),
        }
    }

    #[test]
    fn agent_ready_with_empty_strings() {
        let msg = AgentMessage::Ready {
            agent_version: String::new(),
            hostname: String::new(),
            os: String::new(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::Ready {
                agent_version,
                hostname,
                os,
            } => {
                assert!(agent_version.is_empty());
                assert!(hostname.is_empty());
                assert!(os.is_empty());
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn ping_pong_ts_zero() {
        let ping = ServerMessage::Ping { ts: 0 };
        let json = serde_json::to_string(&ping).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::Ping { ts } => assert_eq!(ts, 0),
            other => panic!("expected Ping, got {other:?}"),
        }

        let pong = AgentMessage::Pong { ts: 0 };
        let json = serde_json::to_string(&pong).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::Pong { ts } => assert_eq!(ts, 0),
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[test]
    fn ping_pong_ts_max() {
        let ping = ServerMessage::Ping { ts: u64::MAX };
        let json = serde_json::to_string(&ping).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::Ping { ts } => assert_eq!(ts, u64::MAX),
            other => panic!("expected Ping, got {other:?}"),
        }
    }

    #[test]
    fn server_spawn_session_with_large_env() {
        let mut env = HashMap::new();
        for i in 0..100 {
            env.insert(format!("VAR_{i}"), format!("value_{i}"));
        }

        let msg = ServerMessage::SpawnSession {
            session_id: "env-test".into(),
            command: vec!["env".into()],
            working_dir: None,
            env,
            cols: 80,
            rows: 24,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::SpawnSession { env, .. } => {
                assert_eq!(env.len(), 100);
                assert_eq!(env.get("VAR_0").map(|s| s.as_str()), Some("value_0"));
                assert_eq!(env.get("VAR_99").map(|s| s.as_str()), Some("value_99"));
            }
            other => panic!("expected SpawnSession, got {other:?}"),
        }
    }

    #[test]
    fn agent_session_list_multiple_sessions_roundtrip() {
        let sessions = vec![
            SessionInfo {
                session_id: "s-1".into(),
                command: vec!["bash".into()],
                working_dir: Some("/home/a".into()),
                started_at: "2026-06-12T08:00:00Z".into(),
                cols: 80,
                rows: 24,
                pid: 100,
            },
            SessionInfo {
                session_id: "s-2".into(),
                command: vec!["python3".into(), "-m".into(), "http.server".into()],
                working_dir: Some("/srv/www".into()),
                started_at: "2026-06-12T09:00:00Z".into(),
                cols: 120,
                rows: 40,
                pid: 200,
            },
        ];
        let msg = AgentMessage::SessionList { sessions };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            AgentMessage::SessionList { sessions } => {
                assert_eq!(sessions.len(), 2);
                assert_eq!(sessions[0].session_id, "s-1");
                assert_eq!(sessions[1].session_id, "s-2");
                assert_eq!(sessions[1].command.len(), 3);
            }
            other => panic!("expected SessionList, got {other:?}"),
        }
    }
}
