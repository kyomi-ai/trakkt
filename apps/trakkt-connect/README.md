# Trakkt Connect agent

The agent runs on your own machine and opens an outbound WebSocket connection
so you can use its terminal sessions from Trakkt. Commands execute locally under
the agent's OS user. Install and run it as a user whose files and tools you intend
to access; the command allowlist restricts executable names, not shell arguments.

The release asset `trakkt-connect-linux-x86_64.tar.gz` supports Linux x86_64 with
glibc 2.35 or newer (Ubuntu 22.04 or newer). macOS, Windows, ARM and musl binaries
are not included. Download it and `SHA256SUMS` from the same GitHub release:

```sh
sha256sum --check SHA256SUMS
tar -xzf trakkt-connect-linux-x86_64.tar.gz
./trakkt-connect --help
```

Create `~/.config/trakkt-connect/config.toml` (or the platform configuration
folder shown by `trakkt-connect setup`) with permissions `0600`:

```toml
token = "trakkt-YOUR-TOKEN-HERE"
server_url = "wss://trakkt.app/ws/connect/agent"
working_dir = "/home/you/projects"
allowed_commands = ["bash", "sh", "claude"]
```

The API token requires write access and must belong to the same active workspace
and user as the browser session. Keep the token private. Environment variables override configuration:
`TRAKKT_TOKEN`, `TRAKKT_SERVER_URL`, `TRAKKT_WORKING_DIR`,
`TRAKKT_ALLOWED_COMMANDS`, `TRAKKT_HEALTH_PORT`, `TRAKKT_SCROLLBACK_SIZE`.

```sh
./trakkt-connect run
./trakkt-connect status
```

`/healthz` on port 9090 reports WebSocket connection status. PTYs and their
bounded scrollback remain on the agent during reconnects or server restarts;
exit events remove sessions. Reopen a terminal to fetch its current scrollback.
SaaS runs one server relay instance; a server deployment briefly interrupts the
UI and WebSockets while the replacement becomes ready.
