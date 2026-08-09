# Tane Coding Standards

Standards learned from code reviews. All implementers MUST follow these rules.

## Rust Patterns

### Database Queries
- Use `db_fetch_all!`, `db_fetch_one!`, `db_fetch_optional!`, `db_execute!`, `db_fetch_scalar!` macros for ALL database queries. Never use raw sqlx pool access.
- Always match on `DbPool::Postgres` and `DbPool::Sqlite` variants — never assume one backend.
- Always create SQLite migrations alongside Postgres migrations. Same schema changes, adapted for SQLite syntax (e.g. no ON DELETE CASCADE).
- Use `sql_compat` helpers (`now()`, `bool_true()`, `ilike()`, etc.) for dialect-dependent SQL fragments.
- Generate UUIDs as `Uuid::new_v4().to_string()` for all entity IDs.
- JSONB columns (e.g. `settings`) must use `CAST(col AS TEXT) AS col` in every SELECT query. The Rust row type declares `Option<String>`, which sqlx decodes as TEXT — Postgres JSONB is not compatible with TEXT without the explicit cast. NULL values happen to work without the cast, masking the bug until real data is written.

### Service Layer
- All service functions are free functions with `db: &DbPool` as the first argument.
- Service functions return `trakkt_core::Result<T>`, never `ServerFnError`.
- Every write operation must append to `sync_log` **in the same transaction as** the main write, so the row and the sync entry that carries it to other clients commit or roll back together. A write that commits without its entry is missing from the delta stream other clients replay; one that rolls back after its entry was written announces a change that never happened. See `### Transactions` below for the rules that apply while that transaction is open.
- When changing a service function signature, grep for ALL call sites (server functions, websocket bootstrap, tests, other services).

### Transactions
- Never call a pool-scoped macro (`db_fetch_*!`, `db_execute!`, `db_with_pool!`) or a `&DbPool`-taking helper between `db.begin()` and the matching commit. SQLite runs with `max_connections(1)`, so the open transaction holds the only connection — the pool call stalls until sqlx's 30s `acquire_timeout` fires and then fails with `PoolTimedOut`. Use the `tx_*!` macros or a `_tx` helper variant instead. Do all authorization reads and validation on the pool *before* `db.begin()`. Enforced in CI by the `Transaction Pool Safety` job — run `scripts/check-tx-pool.py` locally before pushing. Its header records what it deliberately cannot detect (cross-crate calls, and helpers that reach the pool without naming `&DbPool` in their signature), so a green run is not a proof of absence.
- Route every commit through `SyncBatch`/`commit_and_deliver` rather than hand-rolling commit-then-broadcast. One `SyncAudience` value must drive both the persisted `visibility_user_id` column and the delivery call, so "persist private but broadcast workspace-wide" is unrepresentable.
- The payload persisted to `sync_log.data` must be the same value that is broadcast. Passing `None` to the persisted column while broadcasting a real payload silently drops the entity from every client's delta replay — `cache/apply.rs` discards data-less insert/update actions before they reach IndexedDB.

### Server Functions (Leptos)
- Server functions are thin wrappers: extract auth, extract context, call service, return.
- No business logic in server functions — delegate to `trakkt-auth` services.
- Use `#[server(prefix = "/leptos-api")]` for all server functions.
- Return typed Rust enums (not HTTP status codes). UI pattern-matches on variants.

### Concurrent Inserts
- All INSERT statements for user-scoped or entity-scoped records must use `ON CONFLICT DO NOTHING` (Postgres) or `INSERT OR IGNORE` (SQLite). Check-then-insert patterns without ON CONFLICT are race conditions — concurrent requests can hit a UNIQUE constraint violation between the check and the insert. This applies to favorites, watchers, preferences, release_issues, and any table with a composite PK or UNIQUE constraint.

### Error Handling
- Never silently discard errors with `let _ =`. At minimum, log with `tracing::warn!`.
- Never use `unwrap_or_default()` on serialization or deserialization (JSON parse, `to_value`, filter decode, etc.) — use `match` and log the error with `tracing::warn!`.
- Service errors propagate as `trakkt_core::Error` variants.
- Server function errors convert via `IntoServerFnError` trait.

### Double-Option (Clearable Fields)
- `IssueUpdate` uses double-Option for clearable fields (`Option<Option<T>>`). `Some(None)` means "clear the field" (set to NULL), not "set the field". When checking if a field was set to a value, use `matches!(field, Some(Some(_)))`, not `.is_some()` — the latter fires on field-clear too.

### Comment and Doc Accuracy
- A comment that makes a checkable claim must be checked. Reviews repeatedly find comments asserting a consequence that does not hold ("would hang forever" when the real outcome is a 30s `PoolTimedOut`; "replaces 11 copy-pasted blocks" when it was 10; a `Drop` guard "covers panics" when `panic = "abort"` is set in the release profile). If you cannot verify a claim, state the narrower thing you can verify.
- Prefer documenting the invariant a caller depends on at the callee, not only at the caller. A guarantee recorded only at the call site is silently invalidated the next time the callee is edited in isolation.

### No Banned Patterns
- No `#[allow(dead_code)]`, `#[allow(unused_variables)]`, `#[allow(unused_imports)]`
- No `closure.forget()` on persistent listeners
- No hardcoded `"unknown"` IP addresses
- No duplicated helper functions across modules

## The Clippy Gate

### Run the same commands CI runs

CI's `Clippy` job runs exactly these two commands, and nothing else:

```
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked -p trakkt-ui --target wasm32-unknown-unknown --features hydrate --all-targets -- -D warnings
```

Run both locally before pushing. The second needs the wasm target installed once: `rustup target add wasm32-unknown-unknown`.

`--all-targets` is load-bearing, not decoration. Without it clippy lints only the `lib`/`bin` targets, which are compiled *without* `cfg(test)` — so `#[cfg(test)]` modules, `tests/`, benches and examples are never linted at all. Everything in this document that clippy can enforce was unenforced in test code until TRA-9956 added the flag, and 108 violations had accumulated behind it: 106 `unwrap_used`, one `type_complexity`, one `dead_code`.

It is close to free. Measured locally cold — `cargo clean` plus a private empty `SCCACHE_DIR` per run, so both start at a 0% cache hit rate — the two commands total 318s with the flag and 318s without (201s+117s vs 206s+112s). Roughly 700 dependency crates dominate the build and both forms compile them identically; `--all-targets` adds 18 compile units (+1.2%), all of them workspace test targets.

The two commands are not redundant. The first compiles for the host, where `cfg(target_arch = "wasm32")` is false, so it cannot see the `#[cfg(all(test, target_arch = "wasm32"))]` `wasm-bindgen-test` modules in `trakkt-ui`. Only the second lints those. `trakkt-ui` declares no `[[bin]]`, `[[test]]`, `[[example]]` or `[[bench]]` targets and `crates/trakkt-ui/tests/` holds only Playwright JS, so `--all-targets` and `--tests` cover the same ground there today; `--all-targets` is used for symmetry with the host command and so a future Rust test or example target is covered without a second edit.

Clippy type-checks but never *runs* tests, so neither command needs a live Redis on :6381 — the `redis::tests::test_redis_connects` caveat applies to `cargo test`, not here.

### `unwrap()` in tests

`clippy::unwrap_used` is `warn` at the workspace level (`Cargo.toml`, `[workspace.lints.clippy]`) and `-D warnings` promotes it to an error. It applies in test code on exactly the same terms as production code — there is no `#[cfg(test)]` carve-out, and adding one would recreate the gap the gate exists to close.

Use `expect()` / `expect_err()` and make the message say **what was being attempted**, so a failing test names the belief that turned out false rather than only a line number:

```rust
let encrypted = encrypt(plaintext, &key).expect("encrypting an ASCII password with the test key");
let err = parse_action_type("upsert").expect_err("parsing the unrecognised action type \"upsert\" must be rejected");
```

Filler messages — `expect("unwrap")`, `expect("should work")`, `expect("failed")` — pass the lint and defeat its point. If you cannot say what was expected, read the test until you can.

### No suppressions

Do not silence a clippy finding with `#[allow(...)]`, and never with a crate-level `#![allow(clippy::unwrap_used)]`. The `Lint Suppression Policy` job fails any PR whose diff adds `#[allow(` to a `.rs` file or `= "allow"` to a `Cargo.toml`. Fix the finding instead: `clippy::type_complexity` wants a `type` alias, `dead_code` wants the field genuinely read or removed.

### Rollback tests for sync_log writes

A converted write needs a test proving the mutation rolls back when its `sync_log`
insert fails. Inject the failure with a real trigger on `sync_log` — no mocks, no
`#[cfg(test)]` branch in production control flow — and assert **both** an error
return and that prior state is intact, compared as ordered `Vec`s.

Two helpers exist in `crates/trakkt-core/src/test_helpers/dual_backend.rs`:

- `reject_sync_log_inserts` — a blanket trigger. Correct only for a mutation that
  writes exactly one entry.
- `reject_sync_log_inserts_of_type` — narrows with a `WHEN NEW.entity_type = …`
  clause so entries written *before* the probed type are accepted. Pair with
  `clear_sync_log_rejection` to probe several types against one database.

A blanket trigger on a function that writes several entries aborts the *first* one,
so the assertion passes without the code under test ever being reached. TRA-9950 hit
this in `notification_service::update_preference`, where `get_or_default_preferences`
emits an `Insert` first; TRA-9971's cascade had the same shape across four loops.

Narrowing by entity type does not discriminate when every entry a function writes
shares one type — a bulk mark-read writing N `NOTIFICATION` entries is the case.
There, seed the prior state and let it commit *before* installing the trigger, so the
trigger only sees the entries the function under test writes. Say which of the two
you used and why; a rollback test that cannot fail is worse than none.

Mutation-test each converted function individually and report per function. A sweep
that reports one aggregate result cannot distinguish "all six covered" from "one
covered five times".

A new rollback test goes beside its siblings in
`crates/trakkt-auth/src/sync_log_service.rs`, on SQLite. It belongs in the
dialect suite instead only if it asserts something the two dialects can still
disagree about, which in practice means a rollback whose *extent* is decided by
`ON DELETE` actions rather than by the code. The section of
`apps/server/tests/postgres_dialect.rs` headed "The 60 SQLite-only rollback
tests" records where that line was drawn, which three shapes are already run on
Postgres, and the candidate that was written and then discarded for duplicating
one of them.

## The Postgres dialect suite

Production runs Postgres. Every test in the workspace except this suite runs
SQLite, so a defect confined to an `is_pg` query arm — a wrong placeholder
index, a missing cast, `RETURNING` versus `last_insert_rowid()` — compiles,
passes clippy, passes `cargo test`, and ships without anything having executed
it. `sort_order` decoded as `f64` from a `FLOAT4` column shipped exactly that way
twice.

The suite lives in `apps/server/tests/postgres_dialect.rs` and its harness in
`crates/trakkt-core/src/test_helpers/dual_backend.rs`.

### What it covers, and what it does not

Read this before treating a green `Postgres Dialect Tests` job as a statement
about the Postgres arms in general. It is not one. It says the bodies in that
file ran, and nothing about the arms none of them reaches.

As of TRA-10001 the file holds 25 `dual_backend_test!` bodies — 50 tests, one
pair each — plus three Postgres-only tests that need both backends open at once
and one SQLite-only test about SQLite's rowid rule. What those bodies execute:

- the six `tx_*` macros, and `write_sync_entry_in_tx`'s `RETURNING sync_id`;
- the rollback contract, in all three of its shapes — a mutation's first entry
  rejected, an entry rejected after an earlier one was accepted, and an entry
  rejected after the statement has already fired `ON DELETE CASCADE`;
- the eight `sql_compat` helpers production builds SQL with (see the ledger at
  the head of that file's `sql_compat` section, which also names the twelve it
  does not and why);
- the `sort_order` decode — the FLOAT4-versus-`f64` class that shipped twice —
  across all five columns that carry the name;
- schema parity: foreign keys and their `ON DELETE`, primary-key nullability,
  and the runtime behaviour of the four keys whose actions used to differ;
- the migration chain itself, on both dialects.

What it does not cover, stated so nobody has to infer it:

- **Most `is_pg` branch points.** There are 119 `is_postgres()` call sites
  outside `apps/server/tests`, and 164 `sql_compat::` call sites. The suite
  reaches a minority of them, because each body is written for a defect class
  rather than swept over a list.
- **All 60 rollback tests** in `crates/trakkt-auth/src/sync_log_service.rs`,
  which stay on SQLite. TRA-10001 converted none of them and recorded why in
  that file's own rollback-decision section: 52 install the blanket trigger and
  8 narrow it, so between them they exercise two of the three shapes the suite
  already runs on Postgres, and all 60 hang off a fixture that hardcodes
  `DbPool::connect("sqlite::memory:")` and leans on SQLite column defaults.
- **`trakkt-auth`'s 178 `#[tokio::test]`s as a whole**, which open an in-memory
  SQLite pool and are the workspace's main body of service-layer testing.
- **Twelve `sql_compat` helpers with no production caller** — a deletion to
  schedule, not a testing gap.

Adding a body is how that list shrinks. Do not let it shrink by editing the
list: if this section reads as broader than the file, that is a defect in
whichever change made it so.

### Running it locally

Start the test-rung Postgres once (5436 is Trakkt's test port; 5435 is Trakkt's
development database and 5433/5434 belong to another project):

```
podman run -d --name trakkt-postgres-test -p 5436:5432 \
    -e POSTGRES_USER=trakkt -e POSTGRES_PASSWORD=trakkt -e POSTGRES_DB=postgres \
    docker.io/library/postgres:16
```

Then:

```
cargo test -p trakkt-server --test postgres_dialect -- --include-ignored
```

`--include-ignored` is what runs the Postgres halves. Without it you get the
SQLite halves only. Set `TEST_DATABASE_URL` to point somewhere else; it is used
only to `CREATE` and `DROP` a throwaway `trakkt_test_*` database per test, and no
table in the database it names is read or written.

### What happens with no Postgres running

`cargo test` — with or without `--workspace` — still works, and nothing about
the SQLite path becomes prerequisite-heavy. Each test is declared once with
`dual_backend_test!` and expands into a pair:

```
test sync_entry_id_addresses_the_committed_row::sqlite   ... ok
test sync_entry_id_addresses_the_committed_row::postgres ... ignored, requires a live Postgres — see crates/trakkt-core/src/test_helpers/dual_backend.rs
```

The SQLite half runs and passes. The Postgres half reports `ignored`, and the
summary reports a non-zero `N ignored`. That count is the suite stating plainly
that nothing was verified on Postgres.

It is `#[ignore]` and not a silent skip on purpose, for the reason
`crates/trakkt-core/src/redis.rs` already records: a harness that catches the
connection error and returns early prints `ok`, which is indistinguishable from
a run where Postgres was present and the code genuinely worked. Every machine
without Postgres would then report success for a path it never touched — the
exact failure mode this suite exists to remove. Do not "improve" it into a
silent skip.

Asked to run anyway (`--include-ignored`) with no server reachable, the harness
**fails** with instructions rather than skipping. `ignored` never means "passed"
and an unreachable database never means "fine".

### Adding a test

Write one body; the macro runs it on both backends:

```rust
dual_backend_test! {
    /// What the assertion is for.
    async fn a_committed_row_is_readable(db) {
        // `db` is a `&DbPool` — Postgres in one run, SQLite in the other.
    }
}
```

A single shared body is the point. Two files, one per backend, would answer
"does some Postgres test exist"; only a shared body keeps answering whether the
*same* assertion holds on both after someone edits one half.

Assert on decoded values, not on bytes. Postgres stores JSONB parsed and
re-serialises it, so a payload written as `{"a":1}` reads back as `{"a": 1}`
while SQLite returns the TEXT verbatim — a byte comparison fails on Postgres for
no defect at all.

### In CI

The `Postgres Dialect Tests` job runs a `postgres:16` service container on 5436
— the same port the local default names, so the default is the configuration CI
exercises rather than an untested fallback. It runs with `--include-ignored` and
then asserts the summary line reports `0 ignored`: in CI a database is present,
so `ignored` would mean a Postgres arm quietly not exercised on a PR about to
merge.

## Leptos / Frontend

### Component Patterns
- Use Leptos components (`<Button>`, `<Modal>`), never raw HTML elements for styled components.
- Styles live in the component definition, not in the caller.
- Pass `variant`, `size`, and optional `class` props — don't inline Tailwind in callers.

### Layout
- Never put `overflow-x-auto` on containers that have absolutely-positioned dropdown children — the overflow clipping hides the dropdown. Use `flex-wrap` or move the dropdown to a portal.

### Reactive Primitives
- Never create `signal()`, `RwSignal::new()`, or `Effect::new()` inside reactive rendering closures (`move || { ... }`). They reset on every re-render and leak. Hoist all reactive primitives to component setup level (outside the view closure).

### WASM Browser Tests
- A `wasm-bindgen-test` that constructs an `Effect`, `Resource` or `LocalResource` must call `crate::wasm_test_support::boot_leptos_executor()` before it does so. All three spawn the moment they are constructed, and `any_spawner`'s executor is global and initialized once per test binary — in production by `mount_to`, which a test never calls. Without the explicit call the test passes only when some earlier test in the binary happened to initialize it first. That is an ordering dependency, it stays green until a runner picks a different order, and it has done exactly that twice.
- Enforced in CI by the `WASM Browser Tests` job, which runs the suite as one binary and then once per module in isolation — run `scripts/wasm-test-isolated.sh` locally before pushing. It derives the module list from the source tree, so a new module is swept without anyone editing a list, and it fails rather than passing quietly when a module's filter selects no test or a test is selected by no module. What it cannot see is an ordering dependency *within* one module, since a module is still run as a whole.

### SSR / Hydration
- Never use `Resource::new()` inside `#[cfg(target_arch = "wasm32")]` blocks (desyncs hydration IDs).
- Gate server-only code with `#[cfg(feature = "ssr")]`.
- Gate client-only code with `#[cfg(target_arch = "wasm32")]`.

### Dynamic Query Builders
- When binding values dynamically in Postgres, CAST integer/bigint parameters explicitly: `CAST($N AS INTEGER)` or `CAST($N AS BIGINT)`. Postgres rejects TEXT binds for integer columns.
- Inline LIMIT/OFFSET values directly (they are sanitized i64, not user input). Postgres rejects TEXT binds for LIMIT/OFFSET.
- Never use `Vec<String>` as a Leptos server function input parameter — URL encoding doesn't support it. Use a comma-separated `Option<String>` and parse server-side.

### WebSocket / Sync
- Service functions that write data need `ws_manager: Option<&WebSocketManager>` to broadcast changes. The Option allows calling without a manager in tests.
- Every write to sync_log should also broadcast the SyncAction to the workspace via ws_manager.
- Client-side IndexedDB operations must be wrapped in `spawn_local` — they are async but not Send.
- Wrap non-Send types (like IDB handles) in `SendWrapper` for use with Leptos signals.

## Design System Adherence
- Root font size is 15px — all rem calculations must account for this.
- Primary accent: teal #0D9488 (NOT amber).
- All interactive elements need `transition-colors` with `duration-200`.
- All interactive elements need `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring`.
- Use design tokens (`text-success-foreground`, `text-error-foreground`, `text-muted-foreground`) instead of hardcoded Tailwind color classes (`text-green-600`, `text-red-600`). Only use raw color classes when no design token exists.
- All `<a>` tags with `target="_blank"` must include `rel="noopener noreferrer"`.

## CI / Security Scanning
- Test fixtures must not contain strings matching real secret patterns (e.g. `xoxb-`, `xoxp-`, `AKIA`). Trivy's secret scanner cannot distinguish `#[cfg(test)]` from production code. Use obviously-fake prefixes like `slack-bot-`, `test-key-` instead.
