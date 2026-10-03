// SPDX-License-Identifier: AGPL-3.0-or-later

//! Session-owned terminal buffers survive tab changes and connection loss.

use std::collections::HashMap;

use trakkt_connect_protocol::{SessionEventKind, SessionInfo};

use super::tab_manager::SessionTab;
use super::{Grid, TerminalHandler};

pub struct Session {
    pub label: String,
    pub grid: Grid,
    parser: vte::Parser,
    pub running: bool,
    pub restoring: bool,
    subscribed: bool,
    spawn_failure: Option<String>,
}

#[derive(Default)]
pub struct Sessions {
    pub entries: HashMap<String, Session>,
    order: Vec<String>,
    pub active: Option<String>,
}

impl Sessions {
    pub fn add(&mut self, id: String, label: String, cols: u16, rows: u16) {
        if !self.entries.contains_key(&id) {
            self.order.push(id.clone());
            self.entries.insert(
                id.clone(),
                Session {
                    label,
                    grid: Grid::new(cols as usize, rows as usize),
                    parser: vte::Parser::new(),
                    running: true,
                    restoring: false,
                    subscribed: true,
                    spawn_failure: None,
                },
            );
        }
        self.active = Some(id);
    }

    pub fn tabs(&self) -> Vec<SessionTab> {
        self.order
            .iter()
            .filter_map(|id| {
                self.entries.get(id).map(|session| SessionTab {
                    session_id: id.clone(),
                    label: if session.running {
                        session.label.clone()
                    } else {
                        format!("{} (ended)", session.label)
                    },
                })
            })
            .collect()
    }

    pub fn grid(&self) -> Grid {
        self.active
            .as_ref()
            .and_then(|id| self.entries.get(id))
            .map(|session| session.grid.clone())
            .unwrap_or_else(|| Grid::new(80, 24))
    }

    /// A lost browser/agent connection invalidates output subscriptions, not buffers.
    pub fn disconnected(&mut self) {
        for entry in self.entries.values_mut() {
            entry.subscribed = false;
        }
    }

    /// Reconcile presence without replaying healthy streams on periodic lists.
    pub fn restore_list(&mut self, sessions: &[SessionInfo], connected: bool) -> Vec<String> {
        // Agent absence is not evidence that its local PTYs have exited.
        if !connected {
            return Vec::new();
        }
        for entry in self.entries.values_mut() {
            entry.running = false;
        }
        let previous_active = self.active.clone();
        let mut requests = Vec::new();
        for session in sessions {
            let new = !self.entries.contains_key(&session.session_id);
            self.add(
                session.session_id.clone(),
                session.command.join(" "),
                session.cols,
                session.rows,
            );
            if let Some(entry) = self.entries.get_mut(&session.session_id) {
                entry.running = true;
                if new || !entry.subscribed {
                    entry.subscribed = true;
                    entry.restoring = true;
                    requests.push(session.session_id.clone());
                }
            }
        }
        for entry in self.entries.values_mut().filter(|entry| !entry.running) {
            entry.subscribed = false;
        }
        self.active = previous_active
            .filter(|id| self.entries.contains_key(id))
            .or_else(|| self.order.first().cloned());
        requests
    }

    pub fn output(&mut self, id: &str, bytes: &[u8], scrollback: bool) -> Vec<u8> {
        let Some(session) = self.entries.get_mut(id) else {
            return Vec::new();
        };
        if scrollback {
            let cols = session.grid.cols;
            let rows = session.grid.rows;
            session.grid = Grid::new(cols, rows);
            session.parser = vte::Parser::new();
            session.restoring = false;
        } else if session.restoring {
            // The requested dump includes output queued before it. Replaying
            // these bytes as well would duplicate content after reconnect.
            return Vec::new();
        }
        let mut handler = TerminalHandler::new(&mut session.grid);
        for byte in bytes {
            session.parser.advance(&mut handler, *byte);
        }
        std::mem::take(&mut session.grid.response_bytes)
    }

    pub fn event(&mut self, id: &str, event: &SessionEventKind) -> Option<String> {
        let Some(session) = self.entries.get_mut(id) else {
            return match event {
                SessionEventKind::SpawnFailed { error } => Some(error.clone()),
                _ => None,
            };
        };
        match event {
            SessionEventKind::Started => session.running = true,
            SessionEventKind::Exited { exit_code } => {
                session.running = false;
                if *exit_code == 0 {
                    return None;
                }
                return Some(format!(
                    "{} exited with code {exit_code}. Its output remains available.",
                    session.label
                ));
            }
            SessionEventKind::Killed => session.running = false,
            SessionEventKind::SpawnFailed { error } => {
                session.running = false;
                // A command queued while spawning can be rejected after the
                // failed spawn removes its server reservation. Keep the first
                // cause instead of replacing it with that follow-up error.
                if session.spawn_failure.is_some() {
                    return None;
                }
                let message = format!("Could not start {}: {error}", session.label);
                session.spawn_failure = Some(message.clone());
                return Some(message);
            }
        }
        None
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
        self.order.retain(|entry| entry != id);
        if self.active.as_deref() == Some(id) {
            self.active = self.order.first().cloned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(grid: &Grid) -> String {
        grid.cells
            .iter()
            .flat_map(|row| row.iter())
            .map(|cell| cell.c)
            .collect()
    }

    #[test]
    fn switching_sessions_preserves_parser_modes_and_scrollback() {
        let mut sessions = Sessions::default();
        sessions.add("one".into(), "shell".into(), 10, 2);
        sessions.output("one", b"first\r\nsecond\r\nthird\x1b[?1h", false);
        let saved = sessions.grid();
        sessions.add("two".into(), "shell".into(), 10, 2);
        sessions.output("two", b"other", false);
        sessions.active = Some("one".into());
        let restored = sessions.grid();
        assert_eq!(visible(&restored), visible(&saved));
        assert!(!restored.scrollback.is_empty());
        assert!(restored.modes.application_cursor_keys);
    }

    #[test]
    fn failed_and_exited_sessions_keep_their_output() {
        let mut sessions = Sessions::default();
        sessions.add("one".into(), "shell".into(), 80, 24);
        sessions.output("one", b"important output", false);
        assert!(
            sessions
                .event(
                    "one",
                    &SessionEventKind::SpawnFailed {
                        error: "missing executable".into()
                    }
                )
                .expect("spawn failure must be visible")
                .contains("missing executable")
        );
        assert!(visible(&sessions.grid()).contains("important output"));
        assert!(!sessions.entries["one"].running);
        sessions.remove("one");
        assert!(sessions.active.is_none());
    }

    #[test]
    fn follow_up_command_failure_preserves_each_sessions_original_spawn_error() {
        let mut sessions = Sessions::default();
        sessions.add("one".into(), "claude".into(), 80, 24);
        sessions.output("one", b"retained output", false);
        // An earlier list can mark a pending session ended before its failure.
        sessions.restore_list(&[], true);
        let original = sessions
            .event(
                "one",
                &SessionEventKind::SpawnFailed {
                    error: "command not allowed: claude".into(),
                },
            )
            .expect("the first spawn failure must be visible even after an empty list");
        assert!(original.contains("command not allowed: claude"));
        assert_eq!(
            sessions.event(
                "one",
                &SessionEventKind::SpawnFailed {
                    error: "Unknown session".into(),
                },
            ),
            None
        );
        assert_eq!(
            sessions.entries["one"].spawn_failure.as_ref(),
            Some(&original)
        );
        assert!(visible(&sessions.grid()).contains("retained output"));
        assert!(!sessions.entries["one"].running);

        sessions.add("two".into(), "sh".into(), 80, 24);
        assert!(
            sessions
                .event(
                    "two",
                    &SessionEventKind::SpawnFailed {
                        error: "missing executable".into(),
                    },
                )
                .expect("a different session's first failure must still be visible")
                .contains("missing executable")
        );
        assert_eq!(
            sessions.entries["one"].spawn_failure.as_ref(),
            Some(&original)
        );
    }

    #[test]
    fn fullscreen_cli_restores_primary_cells_and_cursor_after_resize() {
        let mut sessions = Sessions::default();
        sessions.add("one".into(), "shell".into(), 20, 4);
        sessions.output("one", b"shell output\x1b[?1049h\x1b[Hfullscreen", false);
        assert!(sessions.grid().modes.alternate_screen);
        assert!(visible(&sessions.grid()).contains("fullscreen"));
        assert!(!visible(&sessions.grid()).contains("shell output"));
        sessions
            .entries
            .get_mut("one")
            .expect("session exists")
            .grid
            .resize(30, 5);
        sessions.output("one", b"\x1b[?1049l", false);
        let grid = sessions.grid();
        assert!(!grid.modes.alternate_screen);
        assert_eq!((grid.cols, grid.rows), (30, 5));
        assert!(visible(&grid).contains("shell output"));
        assert_eq!(grid.cursor.col, "shell output".len());
    }

    #[test]
    fn reconnection_dump_replaces_old_content_without_duplicate_live_output() {
        let mut sessions = Sessions::default();
        let info = SessionInfo {
            session_id: "one".into(),
            command: vec!["sh".into()],
            working_dir: None,
            started_at: "2026-10-03T00:00:00Z".into(),
            cols: 80,
            rows: 24,
            pid: 1,
        };
        sessions.add("one".into(), "shell".into(), 80, 24);
        sessions.output("one", b"old output", false);
        sessions.disconnected();
        assert_eq!(sessions.restore_list(&[info], true), vec!["one"]);
        sessions.output("one", b"queued before snapshot", false);
        sessions.output("one", b"whole snapshot", true);
        sessions.output("one", b" then live", false);
        let text = visible(&sessions.grid());
        assert!(text.contains("whole snapshot then live"));
        assert!(!text.contains("old output"));
        assert!(!text.contains("queued before snapshot"));
    }
    #[test]
    fn healthy_periodic_lists_preserve_fullscreen_parser_and_live_output() {
        let mut sessions = Sessions::default();
        sessions.add("one".into(), "sh".into(), 80, 24);
        let info = SessionInfo {
            session_id: "one".into(),
            command: vec!["sh".into()],
            working_dir: None,
            started_at: "now".into(),
            cols: 80,
            rows: 24,
            pid: 1,
        };
        sessions.output("one", b"primary\x1b[?1049hfullscreen\x1b[3", false);
        assert!(
            sessions
                .restore_list(std::slice::from_ref(&info), true)
                .is_empty()
        );
        assert!(
            sessions
                .restore_list(std::slice::from_ref(&info), true)
                .is_empty()
        );
        // Finish a split SGR sequence after the list: the existing parser must survive.
        sessions.output("one", b"1m red\x1b[0m", false);
        let grid = sessions.grid();
        assert!(grid.modes.alternate_screen);
        assert!(visible(&grid).contains("fullscreen red"));
        assert_eq!(grid.cells[0][11].fg, super::super::Color::Indexed(1));
        sessions.output("one", b"\x1b[?1049l", false);
        assert!(visible(&sessions.grid()).contains("primary"));
        sessions.disconnected();
        assert_eq!(
            sessions.restore_list(std::slice::from_ref(&info), true),
            vec!["one"]
        );
        // Another list while recovery is pending must not enqueue a second dump.
        assert!(sessions.restore_list(&[info], true).is_empty());
    }
    #[test]
    fn absent_agent_empty_list_keeps_live_session_and_output() {
        let mut sessions = Sessions::default();
        sessions.add("one".into(), "sh".into(), 80, 24);
        sessions.output("one", b"valuable output", false);
        // Production receives AgentStatus(false), then the relay's empty list.
        sessions.disconnected();
        assert!(sessions.restore_list(&[], false).is_empty());
        assert!(sessions.entries["one"].running);
        assert!(visible(&sessions.grid()).contains("valuable output"));
        assert_eq!(sessions.active.as_deref(), Some("one"));
        // A connected agent's empty list can authoritatively report an exit.
        assert!(sessions.restore_list(&[], true).is_empty());
        assert!(!sessions.entries["one"].running);
        assert!(visible(&sessions.grid()).contains("valuable output"));
    }
}
