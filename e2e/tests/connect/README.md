# Connect acceptance

This suite starts the actual Trakkt server and Linux Connect agent. It uses port
3441 for the server and 3442 for agent health, refusing occupied ports. It creates
a fresh temporary SQLite database in personal mode and does not load `.env`,
the normal E2E authentication state, or inherited service credentials. Processes,
their PTY descendants, and temporary data are cleaned up; server/agent logs are
attached to the Playwright result.

Build the server, agent, and hydrated UI from the changes being tested through
the repository's build queue before running this command. The harness requires
explicit artifact paths and never starts a build itself:

```sh
cd e2e
npm ci
CONNECT_SERVER_BINARY=/absolute/path/to/trakkt-server \
CONNECT_AGENT_BINARY=/absolute/path/to/trakkt-connect \
CONNECT_DIST_DIR=/absolute/path/to/trakkt-ui/dist \
npx playwright test --config playwright.connect.config.ts
```

Collection alone does not require binaries or a database:

```sh
npx playwright test --config playwright.connect.config.ts --list
```

The shell interaction checks keyboard input, a browser paste event, actual kernel
PTY dimensions after resize, alternate-screen escapes, fullscreen pane controls,
session switching, browser reconnect after an offline outage, scrollback after
reload, kill, retained exit status, and agent disconnect. The outage blocks new
connections and closes the existing native terminal WebSocket; reconnection uses
the application's normal path. Claude spawn failure uses the real agent's command allowlist; Claude
itself is not required. Personal mode deliberately bypasses application login,
so this suite does not claim authentication coverage.

The **Realtime E2E** workflow runs this suite after its two-client sync checks,
reusing the server and frontend it already built. A reusable Connect Agent Build
job supplies the Linux customer release package from the same workflow run;
the download fails on a digest mismatch and verifies `SHA256SUMS` before
extraction. The fixture still starts its own server on 3441, agent on 3442,
and temporary personal-mode SQLite database, isolated from the workflow's
existing server on 3100 and Postgres service. Failures upload the Connect
Playwright traces and attached fixture logs with the existing E2E artifacts.
No Claude installation or model request is needed.

A failed Connect run also saves the built server and WASM bundle as
`connect-acceptance-runtime`, so the exact failed runtime can be reproduced
without rebuilding it locally.
