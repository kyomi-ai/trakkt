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
- Every write operation must append to `sync_log` after the main write.
- When changing a service function signature, grep for ALL call sites (server functions, websocket bootstrap, tests, other services).

### Transactions
- Never call a pool-scoped macro (`db_fetch_*!`, `db_execute!`, `db_with_pool!`) or a `&DbPool`-taking helper between `db.begin()` and the matching commit. SQLite runs with `max_connections(1)`, so the open transaction holds the only connection — the pool call stalls until sqlx's 30s `acquire_timeout` fires and then fails with `PoolTimedOut`. Use the `tx_*!` macros or a `_tx` helper variant instead. Do all authorization reads and validation on the pool *before* `db.begin()`.
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

## Leptos / Frontend

### Component Patterns
- Use Leptos components (`<Button>`, `<Modal>`), never raw HTML elements for styled components.
- Styles live in the component definition, not in the caller.
- Pass `variant`, `size`, and optional `class` props — don't inline Tailwind in callers.

### Layout
- Never put `overflow-x-auto` on containers that have absolutely-positioned dropdown children — the overflow clipping hides the dropdown. Use `flex-wrap` or move the dropdown to a portal.

### Reactive Primitives
- Never create `signal()`, `RwSignal::new()`, or `Effect::new()` inside reactive rendering closures (`move || { ... }`). They reset on every re-render and leak. Hoist all reactive primitives to component setup level (outside the view closure).

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
