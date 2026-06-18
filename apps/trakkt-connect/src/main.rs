// SPDX-License-Identifier: AGPL-3.0-or-later

//! `trakkt-connect` — customer-deployed agent for interactive terminal sessions.
//!
//! Connects outbound to the Trakkt server via WebSocket and spawns PTY sessions
//! on demand. Terminal processes run on the customer's own infrastructure; the
//! server relays I/O between the web UI and this agent.

mod config;
mod config_file;
mod health;
mod pty_manager;
mod ws_client;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};
use tokio::sync::mpsc;

use crate::config::ConnectConfig;
use crate::pty_manager::{PtyConfig, PtyManager};
use crate::ws_client::WsClient;

#[derive(Parser)]
#[command(
    name = "trakkt-connect",
    about = "Trakkt Connect — terminal session agent"
)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the Connect agent
    Run,
    /// Show connection status
    Status,
    /// Set up the agent (prints instructions)
    Setup,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Run) | None => {
            // Default behavior: run the agent
            run_agent().await;
        }
        Some(Commands::Status) => {
            run_status().await;
        }
        Some(Commands::Setup) => {
            run_setup();
        }
    }
}

async fn run_agent() {
    eprintln!();
    eprintln!("  Trakkt Connect");
    eprintln!("  ──────────────");
    eprintln!();

    // 1. Load configuration
    eprint!("  Configuration        ");
    let config = match ConnectConfig::load() {
        Ok(c) => {
            eprintln!("\x1b[32m\u{2713}\x1b[0m");
            c
        }
        Err(e) => {
            eprintln!("\x1b[31m\u{2717}\x1b[0m  {e}");
            eprintln!();
            eprintln!("  Run 'trakkt-connect setup' for configuration instructions.");
            std::process::exit(1);
        }
    };

    let health_port = config.health_port;
    let server_url = config.server_url.clone();
    let token = config.token.clone();

    // 2. Build components
    let (agent_tx, agent_rx) = mpsc::channel(256);

    let pty_config = PtyConfig {
        working_dir: config.working_dir.clone(),
        allowed_commands: config.allowed_commands.clone(),
        scrollback_size: config.scrollback_size,
    };

    let pty_manager = Arc::new(PtyManager::new(agent_tx.clone(), pty_config));
    let ws_client = WsClient::new(server_url.clone(), token);
    let ws_connected = Arc::new(AtomicBool::new(false));

    eprintln!("  Server URL           {server_url}");
    eprintln!("  Working Directory    {}", config.working_dir.display());
    eprintln!("  Health Port          {health_port}");
    eprintln!(
        "  Allowed Commands     {}",
        config.allowed_commands.join(", ")
    );
    eprintln!();

    // 3. Start health check server
    tokio::spawn(health::run_health_server(
        health_port,
        ws_connected.clone(),
    ));

    eprintln!("  Ready — connecting to server.");
    eprintln!();

    // 4. Run forever (reconnects automatically on disconnection)
    ws_client
        .run_forever(ws_connected, pty_manager, agent_tx, agent_rx)
        .await;
}

async fn run_status() {
    let config = match ConnectConfig::load() {
        Ok(c) => c,
        Err(_) => {
            eprintln!();
            eprintln!("  No configuration found.");
            eprintln!("  Run 'trakkt-connect setup' for instructions.");
            eprintln!();
            std::process::exit(1);
        }
    };

    eprintln!();
    eprintln!("  Trakkt Connect Status");
    eprintln!("  ─────────────────────");
    eprintln!();

    // Config check
    eprint!("  Config         ");
    eprintln!("\x1b[32m\u{2713}\x1b[0m");

    // Token check
    eprint!("  Token          ");
    if config.token.starts_with("trakkt-") {
        eprintln!("\x1b[32m\u{2713}\x1b[0m  {}...", &config.token[..14.min(config.token.len())]);
    } else {
        eprintln!("\x1b[33m?\x1b[0m  (non-standard format)");
    }

    // Server URL
    eprintln!("  Server         {}", config.server_url);

    // Health check
    eprint!("  Health         ");
    let health_url = format!("http://127.0.0.1:{}/healthz", config.health_port);
    match reqwest_health_check(&health_url).await {
        Some(status) => {
            if status.ws_connected {
                eprintln!("\x1b[32m\u{2713}\x1b[0m  connected");
            } else {
                eprintln!("\x1b[33m!\x1b[0m  running but not connected to server");
            }
        }
        None => {
            eprintln!("\x1b[31m\u{2717}\x1b[0m  agent not running (port {})", config.health_port);
        }
    }

    eprintln!();
}

/// Minimal health check response shape.
#[derive(serde::Deserialize)]
struct HealthResponse {
    ws_connected: bool,
}

/// Hit the local health endpoint to check if the agent is running.
async fn reqwest_health_check(url: &str) -> Option<HealthResponse> {
    // Use a raw TCP connection to avoid pulling in reqwest as a dependency.
    // The health endpoint returns simple JSON.
    let addr: std::net::SocketAddr = {
        let url_parsed = url.strip_prefix("http://")?;
        let (host_port, _path) = url_parsed.split_once('/')?;
        host_port.parse().ok()?
    };

    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .ok()?
    .ok()?;

    // Send a minimal HTTP request
    let request = format!(
        "GET /healthz HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    );

    stream.writable().await.ok()?;
    stream.try_write(request.as_bytes()).ok()?;

    // Read the response
    let mut buf = vec![0u8; 4096];
    let mut total = 0;
    loop {
        stream.readable().await.ok()?;
        match stream.try_read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }
    }

    let response = std::str::from_utf8(&buf[..total]).ok()?;
    let body = response.split("\r\n\r\n").nth(1)?;
    serde_json::from_str(body).ok()
}

fn run_setup() {
    let config_dir = config_file::ConfigFile::default_config_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("~/.config/trakkt-connect"));
    let config_path = config_dir.join("config.toml");

    eprintln!();
    eprintln!("  Trakkt Connect Setup");
    eprintln!("  ────────────────────");
    eprintln!();
    eprintln!("  1. Create a config file at {}:", config_path.display());
    eprintln!();
    eprintln!("     token = \"trakkt-...\"");
    eprintln!("     server_url = \"wss://app.trakkt.dev/api/connect/ws\"");
    eprintln!("     working_dir = \"/home/user/projects\"");
    eprintln!();
    eprintln!("  2. Or set environment variables:");
    eprintln!();
    eprintln!("     TRAKKT_TOKEN=trakkt-...");
    eprintln!("     TRAKKT_SERVER_URL=wss://app.trakkt.dev/api/connect/ws");
    eprintln!("     TRAKKT_WORKING_DIR=/home/user/projects");
    eprintln!();
    eprintln!("  3. Run the agent:");
    eprintln!();
    eprintln!("     trakkt-connect run");
    eprintln!();
    eprintln!("  Optional settings:");
    eprintln!();
    eprintln!("     health_port = 9090           # Health check HTTP port");
    eprintln!("     scrollback_size = 65536      # Scrollback buffer size (bytes)");
    eprintln!("     allowed_commands = [\"claude\", \"bash\", \"zsh\", \"sh\", \"fish\"]");
    eprintln!();

    // Create the config directory if it doesn't exist so the user can
    // immediately drop a config file in.
    if let Ok(path) = config_file::ConfigFile::default_config_dir()
        && !path.exists()
    {
            if let Err(e) = std::fs::create_dir_all(&path) {
                eprintln!("  (Could not create config directory: {e})");
            } else {
                eprintln!("  Config directory created at {}", path.display());
                eprintln!();

                // Write an example config file if none exists
                let example = config_file::ConfigFile {
                    token: Some("trakkt-YOUR-TOKEN-HERE".into()),
                    server_url: Some("wss://app.trakkt.dev/ws/connect/agent".into()),
                    working_dir: None,
                    allowed_commands: None,
                    health_port: None,
                    scrollback_size: None,
                };
                match example.save_to(&path) {
                    Ok(p) => eprintln!("  Example config written to {}", p.display()),
                    Err(e) => eprintln!("  (Could not write example config: {e})"),
                }
                eprintln!();
            }
        }
}
