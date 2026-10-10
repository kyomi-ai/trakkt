# Direct GitHub installation regression

Build the server binary and matching frontend assets from the change under test,
then run from `e2e`:

```
GITHUB_DIRECT_SERVER_BINARY=/absolute/path/to/trakkt \
GITHUB_DIRECT_DIST_DIR=/absolute/path/to/crates/trakkt-ui/dist \
node scripts/run-github-direct.cjs
```

The runner creates a temporary SQLite database, starts a SaaS server on localhost
port 3701 (3200 + TRA-10101’s 501 remainder) (override with `GITHUB_DIRECT_PORT`), runs Chromium tests and stops the
server and removes its database. It rejects an occupied port and passes only
fixture configuration to the server. It does not reuse a development database,
production credentials or an existing session. The server must apply migrations
on startup, as the normal executable does.

The sign-in case creates a disposable verified user with a real Argon2 password
and administrator workspace membership. It performs actual password login and
checks return to the direct-install landing, selected workspace and reload. It
also verifies that merely opening the landing creates neither an installation
nor an authorization state. Clicking Continue then calls the actual server,
creates fresh workspace/user-bound OAuth state and PKCE, and redirects to an
intercepted authorization page. The runner generates a temporary RSA key and
uses fake GitHub App/OAuth credentials; neither browser nor server contacts
GitHub during this test. An installation is still not linked before OAuth proof.

The other cases intercept server-function responses to exercise actual frontend
routing: member versus administrator selection, explicit Continue, and strict
state callback handling with URL scrubbing. Those cases prove UI behavior, not
provider ownership verification. They make no GitHub requests. The Rust service
tests separately exercise OAuth state, PKCE, authorization and binding against
controlled provider responses and both database backends.
