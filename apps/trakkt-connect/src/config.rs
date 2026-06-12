// SPDX-License-Identifier: AGPL-3.0-or-later

//! Runtime configuration for `trakkt-connect`.
//!
//! Merges values from environment variables and the TOML config file. Environment
//! variables always take precedence.

use std::path::PathBuf;

use crate::config_file::ConfigFile;

/// Default commands that may be spawned as PTY sessions.
const DEFAULT_ALLOWED_COMMANDS: &[&str] = &["claude", "bash", "zsh", "sh", "fish"];

/// Default maximum size of the per-session scrollback ring buffer (bytes).
const DEFAULT_SCROLLBACK_SIZE: usize = 65536;

/// Default port for the health check HTTP server.
const DEFAULT_HEALTH_PORT: u16 = 9090;

/// Resolved runtime configuration.
pub struct ConnectConfig {
    /// Bearer token for authenticating with the Trakkt server.
    pub token: String,
    /// WebSocket URL of the Trakkt server (e.g. `wss://app.trakkt.dev/api/connect/ws`).
    pub server_url: String,
    /// Default working directory for spawned PTY sessions.
    pub working_dir: PathBuf,
    /// Commands allowed to be spawned. The first element of a `SpawnSession`
    /// command vector must appear in this list.
    pub allowed_commands: Vec<String>,
    /// Port for the health check HTTP server.
    pub health_port: u16,
    /// Maximum size of the per-session scrollback ring buffer in bytes.
    pub scrollback_size: usize,
}

impl ConnectConfig {
    /// Load configuration by merging environment variables with the config file.
    ///
    /// Environment variables:
    /// - `TRAKKT_TOKEN` — bearer token (required)
    /// - `TRAKKT_SERVER_URL` — WebSocket URL (required)
    /// - `TRAKKT_WORKING_DIR` — default working directory
    /// - `TRAKKT_ALLOWED_COMMANDS` — comma-separated list of allowed commands
    /// - `TRAKKT_HEALTH_PORT` — health check port
    /// - `TRAKKT_SCROLLBACK_SIZE` — scrollback buffer size in bytes
    pub fn load() -> anyhow::Result<Self> {
        let file_config = match ConfigFile::load() {
            Ok(cf) => cf,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load config file, using env vars only");
                None
            }
        };

        // Token — required
        let token = std::env::var("TRAKKT_TOKEN")
            .ok()
            .or_else(|| file_config.as_ref().and_then(|cf| cf.token.clone()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "TRAKKT_TOKEN not set. Set the environment variable or add 'token' \
                     to ~/.config/trakkt-connect/config.toml"
                )
            })?;

        // Server URL — required
        let server_url = std::env::var("TRAKKT_SERVER_URL")
            .ok()
            .or_else(|| file_config.as_ref().and_then(|cf| cf.server_url.clone()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "TRAKKT_SERVER_URL not set. Set the environment variable or add \
                     'server_url' to ~/.config/trakkt-connect/config.toml"
                )
            })?;

        // Working directory — defaults to current directory
        let working_dir = std::env::var("TRAKKT_WORKING_DIR")
            .ok()
            .or_else(|| file_config.as_ref().and_then(|cf| cf.working_dir.clone()))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"))
            });

        // Allowed commands (comma-separated env var)
        let allowed_commands = std::env::var("TRAKKT_ALLOWED_COMMANDS")
            .ok()
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .or_else(|| file_config.as_ref().and_then(|cf| cf.allowed_commands.clone()))
            .unwrap_or_else(|| {
                DEFAULT_ALLOWED_COMMANDS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect()
            });

        // Health port
        let health_port = std::env::var("TRAKKT_HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .or_else(|| file_config.as_ref().and_then(|cf| cf.health_port))
            .unwrap_or(DEFAULT_HEALTH_PORT);

        // Scrollback size
        let scrollback_size = std::env::var("TRAKKT_SCROLLBACK_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .or_else(|| file_config.as_ref().and_then(|cf| cf.scrollback_size))
            .unwrap_or(DEFAULT_SCROLLBACK_SIZE);

        Ok(Self {
            token,
            server_url,
            working_dir,
            allowed_commands,
            health_port,
            scrollback_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_allowed_commands_includes_expected() {
        assert!(DEFAULT_ALLOWED_COMMANDS.contains(&"claude"));
        assert!(DEFAULT_ALLOWED_COMMANDS.contains(&"bash"));
        assert!(DEFAULT_ALLOWED_COMMANDS.contains(&"zsh"));
        assert!(DEFAULT_ALLOWED_COMMANDS.contains(&"sh"));
        assert!(DEFAULT_ALLOWED_COMMANDS.contains(&"fish"));
    }

    #[test]
    fn default_scrollback_size_is_64k() {
        assert_eq!(DEFAULT_SCROLLBACK_SIZE, 65536);
    }

    #[test]
    fn default_health_port_is_9090() {
        assert_eq!(DEFAULT_HEALTH_PORT, 9090);
    }
}
