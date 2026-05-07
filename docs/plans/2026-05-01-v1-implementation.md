# Tane V1 Implementation Plan

**Date:** 2026-05-01
**Goal:** Working issue tracker with auth, issues CRUD, list/detail/board views, and full design system.

## Reference

- Architecture: `docs/ARCHITECTURE.md`
- Design System: `DESIGN.md`
- Kyomi source (copy patterns from): `/home/jason/repos/kyomi`

## Strategy

Copy Kyomi's proven patterns (dual DB, KV store, auth, server functions, Leptos setup, Trunk build, Tailwind config). Adapt for Trakkt's simpler data model. Postgres is first-class (SaaS deployment on K8s). SQLite supported for self-hosted mode.

## Tasks

Tasks are ordered by dependency. Tasks within the same group can be parallelized.

---

### GROUP 1: Foundation (parallel)

#### Task 1: Cargo Workspace + trakkt-core

Create the Cargo workspace root and the `trakkt-core` crate with database abstraction, KV store, config, and error types.

**Files to create:**

1. `/Cargo.toml` — workspace root
   - Members: `crates/trakkt-core`, `crates/trakkt-auth`, `crates/trakkt-types`, `crates/trakkt-ui`, `apps/server`
   - Workspace dependencies (pinned versions): axum 0.8, tower, tower-http, sqlx 0.8 (postgres+sqlite+runtime-tokio), jsonwebtoken, argon2, serde, serde_json, tokio, tracing, tracing-subscriber, uuid, chrono, rand, base64, reqwest, redis, dashmap, async-trait, thiserror, leptos 0.8, leptos_router, leptos_meta, leptos_axum, phosphor-leptos, web-sys, wasm-bindgen, js-sys, send_wrapper, gloo-timers, indexed_db_futures, rust-embed, any_spawner
   - Profiles: dev, release (opt-level=z, lto=true, codegen-units=1, strip=true, panic=abort), dev-server (inherits dev, opt-level=1), wasm-dev (inherits dev, opt-level=s)

2. `crates/trakkt-core/Cargo.toml` — dependencies: sqlx, redis, dashmap, tokio, tracing, serde, serde_json, uuid, chrono, async-trait, thiserror, rand, base64, argon2

3. `crates/trakkt-core/src/lib.rs` — public modules: config, db, error, kv_store, kv_store_memory, kv_store_redis, sql_compat. Re-export Config, DbPool, Error, Result, KVPool, KVStore.

4. `crates/trakkt-core/src/db.rs` — Copy Kyomi's DbPool enum pattern:
   - `DbPool::Postgres(PgPool)` / `DbPool::Sqlite(SqlitePool)`
   - `connect(url)` — auto-detect from URL prefix, run migrations, configure pools (Postgres: max 10, SQLite: max 1 + WAL + foreign_keys)
   - `is_postgres()`, `is_sqlite()`, `pg_pool()`, `ping()`
   - `DbQueryResult` wrapper
   - Macros: `db_fetch_all!`, `db_fetch_one!`, `db_fetch_optional!`, `db_execute!`, `db_fetch_scalar!`, `db_with_pool!`
   - Migration paths: `../../apps/server/migrations` (Postgres), `../../apps/server/migrations-sqlite` (SQLite)

5. `crates/trakkt-core/src/kv_store.rs` — Copy Kyomi's KVStore trait:
   - Methods: set, get, del, getdel, incr, expire, sadd, srem, smembers, sdel, ping
   - `KVPool = Arc<dyn KVStore>`
   - `create_kv_store(redis_url: Option<&str>)` factory

6. `crates/trakkt-core/src/kv_store_redis.rs` — RedisKVStore using redis::aio::ConnectionManager

7. `crates/trakkt-core/src/kv_store_memory.rs` — InMemoryKVStore with RwLock<HashMap>, 30-second expiry sweep

8. `crates/trakkt-core/src/config.rs` — Config struct:
   - `database_url` (required)
   - `redis_url` (optional)
   - `jwt_secret` (required)
   - `encryption_key` (required, base64url 32 bytes)
   - `port` (default 3000)
   - `frontend_url` (default http://localhost:3000)
   - `base_url` (default http://localhost:3000)
   - `passkeys_enabled` (default true)
   - `password_auth_enabled` (default true)
   - `webauthn_rp_id` / `webauthn_rp_name` (inferred from frontend_url)
   - `self_hosted: bool` (derived: no DATABASE_URL means SQLite auto-config)
   - `Config::from_env()` entry point
   - `Config::test_config()` for tests

9. `crates/trakkt-core/src/error.rs` — Error enum:
   - NotFound, Unauthorized, Forbidden, BadRequest, Conflict, TooManyRequests, NotImplemented, ServiceUnavailable, Internal, Sqlx, Migrate, Redis, SerdeJson
   - `is_transient()` method
   - `IntoResponse` impl for Axum

10. `crates/trakkt-core/src/sql_compat.rs` — Copy essential helpers:
    - `now()`, `bool_true()`, `bool_false()`, `ilike()`, `cast_to_text()`, `coalesce_now()`, `json_extract_text()`

**Reference files in Kyomi:**
- `/home/jason/repos/kyomi/Cargo.toml`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/db.rs`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/kv_store.rs`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/kv_store_redis.rs`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/kv_store_memory.rs`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/config.rs`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/error.rs`
- `/home/jason/repos/kyomi/crates/kyomi-core/src/sql_compat.rs`

---

#### Task 2: trakkt-types

Shared DTOs and enums, WASM-safe (no server dependencies).

**Files to create:**

1. `crates/trakkt-types/Cargo.toml` — dependencies: serde, serde_json, chrono (optional, not on wasm)

2. `crates/trakkt-types/src/lib.rs` — module declarations + re-exports

3. `crates/trakkt-types/src/enums.rs`:
   - `IssueStatus` — Backlog, Todo, InProgress, Done, Cancelled. Serialize as lowercase snake_case. Display, ordering.
   - `Priority` — None=0, Urgent=1, High=2, Medium=3, Low=4. Serialize as integer. Display names.
   - `WorkspaceRole` — Owner, Admin, Member. Serialize as lowercase.

4. `crates/trakkt-types/src/models.rs` — DTOs (all Serialize + Deserialize + Clone + Debug):
   - `Workspace { workspace_id, name, slug, created_at, updated_at }`
   - `User { user_id, email, display_name, avatar_url, created_at }`
   - `WorkspaceMember { workspace_id, user_id, role, joined_at, display_name, email }`
   - `Issue { issue_id, workspace_id, number, title, description, status, priority, assignee_id, creator_id, due_date, created_at, updated_at }`
   - `IssueWithDetails { issue, labels, assignee_name, creator_name }` (for list/detail views)
   - `Label { label_id, workspace_id, name, color, created_at }`
   - `Comment { comment_id, issue_id, user_id, body, parent_id, created_at, updated_at, author_name, author_avatar }`
   - `Notification { notification_id, workspace_id, user_id, issue_id, notification_type, read, created_at, issue_title, issue_number }`
   - `ApiToken { token_id, workspace_id, user_id, name, token_prefix, scopes, last_used_at, expires_at, created_at }`

5. `crates/trakkt-types/src/sync.rs` — Sync protocol types (copy from Kyomi):
   - `SyncRequest` enum: SyncBootstrap, SyncDelta { last_sync_id }
   - `SyncResponse` enum: SyncAction, SyncComplete { last_sync_id }, SyncReset
   - `SyncAction { sync_id, entity_type, entity_id, workspace_id, action, data, timestamp }`
   - `SyncActionType` enum: Insert, Update, Delete
   - Entity type constants: ISSUE, COMMENT, LABEL, NOTIFICATION

**Reference:** `/home/jason/repos/kyomi/crates/kyomi-types/src/`

---

#### Task 3: trakkt-ui Scaffold + Design System

Set up the Leptos frontend with Trunk, Tailwind CSS 4, and the full Tane design system.

**Files to create:**

1. `crates/trakkt-ui/Cargo.toml`:
   - Features: `ssr` (enables leptos_axum, server-side deps), `hydrate` (enables leptos hydrate+csr)
   - Dependencies: leptos, leptos_router, leptos_meta, phosphor-leptos, web-sys, wasm-bindgen, js-sys, send_wrapper, gloo-timers, indexed_db_futures, serde, serde_json, chrono, base64, trakkt-types
   - SSR deps (optional): leptos_axum, trakkt-core, trakkt-auth, axum, sqlx, tokio, tracing, uuid

2. `crates/trakkt-ui/Trunk.toml` — Copy Kyomi's config:
   ```toml
   [build]
   dist = "dist"
   cargo_profile = "wasm-dev"

   [[hooks]]
   stage = "pre_build"
   command = "tailwindcss"
   command_arguments = ["--input", "style/main.css", "--output", "style/output.css"]
   ```
   Skip the brotli/gzip post-build hook for dev (add later for production).

3. `crates/trakkt-ui/index.html` — Adapted from Kyomi:
   - Theme detection script (localStorage key: `trakkt-theme`)
   - Google Fonts: DM Sans, Instrument Serif, Geist Mono
   - Link to output.css
   - Copy public dir
   - Leptos hydrate feature
   - Loading spinner (teal accent, not amber)
   - Auth refresh script
   - Body: `min-h-screen bg-background text-foreground antialiased`
   - Remove Stripe.js (not needed)

4. `crates/trakkt-ui/style/main.css` — Full design system from DESIGN.md:
   ```css
   @import "tailwindcss" source(none);
   @source "../src";
   @custom-variant dark (&:where(.dark, .dark *));
   html { font-size: 15px; }
   ```
   All @theme tokens from DESIGN.md adapted for Trakkt:
   - Fonts: DM Sans (sans), Geist Mono (mono), Instrument Serif (display)
   - Colors: Teal accent (#0D9488), warm grays, semantic colors, priority colors, status colors
   - `--color-primary: #0D9488` (teal, NOT amber)
   - `--color-ring: #0D9488` (teal)
   - Sidebar: same warm-dark-deep (#1C1917)
   - Dark mode tokens
   - Border radius, shadows, animation tokens, durations, easings
   - Keyframes: fade-in, zoom-in-95, slide-in-from-top/bottom/left/right
   - Utility classes: .animate-fade-in, .animate-zoom-fade-in
   - Font-display class for Instrument Serif

5. `crates/trakkt-ui/src/lib.rs` — Module declarations:
   ```rust
   pub mod app;
   pub mod components;
   pub mod pages;
   pub mod server_fns;
   // pub mod cache; — Phase 5
   ```
   Re-export App.
   `register_server_functions()` stub (SSR feature).

6. `crates/trakkt-ui/src/app.rs` — Minimal router:
   ```rust
   pub fn Shell(children: Option<Children>) -> impl IntoView { ... }
   pub fn App() -> impl IntoView {
       // ThemeProvider + Router
       // Routes: /login, / (redirect to /issues), /issues, /issues/:number, /board, /settings
       // ParentRoute with Layout for authenticated pages
   }
   ```

7. `crates/trakkt-ui/src/components/mod.rs` — Empty module declarations
8. `crates/trakkt-ui/src/pages/mod.rs` — Empty module declarations
9. `crates/trakkt-ui/src/server_fns/mod.rs` — Empty module declarations, `extract_context()` and `extract_auth()` helpers (SSR)

**Reference files:**
- `/home/jason/repos/kyomi/crates/kyomi-ui/Trunk.toml`
- `/home/jason/repos/kyomi/crates/kyomi-ui/index.html`
- `/home/jason/repos/kyomi/crates/kyomi-ui/style/main.css`
- `/home/jason/repos/kyomi/crates/kyomi-ui/src/app.rs`
- `/home/jason/repos/kyomi/crates/kyomi-ui/src/lib.rs`

---

### GROUP 2: Database + Server (depends on Group 1)

#### Task 4: Database Migrations

Create both Postgres and SQLite migration files for all 12 tables from ARCHITECTURE.md.

**Files to create:**

1. `apps/server/migrations/001_initial.sql` — Postgres DDL for all tables:
   - workspaces, users, workspace_members, issues, labels, issue_labels, comments, sync_log, notifications, issue_watchers, passkeys, api_tokens, sessions
   - All indexes from ARCHITECTURE.md
   - Use `TEXT` for IDs (UUIDs stored as text for SQLite compat)
   - Use `SERIAL` or `BIGSERIAL` for sync_log.sync_id in Postgres
   - Timestamps as `TIMESTAMPTZ` in Postgres

2. `apps/server/migrations-sqlite/001_initial.sql` — SQLite DDL:
   - Same tables, SQLite-compatible types
   - `INTEGER PRIMARY KEY AUTOINCREMENT` for sync_log.sync_id
   - Timestamps as `TEXT` (ISO 8601)
   - No `TIMESTAMPTZ`, no `SERIAL`

**Important:** Both migrations must create identical logical schemas. The `DbPool` macros handle the type differences at runtime.

**Reference:** `docs/ARCHITECTURE.md` data model section.

---

#### Task 5: apps/server Scaffold

Create the Axum server binary that boots, runs migrations, and serves the Leptos frontend.

**Files to create:**

1. `apps/server/Cargo.toml`:
   - Binary name: `tane`
   - Dependencies: all tane crates, axum 0.8, tokio, tower, tower-http (cors, normalize-path, trace, limit), sqlx, serde, serde_json, tracing, tracing-subscriber, uuid, chrono, rust-embed, any_spawner, webauthn-rs (optional for later)

2. `apps/server/src/main.rs` — Startup sequence (copy Kyomi pattern):
   - Health subcommand (`tane health`)
   - Init tracing
   - Load Config from env
   - Connect to database (auto-detect + migrations)
   - Create KV store
   - Build AppState
   - Register Leptos server functions
   - Build Axum router
   - Bind and serve on config.port

3. `apps/server/src/lib.rs` — Router builder:
   - `build_router(state: AppState) -> Router`
   - Health check routes
   - Leptos server function handler at `/leptos-api/{*fn_name}`
   - Leptos asset serving at `/leptos/{*path}`
   - Fallback to serve Leptos shell
   - Middleware: security headers, CORS, trace, request body limit (10MB)

4. `apps/server/src/state.rs` — AppState:
   ```rust
   #[derive(Clone)]
   pub struct AppState {
       pub db: DbPool,
       pub kv: KVPool,
       pub config: Arc<Config>,
       pub encryption_key: Arc<[u8; 32]>,
       // pub webauthn: Arc<Webauthn>, — Phase 6
       // pub ws_manager: WebSocketManager, — Phase 5
   }
   ```

5. `apps/server/src/leptos_frontend.rs` — Copy Kyomi's pattern:
   - `#[derive(Embed)]` for `../../crates/trakkt-ui/dist/`
   - `serve_protected_page()` — check auth cookie, redirect to /login
   - `serve_leptos_shell()` — serve index.html with no-cache
   - `serve_leptos_asset()` — serve assets with immutable cache
   - `serve()` fallback — SPA routing

6. `apps/server/src/middleware/mod.rs` — Security headers, CORS

**Reference files:**
- `/home/jason/repos/kyomi/apps/server/src/main.rs`
- `/home/jason/repos/kyomi/apps/server/src/lib.rs`
- `/home/jason/repos/kyomi/apps/server/src/state.rs`
- `/home/jason/repos/kyomi/apps/server/src/leptos_frontend.rs`

---

### GROUP 3: Auth + Services (depends on Group 2)

#### Task 6: Authentication

Password auth, JWT sessions, cookies. No passkeys yet (Phase 6).

**Files to create in `crates/trakkt-auth/`:**

1. `Cargo.toml` — dependencies: trakkt-core, trakkt-types, axum, sqlx, async-trait, jsonwebtoken, argon2, serde, serde_json, tokio, tracing, uuid, chrono, rand, base64, redis

2. `src/lib.rs` — public modules: auth_service, password, jwt, session, cookies, user_service, workspace_service, issue_service, label_service, comment_service, notification_service, sync_log_service, rate_limiter, middleware

3. `src/password.rs` — Copy Kyomi:
   - `hash_password(password) -> Result<String>` (argon2id)
   - `verify_password(password, hash) -> Result<bool>` (detect argon2 vs bcrypt)

4. `src/jwt.rs` — Copy Kyomi:
   - Claims struct (sub, exp, iat, jti, extra HashMap)
   - `create_access_token(user_id, secret, expires_minutes)` — HS256 JWT
   - `create_refresh_token()` — opaque `rt_<random>` string
   - `validate_token(token, secret) -> Result<Claims>`

5. `src/session.rs` — Copy Kyomi:
   - `AuthenticatedSession { access_token, refresh_token, cookie_headers, user, workspace_id }`
   - `create_authenticated_session(db, kv, jwt_secret, user)` — creates JWT + refresh token, stores session in DB, sets HTTPOnly cookies

6. `src/cookies.rs` — Cookie names, set/clear helpers

7. `src/auth_service.rs`:
   - `LoginResult` enum: Success, RateLimited, InvalidCredentials, Error
   - `login_with_password_service(db, kv, email, password)` — rate limit check, user lookup, password verify, create session
   - `signup_service(db, email, password, display_name)` — create user, create default workspace, create membership, create session
   - `logout_service(db, kv, session_id)` — revoke session

8. `src/rate_limiter.rs` — KV-based rate limiting (copy Kyomi pattern)

9. `src/middleware.rs` — `extract_auth()` helper for server functions

**Reference files:**
- `/home/jason/repos/kyomi/crates/kyomi-auth/src/password.rs`
- `/home/jason/repos/kyomi/crates/kyomi-auth/src/jwt.rs`
- `/home/jason/repos/kyomi/crates/kyomi-auth/src/session.rs`
- `/home/jason/repos/kyomi/crates/kyomi-auth/src/auth_service.rs`
- `/home/jason/repos/kyomi/crates/kyomi-auth/src/cookies.rs`
- `/home/jason/repos/kyomi/crates/kyomi-auth/src/rate_limiter.rs`

---

#### Task 7: Core Services

Issue, workspace, label, comment, notification, and sync log services.

**Files to create in `crates/trakkt-auth/src/`:**

1. `src/user_service.rs`:
   - `get_user_by_id(db, user_id) -> Result<User>`
   - `get_user_by_email(db, email) -> Result<Option<User>>`
   - `create_user(db, email, display_name, password_hash) -> Result<User>`
   - `update_user(db, user_id, display_name, avatar_url) -> Result<()>`

2. `src/workspace_service.rs`:
   - `create_workspace(db, name, slug, creator_id) -> Result<Workspace>`
   - `get_workspace(db, workspace_id) -> Result<Workspace>`
   - `get_workspace_by_slug(db, slug) -> Result<Option<Workspace>>`
   - `get_user_workspaces(db, user_id) -> Result<Vec<Workspace>>`
   - `get_workspace_members(db, workspace_id) -> Result<Vec<WorkspaceMember>>`
   - `add_member(db, workspace_id, user_id, role) -> Result<()>`
   - `remove_member(db, workspace_id, user_id) -> Result<()>`
   - `update_member_role(db, workspace_id, user_id, role) -> Result<()>`

3. `src/issue_service.rs`:
   - `create_issue(db, workspace_id, creator_id, title, description, priority, assignee_id, due_date, label_ids) -> Result<Issue>`
     - Auto-increment issue number per workspace: `SELECT COALESCE(MAX(number), 0) + 1 FROM issues WHERE workspace_id = $1`
   - `get_issue(db, workspace_id, number) -> Result<IssueWithDetails>`
   - `list_issues(db, workspace_id, filters: IssueFilters) -> Result<Vec<IssueWithDetails>>`
     - IssueFilters: status, priority, assignee_id, label_id, search, limit, offset
   - `update_issue(db, workspace_id, number, updates: IssueUpdate) -> Result<Issue>`
     - IssueUpdate: title, description, status, priority, assignee_id, due_date
   - `delete_issue(db, workspace_id, number) -> Result<()>`
   - `set_issue_labels(db, issue_id, label_ids) -> Result<()>` — replace all labels

4. `src/label_service.rs`:
   - `create_label(db, workspace_id, name, color) -> Result<Label>`
   - `list_labels(db, workspace_id) -> Result<Vec<Label>>`
   - `update_label(db, label_id, name, color) -> Result<Label>`
   - `delete_label(db, label_id) -> Result<()>`
   - `get_issue_labels(db, issue_id) -> Result<Vec<Label>>`

5. `src/comment_service.rs`:
   - `create_comment(db, issue_id, user_id, body, parent_id) -> Result<Comment>`
   - `list_comments(db, issue_id) -> Result<Vec<Comment>>`
   - `update_comment(db, comment_id, user_id, body) -> Result<Comment>`
   - `delete_comment(db, comment_id, user_id) -> Result<()>`

6. `src/notification_service.rs`:
   - `create_notification(db, workspace_id, user_id, issue_id, notification_type) -> Result<()>`
   - `list_notifications(db, user_id, unread_only) -> Result<Vec<Notification>>`
   - `mark_as_read(db, notification_id, user_id) -> Result<()>`
   - `mark_all_as_read(db, user_id) -> Result<()>`
   - `count_unread(db, user_id) -> Result<i64>`

7. `src/sync_log_service.rs` — Copy Kyomi pattern:
   - `append_sync_log(db, workspace_id, entity_type, entity_id, action, payload) -> Result<i64>`
   - `get_bootstrap(db, workspace_id) -> Result<Vec<SyncAction>>`
   - `get_delta(db, workspace_id, last_sync_id) -> Result<Vec<SyncAction>>`

**All services follow the free-function pattern:** first argument is `&DbPool`, use `db_fetch_all!` / `db_execute!` macros. Every write operation also appends to `sync_log`.

---

### GROUP 4: Server Functions + UI Components (depends on Group 3)

#### Task 8: Leptos Server Functions

Thin wrappers over the service layer. These are the RPC bridge between Leptos UI and the backend.

**Files to create in `crates/trakkt-ui/src/server_fns/`:**

1. `mod.rs`:
   - `extract_context()` — extract AppState from Leptos context (SSR)
   - `extract_auth()` — extract JWT claims from cookie (SSR)
   - `IntoServerFnError` trait for converting trakkt_core::Error to ServerFnError

2. `auth.rs`:
   - `get_auth_config() -> AuthConfig` (public, no auth)
   - `login_with_password(email, password) -> LoginResult`
   - `signup(email, password, display_name) -> SignupResult`
   - `logout() -> Result<()>`
   - `get_current_user() -> Result<CurrentUser>` (protected)

3. `issues.rs`:
   - `list_issues(status, priority, assignee, label, search, limit, offset) -> Vec<IssueWithDetails>`
   - `get_issue(number) -> IssueWithDetails`
   - `create_issue(title, description, priority, assignee_id, due_date, label_ids) -> Issue`
   - `update_issue(number, title, description, status, priority, assignee_id, due_date) -> Issue`
   - `delete_issue(number) -> ()`
   - `set_issue_labels(number, label_ids) -> ()`

4. `comments.rs`:
   - `list_comments(issue_number) -> Vec<Comment>`
   - `create_comment(issue_number, body, parent_id) -> Comment`
   - `update_comment(comment_id, body) -> Comment`
   - `delete_comment(comment_id) -> ()`

5. `labels.rs`:
   - `list_labels() -> Vec<Label>`
   - `create_label(name, color) -> Label`
   - `update_label(label_id, name, color) -> Label`
   - `delete_label(label_id) -> ()`

6. `workspace.rs`:
   - `get_workspace() -> Workspace`
   - `get_workspace_members() -> Vec<WorkspaceMember>`
   - `update_workspace(name) -> Workspace`

7. `notifications.rs`:
   - `list_notifications(unread_only) -> Vec<Notification>`
   - `mark_notification_read(notification_id) -> ()`
   - `mark_all_read() -> ()`
   - `count_unread() -> i64`

All server functions use `#[server(prefix = "/leptos-api")]`, extract auth via `extract_auth()`, get workspace_id from auth claims, delegate to trakkt-auth services.

**Update `crates/trakkt-ui/src/lib.rs`:** Add `register_server_functions()` that calls `leptos::server_fn::axum::register_explicit::<T>()` for every server function.

**Reference:** `/home/jason/repos/kyomi/crates/kyomi-ui/src/server_fns/auth.rs`

---

#### Task 9: UI Components

Tane-specific UI components, following the design system. These live in `crates/trakkt-ui/src/components/`.

**Files to create:**

1. `mod.rs` — re-export all components

2. `button.rs` — Button component:
   - Variants: Primary, Secondary, Ghost, GhostMuted, Destructive, Outline
   - Sizes: Default (14px, px-5 py-2.5), Small (13px, px-3.5 py-2)
   - Props: variant, size, disabled, class (override), on_click, children
   - All use teal accent, DM Sans 600 weight, rounded-md, transition-colors
   - Accessibility: focus-visible ring, disabled states

3. `card.rs` — Card, CardHeader, CardTitle, CardContent components

4. `modal.rs` — Modal component:
   - Sizes: sm, md, lg
   - Backdrop: bg-black/50
   - Escape to close
   - Focus trap

5. `confirm_dialog.rs` — ConfirmDialog (wraps Modal):
   - Props: title, message, confirm_text, cancel_text, variant (default/destructive)

6. `toast.rs` — Toast notification system:
   - Variants: success, error, warning, info
   - Auto-dismiss after 5 seconds
   - Provide toast context, `use_toast()` hook

7. `status_badge.rs` — StatusBadge component:
   - Colored dot + text for each IssueStatus
   - Colors from DESIGN.md status colors section

8. `priority_icon.rs` — PriorityIcon component:
   - Colored indicator for each Priority level
   - Colors from DESIGN.md priority colors section

9. `label_badge.rs` — LabelBadge component:
   - Colored pill with label name
   - Dynamic background from label.color with appropriate text contrast

10. `search_input.rs` — SearchInput component:
    - Search icon, input field, clear button
    - Debounced value signal

11. `skeleton.rs` — Skeleton loading placeholder

12. `spinner.rs` — Loading spinner (teal accent)

13. `empty_state.rs` — Empty state with Phosphor icon, heading, description, action button

14. `sidebar.rs` — App sidebar:
    - Dark background (#1C1917)
    - Logo/brand at top
    - Nav items: Issues, Board, Settings
    - Notification bell
    - User avatar + dropdown at bottom
    - Collapsible (20rem expanded, 4rem collapsed)
    - Active state: Light → Fill icon weight

15. `layout.rs` — Layout wrapper:
    - Sidebar + content area
    - Content: `bg-background`, full height
    - Page header pattern from DESIGN.md

16. `dropdown.rs` — Dropdown/select for status, priority, assignee pickers

17. `avatar.rs` — User avatar (initials fallback, rounded-full)

18. `markdown_renderer.rs` — Render markdown to HTML (use pulldown-cmark or similar)

**Reference files:**
- `/home/jason/repos/kyomi/crates/kyomi-ui-components/src/components/button.rs`
- `/home/jason/repos/kyomi/crates/kyomi-ui-components/src/components/modal.rs`
- `/home/jason/repos/kyomi/crates/kyomi-ui-components/src/components/toast.rs`
- `DESIGN.md` for all styling specs

---

### GROUP 5: Pages (depends on Group 4)

#### Task 10: Auth Pages + App Shell

**Files to create/update:**

1. `crates/trakkt-ui/src/pages/mod.rs` — module declarations

2. `crates/trakkt-ui/src/pages/login.rs` — Login/Signup page:
   - Email + password form
   - Toggle between login and signup mode
   - Error display
   - Calls login_with_password / signup server functions
   - On success, redirect to /issues
   - Styled per design system (centered card, teal accent)

3. Update `crates/trakkt-ui/src/app.rs` — Wire up all routes:
   - `/login` → LoginPage (no layout)
   - `/signup` → LoginPage with signup_mode
   - ParentRoute `""` with Layout:
     - `/` redirect to `/issues`
     - `/issues` → IssueListPage
     - `/issues/:number` → IssueDetailPage
     - `/board` → BoardPage
     - `/settings` → SettingsPage

4. Update sidebar and layout components with working navigation links

---

#### Task 11: Issue List + Issue Detail Pages

**Files to create:**

1. `crates/trakkt-ui/src/pages/issue_list.rs` — Issue List page:
   - Page header: "Issues" title + "New Issue" button
   - Toolbar: SearchInput + filter dropdowns (status, priority, label, assignee)
   - Issue rows: status dot, issue number (Geist Mono), title, labels, priority icon, assignee avatar
   - Row hover: bg-surface-alt
   - Click row → navigate to /issues/:number
   - Empty state when no issues
   - Loading skeleton while fetching
   - New issue modal (opened by button or 'c' key)

2. `crates/trakkt-ui/src/pages/issue_detail.rs` — Issue Detail page:
   - Back button (ghost icon button)
   - Issue number (Geist Mono, text-xs)
   - Title (text-2xl font-display, inline-editable)
   - Metadata bar: status dropdown, priority dropdown, assignee dropdown, label pills (add/remove), due date picker
   - Description (markdown rendered, editable)
   - Comments section:
     - Comment list with threading (one level)
     - Each comment: avatar, name, timestamp, markdown body
     - Threaded replies indented
     - New comment textarea with submit button
   - Max-width content area (860px)

---

#### Task 12: Board View + Settings Page

**Files to create:**

1. `crates/trakkt-ui/src/pages/board.rs` — Kanban Board page:
   - Page header: "Board" title
   - Columns: one per status (Backlog, Todo, In Progress, Done, Cancelled)
   - Column header: status name + issue count, sticky
   - Issue cards: bg-card, border, rounded-md, shadow-sm
     - Issue number (Geist Mono, text-xs, text-muted)
     - Title (DM Sans, text-sm, font-medium)
     - Label pills
     - Priority icon + assignee avatar
   - Card hover: shadow-md
   - Drag and drop between columns to change status (use HTML5 drag events or a minimal drag library)
   - Horizontal scroll for columns
   - Each column: min-w-[280px] max-w-[320px]

2. `crates/trakkt-ui/src/pages/settings.rs` — Settings page:
   - Workspace settings: name, slug (read-only after creation)
   - Labels management: list, create, edit, delete labels with color picker
   - Members: list members, roles, invite (future)
   - Profile: display name, avatar
   - (API tokens — deferred to Phase 6)

---

## Compilation & Testing

After all tasks complete:

1. `cargo build` — must compile with no errors
2. `trunk build` in `crates/trakkt-ui/` — must produce dist/
3. `DATABASE_URL=postgres://... cargo run` — must boot, run migrations, serve UI
4. Browser test: register, login, create workspace, create issue, view list, view detail, use board, manage labels

## What's Deferred

- Sync engine (WebSocket + IndexedDB) — Phase 5
- Public REST API + MCP server — Phase 6
- Search (FTS5/tsvector) — Phase 6
- Keyboard navigation + command palette — Phase 6
- Notifications — Phase 6
- Passkey auth — Phase 6
- Dark mode — Phase 6
- API token management — Phase 6
