# TRA-10098 browser fixture and joint release QA

The executable `github-connections.spec.ts` uses the actual server and SQLite.
It requires a disposable migrated personal-mode server, matching fresh WASM
assets, `BASE_URL` pointing to that localhost server, `TRAKKT_MODE=personal` and
`GITHUB_SETTINGS_E2E_DB` pointing to its isolated SQLite file. The personal
workspace must have no GitHub installations. Do not use a developer or live DB.
Configure the fixture server with an RSA GitHub App private key generated for
this test, arbitrary App/OAuth client IDs and secrets, and a localhost OAuth
callback. Settings and disconnect need this configuration but make no GitHub
API requests. Use the dedicated configuration (no development DB or auth setup) and run:

```
cd e2e
npx playwright test --config playwright.github-settings.config.ts
```

This checks two independently scoped cards, personal versus organization labels,
all repositories versus selected empty, Add after connecting, shared rules shown
once, per-card disconnect and retained reconnect, row isolation and durable
refetch in a second tab without navigation. It seeds display fixtures directly;
it does not claim to prove verified OAuth authorization. Runtime browser execution
is deferred to backlog-fast batch QA.

For joint authorization and multi-connection release QA against a test GitHub
App and disposable Trakkt workspaces:

1. Authorize a personal account then an organization in one workspace; repeat in
   reverse order in another isolated run. Verify two stable installation rows,
   accurate repository scope, Add still available and customized rules retained.
2. Reconnect each card through GitHub OAuth. Confirm it targets the selected row,
   does not replace the other identity, clears only its cached credential, and
   retains historical PR links. Test reinstall separately: a replacement GitHub
   installation creates a new immutable row while retaining old history.
3. Attempt foreign installation claims and forged/expired/replayed callback
   states. Confirm rejection leaves both workspaces' rows, rules and links intact.
4. Send signed PR/push fixtures for each account and merge PRs. Confirm only the
   destination workspace changes, shared rules apply, and outbound comments use
   the installation owning the event. Duplicate team keys elsewhere stay intact.
5. Disconnect one card with two tabs open. Verify the other tab updates without
   reloading and later events cannot change links/status/outbound actions for the
   disconnected account; the other remains functional. Suspend/unsuspend/uninstall
   that GitHub installation and verify unsuspend cannot undo local disconnect.
6. Change repository selection on GitHub, including selected empty. Verify both
   tabs refetch, only that card's scope and token cache change, and removed-repo
   events cannot mutate tickets. Force scope-refresh failure and confirm Settings
   shows repository access needs refresh with a targeted reconnect action; no
   events process until authoritative permissions are reconciled. Reconnect cannot
   enlarge verified permissions.
7. Retain legacy installation/link identity and customized rules across migration;
   legacy cards require verified authorization before automation resumes. Repair
   missed historical events only through the ticket's documented reconciliation
   path; reconnect alone cannot replay already-processed delivery IDs.
