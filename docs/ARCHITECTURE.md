# Tane — Design Document

**Date:** 2026-04-30
**Status:** Draft
**Author:** Jason Adams
**Domain:** tane.dev

## Vision

An open-source issue tracker that takes Linear's opinionated UX philosophy and ships it as a single binary. Local-first, works offline, self-hostable with zero dependencies. The anti-Jira.

Built on the same Rust/Axum/Leptos stack as Kyomi, reusing the battle-tested patterns (local-first sync engine, dual-mode database, single-binary deployment) with a radically simpler data model.

## Core Principles

1. **Opinionated, not configurable.** Fixed workflow. Fixed priorities. No custom fields. If you need that, use Jira.
2. **Single binary.** `./tane` and you're running. No Docker Compose with 5 services.
3. **Local-first.** Instant UI. Data cached in IndexedDB, synced via WebSocket. Works offline.
4. **Agent-native.** Built-in MCP server so AI agents can manage issues without custom integrations.

## V1 Feature Set

### In Scope

| Feature | Details |
|---------|---------|
| **Issues** | Title, markdown description, status, priority, assignee, labels, due date |
| **Statuses** | Fixed workflow: `Backlog → Todo → In Progress → Done → Cancelled` |
| **Priorities** | `Urgent > High > Medium > Low > None`. Automatic sort order. |
| **Labels** | Flat tags with color. Workspace-scoped. |
| **Comments** | Markdown. Threaded one level deep. |
| **Users & Workspaces** | Multi-workspace. Password + passkey auth. Invite by email. |
| **Views** | List view (default), board view (kanban by status). Sort/filter by priority, label, assignee. |
| **Keyboard navigation** | `j/k` navigate, `Enter` opens, `x` selects, bulk actions on selection. Command palette (`Cmd+K`). |
| **Markdown editor** | Inline editing with preview. Good textarea with shortcuts, not WYSIWYG. |
| **Search** | Full-text search across titles + descriptions. |
| **Notifications** | In-app only. Assigned to you, mentioned, watching. |
| **API** | REST + WebSocket. Internal endpoints power the UI; public versioned API (`/api/v1/`) for third-party integrations. |
| **MCP Server** | Streamable HTTP transport at `/mcp`. Agents can create, update, query issues and comments. |
| **Self-hosted** | Single binary, SQLite, zero config. |
| **SaaS mode** | Postgres + Redis (same binary, env vars switch it). |

### Explicitly Out of Scope (v1)

| Feature | Rationale |
|---------|-----------|
| Cycles/sprints | Filter by due date instead. Formal sprints add complexity. |
| Projects/milestones | Labels handle grouping for small-medium teams. |
| Sub-issues/parent-child | Flat list with labels. Hierarchy creates nightmares. |
| Custom fields | The whole point is opinionated. |
| File attachments | Support image URLs in markdown. No upload infrastructure. |
| Built-in integrations (GitHub, Slack) | Keep the core clean first. Third parties integrate inbound via the public API — Tane doesn't reach out. |
| Automations/rules | Status transitions are manual. |
| Time tracking | Different product. Never. |
| Gantt charts | Never. |

## Data Model

```sql
-- Workspaces
CREATE TABLE workspaces (
    workspace_id    TEXT NOT NULL PRIMARY KEY,
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL UNIQUE,  -- used as issue prefix (e.g. TRK-1)
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

-- Users
CREATE TABLE users (
    user_id         TEXT NOT NULL PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    avatar_url      TEXT,
    password_hash   TEXT,
    created_at      TEXT NOT NULL
);

-- Workspace membership
CREATE TABLE workspace_members (
    workspace_id    TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id         TEXT NOT NULL REFERENCES users(user_id),
    role            TEXT NOT NULL,  -- 'owner' | 'admin' | 'member'
    joined_at       TEXT NOT NULL,
    PRIMARY KEY (workspace_id, user_id)
);

-- Issues
CREATE TABLE issues (
    issue_id        TEXT NOT NULL PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(workspace_id),
    number          INTEGER NOT NULL,  -- auto-increment per workspace
    title           TEXT NOT NULL,
    description     TEXT,              -- markdown
    status          TEXT NOT NULL DEFAULT 'backlog',
    priority        INTEGER NOT NULL DEFAULT 0,  -- 0=none, 1=urgent, 2=high, 3=medium, 4=low
    assignee_id     TEXT REFERENCES users(user_id),
    creator_id      TEXT NOT NULL REFERENCES users(user_id),
    due_date        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(workspace_id, number)
);

-- Labels
CREATE TABLE labels (
    label_id        TEXT NOT NULL PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(workspace_id),
    name            TEXT NOT NULL,
    color           TEXT NOT NULL,  -- hex color
    created_at      TEXT NOT NULL,
    UNIQUE(workspace_id, name)
);

-- Issue-label join
CREATE TABLE issue_labels (
    issue_id        TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    label_id        TEXT NOT NULL REFERENCES labels(label_id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, label_id)
);

-- Comments
CREATE TABLE comments (
    comment_id      TEXT NOT NULL PRIMARY KEY,
    issue_id        TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL REFERENCES users(user_id),
    body            TEXT NOT NULL,     -- markdown
    parent_id       TEXT REFERENCES comments(comment_id),  -- one level threading
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

-- Sync log (powers local-first protocol)
CREATE TABLE sync_log (
    sync_id         INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(workspace_id),
    entity_type     TEXT NOT NULL,  -- 'issue' | 'comment' | 'label'
    entity_id       TEXT NOT NULL,
    action          TEXT NOT NULL,  -- 'upsert' | 'delete'
    payload         TEXT NOT NULL,  -- JSON
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_sync_log_workspace_cursor
    ON sync_log(workspace_id, sync_id);

-- Notifications
CREATE TABLE notifications (
    notification_id TEXT NOT NULL PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id         TEXT NOT NULL REFERENCES users(user_id),
    issue_id        TEXT NOT NULL REFERENCES issues(issue_id),
    type            TEXT NOT NULL,  -- 'assigned' | 'mentioned' | 'comment'
    read            INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_notifications_user_unread
    ON notifications(user_id, read, created_at);

-- Issue watchers
CREATE TABLE issue_watchers (
    issue_id        TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL REFERENCES users(user_id),
    PRIMARY KEY (issue_id, user_id)
);

-- Passkey credentials (WebAuthn)
CREATE TABLE passkeys (
    passkey_id      TEXT NOT NULL PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(user_id),
    credential_id   TEXT NOT NULL UNIQUE,
    public_key      BLOB NOT NULL,
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

-- API tokens (third-party integrations + MCP)
CREATE TABLE api_tokens (
    token_id        TEXT NOT NULL PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id         TEXT NOT NULL REFERENCES users(user_id),  -- token creator, used for audit trail
    name            TEXT NOT NULL,       -- human label, e.g. "CI bot"
    token_hash      TEXT NOT NULL UNIQUE, -- argon2 hash of the displayed-once token
    token_prefix    TEXT NOT NULL,        -- first 8 chars, shown in UI for identification
    scopes          TEXT NOT NULL,        -- JSON array: ["issues:read","issues:write","comments:write",...]
    last_used_at    TEXT,
    expires_at      TEXT,                 -- NULL = no expiry
    created_at      TEXT NOT NULL
);

-- Sessions (backup to KV store for self-hosted SQLite mode)
CREATE TABLE sessions (
    session_id      TEXT NOT NULL PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(user_id),
    refresh_token   TEXT NOT NULL,
    expires_at      TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
```

~12 tables total. The `sync_log` table powers the local-first protocol — same pattern as Kyomi. The `api_tokens` table is shared between the public API and MCP authentication.

## Architecture

### Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Axum 0.8, sqlx 0.8 |
| Frontend | Leptos 0.8, SSR + WASM hydration |
| Styling | Tailwind CSS 4, Singlestage UI components |
| Icons | Phosphor Icons |
| Sync | WebSocket bootstrap/delta protocol, IndexedDB |
| Auth | Password + passkey (WebAuthn), JWT sessions |
| KV | Redis (SaaS) / in-memory (self-hosted) |
| Database | Postgres (SaaS) / SQLite (self-hosted) |
| MCP | Streamable HTTP (SSE) at `/mcp` |
| Deployment | Single `FROM scratch` Docker image |

### Crate Structure

```
tane/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── trakkt-core/            # db.rs, kv_store.rs, config.rs, error.rs
│   ├── trakkt-auth/            # auth, sessions, workspace/user/issue services
│   ├── trakkt-types/           # shared DTOs (serde-only, WASM-safe)
│   └── trakkt-ui/              # Leptos frontend + server functions
│       ├── src/
│       │   ├── pages/
│       │   │   ├── issue_list.rs
│       │   │   ├── issue_detail.rs
│       │   │   ├── board.rs
│       │   │   ├── settings.rs
│       │   │   └── login.rs
│       │   ├── components/
│       │   │   ├── issue_card.rs
│       │   │   ├── issue_row.rs
│       │   │   ├── comment_thread.rs
│       │   │   ├── label_badge.rs
│       │   │   ├── priority_icon.rs
│       │   │   ├── status_badge.rs
│       │   │   ├── cmd_palette.rs
│       │   │   ├── markdown_editor.rs
│       │   │   └── notification_bell.rs
│       │   ├── cache/
│       │   │   ├── db.rs         # IndexedDB operations
│       │   │   ├── store.rs      # SyncStore (reactive signals)
│       │   │   └── sync_engine.rs
│       │   └── server_fns/
│       │       ├── issues.rs
│       │       ├── comments.rs
│       │       ├── labels.rs
│       │       └── notifications.rs
│       └── Trunk.toml
├── apps/
│   └── server/
│       ├── src/
│       │   ├── main.rs
│       │   ├── routes/
│       │   │   ├── api_v1.rs     # Public REST API (/api/v1/)
│       │   │   ├── mcp.rs        # MCP streamable HTTP endpoint
│       │   │   └── ws.rs         # WebSocket sync
│       │   ├── middleware/
│       │   │   ├── api_auth.rs   # Token auth + scope checking
│       │   │   └── rate_limit.rs # Per-token rate limiting
│       │   └── mcp/
│       │       ├── mod.rs
│       │       └── tools.rs      # MCP tool definitions
│       ├── migrations/           # Postgres
│       └── migrations-sqlite/    # SQLite
└── deploy/
    ├── Dockerfile
    └── docker-compose.yml
```

4 crates. No agent, no knowledge, no datasource, no embed.

### Sync Protocol

Same as Kyomi's sync engine:

1. **First visit** (empty IndexedDB): Client sends `sync_bootstrap` → server streams all issues, labels, comments → client writes to IDB + reactive signals.
2. **Return visit** (warm IndexedDB): Load from IDB instantly (sub-100ms) → send `sync_delta` with last `sync_id` cursor → server returns only changes since that cursor.
3. **Schema mismatch**: Client detects `SCHEMA_HASH` change → wipes IDB → re-bootstraps.
4. **Writes**: Mutations go through server functions → server writes to DB + appends to `sync_log` → broadcasts delta to connected WebSocket clients → all clients update IDB + signals.

   `sync_id` order must equal commit order, because a client's cursor is the last id it received and anything below it is never sent again. On Postgres a BIGSERIAL is issued at insert, not commit, so every `sync_log` write first takes a per-workspace transaction-scoped advisory lock (`serialise_sync_log_writers`). It is held until commit, which serialises sync-writing transactions within a workspace; keep them short.

Entity types in sync store: `issue`, `comment`, `label`, `notification`.

### MCP Server

Streamable HTTP transport at `/mcp`, served by the same Axum binary.

**Authentication:** Bearer token in the `Authorization` header. Users generate API tokens from settings (stored hashed in the `api_tokens` table, scoped to a workspace).

**Tools:**

| Tool | Parameters | Returns |
|------|-----------|---------|
| `list_issues` | `status?`, `priority?`, `assignee?`, `label?`, `search?`, `limit?` | Lean issue rows (no description) |
| `get_issue` | `issue_number` | Issue detail + comments |
| `create_issue` | `title`, `description?`, `priority?`, `labels?[]`, `assignee?`, `due_date?` | Created issue |
| `update_issue` | `issue_number`, `title?`, `description?`, `status?`, `priority?`, `assignee?`, `labels?[]`, `due_date?` | Updated issue |
| `add_comment` | `issue_number`, `body` | Created comment |
| `list_labels` | — | All labels |
| `create_label` | `name`, `color` | Created label |
| `search_issues` | `query`, `limit?`, `offset?`, `team_key?`/`team_id?`, `status_id?`, `status_category?`, `filters?` (the same composable clauses as `list_issues`, applied in SQL before ranking and paging), `include_comments?`, `include_archived?`, `include_closed?` (deprecated) | Ranked matches `{results, total, has_more, next_offset, truncated}`; no filter is applied unless passed |

All tools write through the same service layer as the UI, which means mutations hit `sync_log` and broadcast to connected WebSocket clients. An agent creating an issue shows up instantly in every open browser tab.

**Resources:**

| Resource | URI Pattern |
|----------|-------------|
| Workspace info | `tane://workspace` |
| Issue detail | `tane://issues/{number}` |
| Label list | `tane://labels` |

### Public API

Versioned REST API at `/api/v1/` for third-party integrations. This is a key differentiator from Kyomi, which has no external integration surface.

**Authentication:** Bearer token in the `Authorization` header. Tokens are created in workspace settings, scoped to specific permissions, and stored hashed in `api_tokens`. The same token system is shared with MCP.

**Design principles:**
- Same service layer as the UI and MCP — a ticket created via API hits `sync_log` and broadcasts to all connected clients instantly.
- Versioned (`/api/v1/`) so we can evolve without breaking integrations.
- Scoped tokens — a CI bot that only needs to create issues doesn't get permission to delete them.
- Rate limited per token (details TBD, but enforced at the Axum middleware layer).

**Endpoints:**

| Method | Path | Scopes Required |
|--------|------|----------------|
| `GET` | `/api/v1/issues` | `issues:read` |
| `POST` | `/api/v1/issues` | `issues:write` |
| `GET` | `/api/v1/issues/:number` | `issues:read` |
| `PATCH` | `/api/v1/issues/:number` | `issues:write` |
| `DELETE` | `/api/v1/issues/:number` | `issues:delete` |
| `GET` | `/api/v1/issues/:number/comments` | `comments:read` |
| `POST` | `/api/v1/issues/:number/comments` | `comments:write` |
| `GET` | `/api/v1/labels` | `labels:read` |
| `POST` | `/api/v1/labels` | `labels:write` |
| `DELETE` | `/api/v1/labels/:id` | `labels:delete` |
| `GET` | `/api/v1/members` | `members:read` |

All list endpoints support `?status=`, `?priority=`, `?label=`, `?assignee=`, `?search=`, `?limit=`, `?offset=` query parameters where applicable.

**Response format:** JSON, consistent envelope:
```json
{
  "data": { ... },
  "meta": { "request_id": "...", "ratelimit_remaining": 95 }
}
```

**Error format:**
```json
{
  "error": { "code": "not_found", "message": "Issue TRK-999 not found" },
  "meta": { "request_id": "..." }
}
```

### GitHub Link Intent

The GitHub integration follows Linear's model: a key found in text is not a link.
A link is made by *intent*, and what a link may do to an issue's status is decided
by the **link kind**, never by "a key was found". Modelled on
[Linear's GitHub integration](https://linear.app/docs/github) and its
[2019](https://linear.app/changelog/2019-07-11-github-workflow-configuration),
[2020](https://linear.app/changelog/2020-05-05),
[2022](https://linear.app/changelog/2022-02-03-github-commit-linking) and
[2023](https://linear.app/changelog/2023-11-15-github-workflow-updates) changelogs.
Code: `crates/trakkt-github/src/patterns.rs` (reading intent),
`events.rs` (links), `transitions.rs` (status).

**Link sources**

| Source | Link kind |
| --- | --- |
| Branch name containing the key | `branch_title` |
| PR title containing the key | `branch_title` |
| Closing magic word + key (title, body) | `closing` |
| Non-closing magic word + key | `contributing` |
| Relation word + key | `related` |
| Full issue URL of this application after a magic word | the word's kind |
| Bare key in the PR **body** | `related` |
| Bare key in a commit message | `related` |
| Magic word in a commit message | the word's kind (a commit never changes status) |
| Anything in a PR *comment* | no link: only `pull_request` and `push` events are processed |
| `skip` / `ignore` + key | no link at all, even if the branch name has the key |

When several sources name one issue the strongest kind wins
(`closing` > `contributing` > `branch_title` > `related`); `skip` in any part of the
PR removes the issue entirely. The words are exactly Linear's lists
(`CLOSING_WORDS`, `CONTRIBUTING_WORDS`, `RELATED_WORDS`, `SKIP_WORDS`); several keys
may follow one word (`Fixes ENG-123, DES-5, and ENG-256`); a word must directly
precede the key (`Fixes the crash in TRA-5` is a mention). A URL only counts when
its host is this application's.

**What each kind does** (one table: `behavior()` in `transitions.rs`)

| Kind | Draft / opened / ready for review | Merged / closed without merge |
| --- | --- | --- |
| `closing` | fires | fires |
| `contributing` | fires | never |
| `branch_title` | fires | never (proposed deviation, see below) |
| `related` | never | never |

`related` links are shown on the issue and the PR and never change status or post a
comment. Commit mentions stay activity only.

**Automation steps.** Built: *draft* (`pr_draft`), *opened / ready for review*
(`pr_opened`, also fired by `reopened`), *merged* (`pr_merged`) and *closed without
merge* (`pr_closed`). A draft PR fires `pr_draft`, which has no default rule, so a
draft does nothing until a team configures it; `ready_for_review` fires `pr_opened`.
Automation fires on lifecycle events, not on edits. The workspace rules
(`github_transition_rules`, defaults: opened -> started, merged -> completed,
closed -> cancelled) are the default; a **team** may override any step
(`github_team_transition_rules`) with a status category or *no action*. The rule's
`close_intent_required` column is no longer consulted: which kinds may fire which
step is the `behavior()` table's job.

**Order of decisions** for each linked issue: the kind may fire the step; a rule
exists (team override, else enabled workspace rule); for merged/closed, no *other*
closing PR on the issue is still open or draft (the merge status applies when the
last one merges, and a PR closed without merging is never a merge); the move is
forward and the issue is not completed or cancelled.

**Deliberate deviations from Linear** (decided by the owner unless marked proposed)

- GitHub events never move a *Completed or Cancelled* issue (PR #366). Linear does
  not document this; we keep it on purpose.
- *Forward-only*: a GitHub event never moves an issue backwards or sideways. Status
  categories rank backlog < unstarted < started < completed/cancelled; the target
  must outrank the current category, so In Review is never pulled back to In Progress.
- A bare key in a PR body is a related-only link: shown, never acts.
- Cross-repository mentions have no special case: a PR in any connected repository
  follows the same rules, so a key named without a working link changes nothing.
- Proposed (pending owner confirmation): a `branch_title` link starts but does not
  complete the issue on merge, which matches the behaviour before this change and the
  safe reading of the incident; Linear completes on merge for branch-linked PRs.
  Flipping `completes` on that row of `behavior()` (and the matching row of
  `expected_category` in the tests) changes it.
- A PR closed without merging cancels the issue only through the existing workspace
  `pr_closed` rule, only for `closing` links, and only when no other closing PR is
  open or already merged. Linear does not do this.

**Linkback.** The bot comment on the PR is posted only when a link actually changed
the issue's status, and can be turned off per workspace
(`github_automation_settings.linkback_enabled`, Settings > Integrations).

**Not built** (tracked as follow-ups): the review-requested step (TRA-10116), the
ready-to-merge step (TRA-10117), and per-target-branch overrides with regex and no
action (TRA-10118). Assigning the linker to an unassigned issue is not built and not
planned (owner decision).

### Deployment Modes

| Mode | Trigger | Database | KV | Auth |
|------|---------|----------|-----|------|
| **Self-hosted** | No `DATABASE_URL` | SQLite in `./data/trakkt.db` | In-memory | Password + passkey |
| **SaaS** | `DATABASE_URL` set | Postgres | Redis | Password + passkey + invite |

Same pattern as Kyomi. Single binary, behavior switches on environment variables.

**Self-hosted quickstart:**
```bash
# Download binary
curl -L https://github.com/jasadams/tane/releases/latest/download/tane-linux-amd64.tar.gz | tar xz

# Run (creates ./data/trakkt.db on first boot)
./tane
# → listening on http://localhost:3000
```

**Docker:**
```bash
docker run -v tane-data:/data -p 3000:3000 ghcr.io/jasadams/tane:latest
```

## Design System

Fork Kyomi's design system with a different accent color to establish separate identity.

| Token | Kyomi | Tane |
|-------|-------|---------|
| Accent | `#D97706` (amber) | `#0D9488` (teal) |
| Display font | Instrument Serif | Instrument Serif |
| Body font | DM Sans | DM Sans |
| Mono font | Geist Mono | Geist Mono |
| Icons | Phosphor | Phosphor |
| Root font size | 15px | 15px |

Same spacing scale, same component library (Singlestage UI), same Phosphor icons. The issue tracker UI is simpler — mostly lists, detail views, and a kanban board. No charts, no dashboards, no chat.

## Key UI Screens

### Issue List (Default View)

```
┌─────────────────────────────────────────────────────────────┐
│ [Logo]  Issues  Board  Settings          🔔  [Avatar]       │
├─────────────────────────────────────────────────────────────┤
│ [Search...]  [Status ▾]  [Priority ▾]  [Label ▾]  [+ New]  │
├─────────────────────────────────────────────────────────────┤
│ ● TRK-42  Fix login redirect loop          ■ Urgent  @jason │
│ ○ TRK-41  Add dark mode toggle             ■ Medium  @—     │
│ ○ TRK-40  Update onboarding copy           ■ Low     @sarah │
│ ○ TRK-39  Refactor auth middleware         ■ None    @—     │
│   ...                                                        │
└─────────────────────────────────────────────────────────────┘
```

### Issue Detail

```
┌─────────────────────────────────────────────────────────────┐
│ ← Back to issues                                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│ TRK-42                                                       │
│ Fix login redirect loop                                      │
│                                                              │
│ Status: In Progress ▾   Priority: Urgent ▾                   │
│ Assignee: @jason ▾      Labels: [bug] [auth]                 │
│ Due: 2026-05-15                                              │
│                                                              │
│ ─────────────────────────────────────────────────────        │
│                                                              │
│ After OAuth callback, users get stuck in a redirect           │
│ loop between /login and /callback. Only happens when...      │
│                                                              │
│ ─────────────────────────────────────────────────────        │
│                                                              │
│ Comments (3)                                                  │
│                                                              │
│ @sarah · 2h ago                                              │
│ Reproduced on Chrome 126. Firefox works fine.                │
│   └─ @jason · 1h ago                                         │
│      Looks like a SameSite cookie issue. Investigating.      │
│                                                              │
│ [Add a comment...]                                           │
└─────────────────────────────────────────────────────────────┘
```

### Board View (Kanban)

```
┌─────────────────────────────────────────────────────────────┐
│ Backlog (12)  │ Todo (4)    │ In Progress (3) │ Done (28)   │
├───────────────┼─────────────┼─────────────────┼─────────────┤
│ ┌───────────┐ │ ┌─────────┐ │ ┌─────────────┐ │             │
│ │ TRK-38    │ │ │ TRK-41  │ │ │ TRK-42      │ │             │
│ │ Refactor  │ │ │ Dark    │ │ │ Fix login   │ │             │
│ │ ■ None    │ │ │ ■ Med   │ │ │ ■ Urgent    │ │             │
│ └───────────┘ │ └─────────┘ │ │ @jason      │ │             │
│ ┌───────────┐ │             │ └─────────────┘ │             │
│ │ TRK-37    │ │             │                 │             │
│ │ ...       │ │             │                 │             │
└───────────────┴─────────────┴─────────────────┴─────────────┘
```

## Shared Code Strategy

Start with copy, extract later. Copy the patterns from Kyomi (db.rs, kv_store.rs, auth, sync engine) into the new repo. Don't extract shared crates on day 1 — that creates coupling between two products before either is stable. Once the tracker ships and the interfaces settle, extract the genuinely shared code into standalone crates.

## Competitive Position

| | Tane | Plane | Huly | Linear |
|---|---|---|---|---|
| **Stack** | Rust single binary | Python/Django + React | Svelte + Haskell | Proprietary |
| **Offline** | Yes (local-first) | No | Partial | Yes |
| **Self-hosted** | Zero-dep binary | Docker Compose (5+ services) | Docker Compose | No |
| **MCP server** | Built-in | No | No | Via 3rd party |
| **Opinions** | Fixed workflow | Configurable | Configurable | Fixed workflow |
| **Binary size** | ~20MB | N/A | N/A | N/A |

**Pitch:** Linear's UX philosophy. Ships as a single binary. Works offline. AI-native with built-in MCP. Open source.

## Open Questions

1. ~~**Name**~~ — **Tane** (`tane.dev`) ✓
2. ~~**License**~~ — AGPL-3.0 ✓
3. ~~**Accent color**~~ — Teal `#0D9488` (Tailwind teal-600), hover `#0F766E` (teal-700), light `#CCFBF1` (teal-50) ✓
4. **Markdown images** — URL-only for v1 (no upload infrastructure). Add S3-compatible storage in v2.
5. **API tokens for MCP** — stored hashed in an `api_tokens` table. Generated from settings. Scoped per workspace.
6. ~~**GitHub org**~~ — `jasadams/tane` (private, personal org) ✓
7. **Issue prefix** — configurable workspace slug (e.g. `TRK-42`) or fixed `TANE-42`? Recommendation: configurable slug.
