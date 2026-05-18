# Tane Coding Standards

Standards learned from code reviews. All implementers MUST follow these rules.

## Rust Patterns

### Database Queries
- Use `db_fetch_all!`, `db_fetch_one!`, `db_fetch_optional!`, `db_execute!`, `db_fetch_scalar!` macros for ALL database queries. Never use raw sqlx pool access.
- Always match on `DbPool::Postgres` and `DbPool::Sqlite` variants — never assume one backend.
- Always create SQLite migrations alongside Postgres migrations. Same schema changes, adapted for SQLite syntax (e.g. no ON DELETE CASCADE).
- Use `sql_compat` helpers (`now()`, `bool_true()`, `ilike()`, etc.) for dialect-dependent SQL fragments.
- Generate UUIDs as `Uuid::new_v4().to_string()` for all entity IDs.

### Service Layer
- All service functions are free functions with `db: &DbPool` as the first argument.
- Service functions return `trakkt_core::Result<T>`, never `ServerFnError`.
- Every write operation must append to `sync_log` after the main write.
- When changing a service function signature, grep for ALL call sites (server functions, websocket bootstrap, tests, other services).

### Server Functions (Leptos)
- Server functions are thin wrappers: extract auth, extract context, call service, return.
- No business logic in server functions — delegate to `trakkt-auth` services.
- Use `#[server(prefix = "/leptos-api")]` for all server functions.
- Return typed Rust enums (not HTTP status codes). UI pattern-matches on variants.

### Error Handling
- Never silently discard errors with `let _ =`. At minimum, log with `tracing::warn!`.
- Never use `unwrap_or_default()` on serialization or deserialization (JSON parse, `to_value`, filter decode, etc.) — use `match` and log the error with `tracing::warn!`.
- Service errors propagate as `trakkt_core::Error` variants.
- Server function errors convert via `IntoServerFnError` trait.

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
