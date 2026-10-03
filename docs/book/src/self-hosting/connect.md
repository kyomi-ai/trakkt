# Connect terminal

Connect opens terminals on a computer running the Trakkt Connect agent. Open **Connect** in the sidebar after starting the agent. **Agent connected** means your agent is available; the browser's connection to Trakkt alone does not make an agent available.

## Install and configure the agent

Download `trakkt-connect-linux-x86_64.tar.gz` and `SHA256SUMS` from the same [Trakkt release](https://github.com/kyomi-ai/trakkt/releases). The supplied binary supports Linux x86_64 with glibc 2.35 or newer, including Ubuntu 22.04 and later.

```sh
sha256sum --check SHA256SUMS
tar -xzf trakkt-connect-linux-x86_64.tar.gz
./trakkt-connect --help
```

Run `./trakkt-connect setup` for the configuration location. Create `~/.config/trakkt-connect/config.toml` with an API token that has **write access** and belongs to the same workspace **and user** signed into the browser:

```toml
token = "trakkt-YOUR-TOKEN-HERE"
server_url = "wss://trakkt.app/ws/connect/agent"
working_dir = "/home/you/projects"
allowed_commands = ["sh", "bash", "claude"]
```

Replace `server_url` with your Trakkt host when self-hosting. Protect the token and start the agent:

```sh
chmod 600 ~/.config/trakkt-connect/config.toml
./trakkt-connect run
```

Commands run as the local OS user running the agent, with access to that user's files and tools. The allowlist controls executable names; allowing a shell permits commands entered into that shell. Install Claude locally if you want **New Claude session** to run it.

Environment variables override the configuration file: `TRAKKT_TOKEN`, `TRAKKT_SERVER_URL`, `TRAKKT_WORKING_DIR`, `TRAKKT_ALLOWED_COMMANDS`, `TRAKKT_HEALTH_PORT`, and `TRAKKT_SCROLLBACK_SIZE`. `./trakkt-connect status` checks the agent's local health endpoint, normally port 9090.

## Use sessions

- **New shell session** starts `sh` in the configured working directory. **New Claude session** starts `claude`. An unavailable or disallowed command produces a visible error.
- Select a session tab to switch terminals. Each tab keeps its screen, terminal modes, and scrollback. Keyboard input and pasted text go to the selected live session.
- **Enter fullscreen** expands the terminal pane; **Exit fullscreen** returns to the normal layout. Resizing updates the underlying PTY. Fullscreen command-line programs use a separate terminal screen and return to the shell's output when they exit.
- Scroll upward to read earlier output. Output follows the bottom while you remain there.
- Close a live tab to force-stop that session; its tab stays until the agent acknowledges the exit. Reconnect a disconnected agent before closing a live session. Exit and spawn-failure messages keep the tab and its output available until you close it.

Refreshing the browser restores live sessions from the agent and requests their bounded scrollback. Network reconnects resubscribe automatically. Existing output remains readable while the agent is disconnected, and new-session controls stay disabled. Stopping the agent stops its local PTYs; an exited session's retained browser output is not persisted across a page refresh.

If the agent remains disconnected, check its startup output, server URL, token, and workspace/user identity. If a session cannot start, check the executable is installed and appears in `allowed_commands`.
