# Direct GitHub installation regression

Build the server binary and matching frontend assets from the change under test,
then run from `e2e`:

```
GITHUB_DIRECT_SERVER_BINARY=/absolute/path/to/trakkt \
GITHUB_DIRECT_DIST_DIR=/absolute/path/to/crates/trakkt-ui/dist \
node scripts/run-github-direct.cjs
```

The runner creates a temporary SQLite database, starts a SaaS server on localhost
port 3702 (3200 + TRA-10102’s 502 remainder) (override with `GITHUB_DIRECT_PORT`), runs Chromium tests and stops the
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

The routing-only cases intercept server-function responses to exercise actual frontend
routing: member versus administrator selection, explicit Continue, and strict
state callback handling with URL scrubbing. Those cases prove UI behavior, not
provider ownership verification. They make no GitHub requests. The Rust service
tests separately exercise OAuth state, PKCE, authorization and binding against
controlled provider responses and both database backends.

The inactive-removal case uses real password login and actual settings server
functions against migrated SQLite. It seeds an uninstalled older generation and
a disconnected replacement for the same stable account, plus two distinct
accounts. It checks confirmation cancellation, removal without reload, persisted
absence after reload, retained installation records, ticket/PR links, events,
rules and permanent ownership claims. An active account still uses Disconnect
and becomes removable without being archived by disconnection. Fresh Add account
then calls the real server and records the removed installation's generation in
workspace/user-bound setup state. Its GitHub installation landing is intercepted;
the account remains archived until provider proof. Removal success is never
mocked. This browser fixture covers SQLite; service tests cover both
SQLite and PostgreSQL and controlled authorization races.
