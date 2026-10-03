// SPDX-License-Identifier: AGPL-3.0-or-later

//! TOML configuration file for `trakkt-connect`.
//!
//! Stored at `~/.config/trakkt-connect/config.toml` (XDG on Linux, platform
//! default on macOS/Windows). The file is created with `0o600` permissions to
//! protect the token.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// On-disk representation of the trakkt-connect config file.
///
/// All fields are optional — the caller merges these values with environment
/// variables (env takes precedence).
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    pub token: Option<String>,
    pub server_url: Option<String>,
    pub working_dir: Option<String>,
    pub allowed_commands: Option<Vec<String>>,
    pub health_port: Option<u16>,
    pub scrollback_size: Option<usize>,
}

impl ConfigFile {
    /// Return the default config directory path.
    pub fn default_config_dir() -> anyhow::Result<PathBuf> {
        dirs::config_dir()
            .map(|d| d.join("trakkt-connect"))
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))
    }

    /// Standard config file paths, in priority order.
    pub fn config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(config_dir) = dirs::config_dir() {
            paths.push(config_dir.join("trakkt-connect").join("config.toml"));
        }
        paths.push(PathBuf::from("/etc/trakkt-connect/config.toml"));
        paths
    }

    /// Load config from the first existing file in standard paths.
    ///
    /// Returns `Ok(None)` if no config file exists, `Err` on parse failure.
    pub fn load() -> anyhow::Result<Option<Self>> {
        for path in Self::config_paths() {
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("Failed to read {}: {e}", path.display()))?;
                let config: Self = toml::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("Failed to parse {}: {e}", path.display()))?;
                tracing::info!(path = %path.display(), "Loaded config file");
                return Ok(Some(config));
            }
        }
        Ok(None)
    }

    /// Save config to a specific directory.
    pub fn save_to(&self, config_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        use std::io::Write;
        file.write_all(content.as_bytes())?;

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ConfigFile {
        ConfigFile {
            token: Some("trakkt-test-token-value".to_string()),
            server_url: Some("wss://trakkt.app/ws/connect/agent".to_string()),
            working_dir: Some("/home/user/projects".to_string()),
            allowed_commands: Some(vec![
                "claude".to_string(),
                "bash".to_string(),
                "zsh".to_string(),
            ]),
            health_port: Some(9090),
            scrollback_size: Some(65536),
        }
    }

    #[test]
    fn round_trip_toml() {
        let config = test_config();
        let toml_str = toml::to_string_pretty(&config).expect("valid config test fixture");
        let loaded: ConfigFile = toml::from_str(&toml_str).expect("valid config test fixture");
        assert_eq!(loaded.token, config.token);
        assert_eq!(loaded.server_url, config.server_url);
        assert_eq!(loaded.working_dir, config.working_dir);
        assert_eq!(loaded.allowed_commands, config.allowed_commands);
        assert_eq!(loaded.health_port, config.health_port);
        assert_eq!(loaded.scrollback_size, config.scrollback_size);
    }

    #[test]
    fn round_trip_minimal_config() {
        let config = ConfigFile {
            token: Some("trakkt-minimal-test-value".to_string()),
            server_url: None,
            working_dir: None,
            allowed_commands: None,
            health_port: None,
            scrollback_size: None,
        };
        let toml_str = toml::to_string_pretty(&config).expect("valid config test fixture");
        let loaded: ConfigFile = toml::from_str(&toml_str).expect("valid config test fixture");
        assert_eq!(loaded.token, config.token);
        assert!(loaded.server_url.is_none());
        assert!(loaded.working_dir.is_none());
        assert!(loaded.allowed_commands.is_none());
        assert!(loaded.health_port.is_none());
        assert!(loaded.scrollback_size.is_none());
    }

    #[test]
    fn default_config_dir_returns_path() {
        let result = ConfigFile::default_config_dir();
        assert!(result.is_ok());
        let path = result.expect("valid config test fixture");
        assert!(path.ends_with("trakkt-connect"));
    }

    #[test]
    fn config_paths_not_empty() {
        let paths = ConfigFile::config_paths();
        assert!(!paths.is_empty());
        assert_eq!(
            paths.last().expect("valid config test fixture"),
            &PathBuf::from("/etc/trakkt-connect/config.toml")
        );
    }

    #[test]
    fn save_to_writes_config_and_sets_permissions() {
        let tmp =
            std::env::temp_dir().join(format!("trakkt-connect-test-save-{}", std::process::id()));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).expect("remove previous test fixture");
        }

        let config = test_config();
        let path = config.save_to(&tmp).expect("valid config test fixture");

        assert!(path.exists());
        let written = std::fs::read_to_string(&path).expect("valid config test fixture");
        assert!(written.contains("trakkt-test-token-value"));
        assert!(written.contains("wss://trakkt.app/ws/connect/agent"));

        // Verify round-trip through file
        let loaded: ConfigFile = toml::from_str(&written).expect("valid config test fixture");
        assert_eq!(loaded.token, config.token);
        assert_eq!(loaded.server_url, config.server_url);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&path)
                .expect("valid config test fixture")
                .permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }

        std::fs::remove_dir_all(&tmp).expect("valid config test fixture");
    }
}
