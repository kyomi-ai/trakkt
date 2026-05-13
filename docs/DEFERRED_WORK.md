# Deferred Work

Items identified during the V1 implementation (Slices 1-12, May 2026). Each item includes enough context for a new agent to pick it up without prior conversation access.

## Status of Previously Deferred Items

Items from the original deferred list that are now **implemented**:
- ~~Keyboard navigation~~ — Implemented in Slice 11 (j/k, Enter, c, Cmd+K command palette)
- ~~MCP server~~ — Implemented in Slice 10 (8 domain tools with scope enforcement)
- ~~Sync engine (server-side)~~ — Implemented in Slice 9 (WebSocket broadcast on all writes)
- ~~ConfirmDialog for destructive actions~~ — Label delete uses ConfirmDialog (Slice 8)
- ~~Hardcoded hex colors~~ — Status dot colors centralized in IssueStatusVariant::dot_color(), surface-alt token added to main.css

---

## Client-Side IndexedDB Cache

**What:** Sync engine plan Tasks 4-6 (`docs/plans/2026-05-01-phase5-sync-engine.md`). IndexedDB persistence for offline-first data. Server-side broadcasting works (Slice 9) but clients don't cache locally.

**Why deferred:** Significant WASM work — `indexed_db_futures`, `SendWrapper`, reactive `SyncStore`.

**Impact:** No offline support. Every page load fetches from server. Multi-tab gets WebSocket notifications but re-fetches rather than applying deltas.

**Files:** Reference `~/repos/kyomi/crates/kyomi-ui/src/cache/`. New files: `crates/trakkt-ui/src/cache/{mod,db,store,sync_engine,websocket}.rs`.

---

## Avatar Image URLs

**What:** No `avatar_url` column on `users` table. All queries use `NULL AS author_avatar`. Avatar component shows initials only.

**Why deferred:** Requires migration + logic to extract URLs from encrypted `oauth_data`.

**Files:** Migration in `apps/server/migrations/`, update queries in `comment_service.rs` (4 sites), `Avatar` component in `crates/trakkt-ui/src/components/avatar.rs`.

---

## Team Delete

**What:** Teams settings page creates teams but has no delete button.

**Why deferred:** Needs decision on issue reassignment when deleting a team with existing issues.

**Files:** `crates/trakkt-auth/src/team_service.rs` (add `delete_team`), `crates/trakkt-ui/src/pages/settings/teams_settings.rs` (add delete UI).

---

## Issue Detail Keyboard Shortcuts

**What:** DESIGN.md specifies `1-5` for priority, `s` for status, `l` for labels, `a` for assignee on the detail page.

**Why deferred:** Detail page is functional via click. These are power-user polish.

**Files:** `crates/trakkt-ui/src/pages/issues/issue_detail.rs`. Use the `is_input_focused` guard from `issue_list.rs`.

---

## Full-Text Search

**What:** Issue search uses `LIKE '%term%'` — functional but slow at scale, no relevance ranking.

**Why deferred:** Needs different implementations per database (FTS5 for SQLite, tsvector+GIN for Postgres).

**Files:** `crates/trakkt-auth/src/issue_service.rs` (update `list_issues` query), new migration for FTS index.

---

## Date Picker Component

**What:** Due dates display in the detail page but can't be set via UI. Service layer supports them.

**Why deferred:** No date picker component exists. Browser-native `<input type="date">` doesn't match the design system.

**Files:** New component `crates/trakkt-ui/src/components/date_picker.rs`, wire into `issue_detail.rs` metadata bar.

---

## Assignee Picker

**What:** Assignee displays but can't be changed via UI. Service layer and MCP both support it.

**Why deferred:** Needs a member-fetching dropdown with avatars.

**Files:** `crates/trakkt-ui/src/pages/issues/issue_detail.rs` (add dropdown to metadata bar), uses `list_workspace_members` from `server_fns/team.rs`.

---

## Transaction Safety for set_issue_labels

**What:** `set_issue_labels` does DELETE then INSERT without a transaction. Concurrent readers could briefly see zero labels.

**Why deferred:** Requires transaction support in `DbPool` abstraction.

**Files:** `crates/trakkt-core/src/db.rs` (add begin/commit/rollback), `crates/trakkt-auth/src/issue_service.rs`.

---

## Public REST API

**What:** ARCHITECTURE.md specifies `/api/v1/` with 11 REST endpoints, token auth, rate limiting. MCP tools exist but the REST surface does not.

**Why deferred:** MCP covers the AI integration use case. REST is for third-party integrations (CI bots, webhooks).

**Files:** New `apps/server/src/routes/api_v1.rs`, reuses the same service layer.

---

## Dark Mode Polish

**What:** Dark mode tokens are defined in `main.css` and the theme toggle works. Some new components may need dark-mode testing.

**Why deferred:** Core functionality was prioritized over visual polish.

**Resolution:** Boot the app, toggle dark mode, screenshot every page, fix any issues.
