# Trakkt Implementation Plan

**Orchestration document. Each slice is a self-contained, shippable increment.**
**Quality bar: would a designer use this product and not wince?**

## Current State

The app starter skeleton is working:
- Auth (password, passkey, Google OAuth) — working
- Settings pages (profile, security, team/workspace) — working
- MCP infrastructure (endpoint, no domain tools) — working
- Leptos + Trunk + Tailwind design system — working
- E2E test suite: 169 tests, 0 failures

## What We're Building

A fully working issue tracker. Every page must feel like a product, not a prototype.

## Slice Order

Each slice must be fully working and browser-verified before moving to the next.

---

### Slice 1: Database Schema

**Goal:** All issue-tracker tables exist and migrations run cleanly on both Postgres and SQLite.

**What to build:**
- `apps/server/migrations/002_issue_tracker.sql` (Postgres)
- `apps/server/migrations-sqlite/002_issue_tracker.sql` (SQLite)

**Tables (from ARCHITECTURE.md):**
- `teams` — team_id, workspace_id, name, key, created_at. UNIQUE(workspace_id, key)
- `issues` — issue_id, workspace_id, team_id, number, title, description, status, priority, assignee_id, creator_id, due_date, created_at, updated_at. UNIQUE(workspace_id, number)
- `labels` — label_id, workspace_id, name, color, created_at. UNIQUE(workspace_id, name)
- `issue_labels` — issue_id, label_id. Composite PK. CASCADE deletes.
- `comments` — comment_id, issue_id, user_id, body, parent_id (one-level threading), created_at, updated_at
- `notifications` — notification_id, workspace_id, user_id, issue_id, type, read, created_at
- `issue_watchers` — issue_id, user_id. Composite PK. CASCADE deletes.
- `api_tokens` — token_id, workspace_id, user_id, name, token_hash, token_prefix, scopes (JSON), last_used_at, expires_at, created_at

**Indexes:**
- `idx_issues_workspace_status` on issues(workspace_id, status)
- `idx_issues_workspace_team_number` on issues(workspace_id, team_id, number)
- `idx_issues_assignee` on issues(assignee_id)
- `idx_comments_issue` on comments(issue_id, created_at)
- `idx_notifications_user_unread` on notifications(user_id, read, created_at)
- `idx_sync_log_workspace_cursor` on sync_log(workspace_id, sync_id)
- `idx_labels_workspace` on labels(workspace_id)
- `idx_teams_workspace` on teams(workspace_id)

**Auto-provision update:** When personal mode creates a workspace, also create a default team with key derived from workspace slug (uppercase, e.g., "TRK").

**Acceptance criteria:**
- [ ] Server boots with fresh DB, migrations run, tables exist
- [ ] Personal mode auto-provisions workspace + default team
- [ ] `cargo test` passes (existing tests still work)
- [ ] E2E test suite still passes (169 tests, 0 failures)

---

### Slice 2: Types + Service Layer

**Goal:** Full backend services for issues, labels, comments, teams, notifications. Real SQL, real queries, tested.

**What to build:**

**In trakkt-types/src/:**
- `enums.rs` — IssueStatus (Backlog, Todo, InProgress, Done, Cancelled), Priority (None=0, Urgent=1, High=2, Medium=3, Low=4), WorkspaceRole
- `models.rs` — Team, Issue, IssueWithDetails, Label, Comment, Notification, IssueFilters, IssueUpdate
- Update `sync.rs` — entity type constants for ISSUE, COMMENT, LABEL, NOTIFICATION, TEAM

**In trakkt-auth/src/:**
- `team_service.rs` — create_team, list_teams, get_team, get_team_by_key, get_default_team
- `issue_service.rs` — create_issue (auto-increment number per team), get_issue, list_issues (with filters: status, priority, assignee, label, search, limit, offset), update_issue, delete_issue, set_issue_labels
- `label_service.rs` — create_label, list_labels, update_label, delete_label, get_issue_labels
- `comment_service.rs` — create_comment (with parent_id for threading), list_comments (ordered, with author info), update_comment, delete_comment
- `notification_service.rs` — create_notification, list_notifications, mark_as_read, mark_all_as_read, count_unread

**All services use the db_fetch_all!/db_execute! macro pattern from trakkt-core.** Every write appends to sync_log.

**Acceptance criteria:**
- [ ] All services compile and have correct SQL for both Postgres and SQLite
- [ ] Integration tests: create team → create issue → update issue → list issues with filters → delete issue
- [ ] Integration tests: create label → attach to issue → list labels for issue
- [ ] Integration tests: create comment → reply to comment → list threaded comments
- [ ] Issue numbers auto-increment per team (not globally)
- [ ] List issues supports filtering by status, priority, assignee, label, and text search
- [ ] `cargo test` all pass
- [ ] E2E suite still passes

---

### Slice 3: Server Functions

**Goal:** Leptos server functions that bridge UI to services. Thin wrappers with auth extraction.

**What to build in trakkt-ui/src/server_fns/:**
- `teams.rs` — list_teams, create_team, get_default_team
- `issues.rs` — list_issues, get_issue, create_issue, update_issue, delete_issue, set_issue_labels
- `comments.rs` — list_comments, create_comment, update_comment, delete_comment
- `labels.rs` — list_labels, create_label, update_label, delete_label
- `notifications.rs` — list_notifications, mark_notification_read, mark_all_read, count_unread

**All server functions:**
- Use `#[server(prefix = "/leptos-api")]`
- Extract auth context via `extract_auth()`
- Get workspace_id and team_id from auth claims
- Delegate to trakkt-auth services
- Return proper ServerFnError on failures

**Update lib.rs** — register all new server functions in `register_server_functions()`.

**Acceptance criteria:**
- [ ] All server functions compile for both native and WASM targets
- [ ] Server boots, server functions respond to HTTP requests
- [ ] Auth is enforced — unauthenticated requests fail
- [ ] E2E test: login → create issue via server function → fetch it back
- [ ] E2E suite still passes

---

### Slice 4: UI Components (Design System)

**Goal:** All reusable components from DESIGN.md, fully styled, accessible, keyboard-navigable.

**What to build in trakkt-ui/src/components/:**

Each component must match DESIGN.md specs exactly. Not "close enough."

- `button.rs` — 6 variants (Primary, Secondary, Ghost, GhostMuted, Destructive, Outline), 2 sizes, disabled states, focus-visible ring, transition-colors. DM Sans 600 weight.
- `card.rs` — Card, CardHeader, CardTitle, CardDescription, CardContent. bg-card, border, rounded-lg, shadow-sm.
- `modal.rs` — Sizes sm/md/lg. bg-black/50 backdrop. Escape to close. Focus trap. Animate in (zoom-fade-in). Rounded-lg, shadow-lg.
- `confirm_dialog.rs` — Wraps Modal. Title, message, confirm/cancel buttons. Destructive variant with red confirm button.
- `toast.rs` — 4 variants (success, error, warning, info). Auto-dismiss 5s. Animated slide-in. Toast provider context + use_toast() hook.
- `status_badge.rs` — Colored dot + status text. Colors from DESIGN.md status colors section. Dot is rounded-full w-2 h-2.
- `priority_icon.rs` — Colored square/indicator for each priority level. Colors from DESIGN.md.
- `label_badge.rs` — Colored pill. Dynamic background from label.color. Auto text contrast (white on dark, black on light). text-xs px-1.5 py-0.5 rounded-sm.
- `search_input.rs` — Search icon (Phosphor MagnifyingGlass), input field, clear button (X icon, appears when has value). Debounced value signal (300ms). Rounded-md, border, focus ring.
- `dropdown.rs` — Trigger button + floating panel. Keyboard: arrow keys navigate, Enter selects, Escape closes. Used for status/priority/assignee pickers. Animated entry.
- `avatar.rs` — Image or initials fallback. Rounded-full. Sizes: sm (20px), md (28px), lg (36px). Initials derived from display_name.
- `skeleton.rs` — Animated pulse placeholder. Rounded. Configurable width/height. bg-muted.
- `spinner.rs` — Teal accent loading spinner. SVG circle animation matching the loading screen spinner.
- `empty_state.rs` — Centered layout: Phosphor icon (Duotone weight, 64px, teal), heading (text-xl font-semibold), description (text-muted-foreground), optional action Button.

**Every component must have:**
- Proper focus-visible:ring-1 ring-ring on interactive elements
- disabled:opacity-50 disabled:cursor-not-allowed on disableable elements
- transition-colors on hover states
- prefers-reduced-motion support
- No raw HTML — components compose other components

**Acceptance criteria:**
- [ ] All components compile for both native and WASM
- [ ] Visual verification in browser: each component rendered with real props
- [ ] Keyboard navigation works on all interactive components (dropdown, modal, search)
- [ ] Dark mode works (toggle theme, verify all components adapt)
- [ ] Focus rings visible and correctly colored (teal)
- [ ] Transitions smooth (100ms color, 200ms position/size)
- [ ] Empty states use Duotone Phosphor icons

---

### Slice 5: Issue List Page

**Goal:** The issue list page — the first thing users see. This sets the quality bar for the entire app.

**What to build:** `trakkt-ui/src/pages/issue_list.rs`

**Layout (from DESIGN.md):**
```
bg-background (flex flex-col h-full)
├── Page header (h-16 px-4 md:px-6): "Issues" title (Instrument Serif, text-3xl) + [+ New Issue] primary button
├── Toolbar (bg-background px-4 md:px-6 py-3): SearchInput + filter dropdowns (Status, Priority, Assignee, Label)
├── Issue rows (flex-1 overflow-y-auto):
│   Each row: status dot + issue number (team key + number, Geist Mono text-xs text-muted) + title (DM Sans text-sm font-medium) + label pills + priority icon + assignee avatar
│   Row hover: bg-surface-alt with transition-colors
│   Row border: border-b border-border
│   Click: navigate to /issues/:number
└── Empty state when no issues (Duotone clipboard icon, "No issues yet", "Create your first issue to get started", primary button)
```

**States that must all work:**
- **Loading:** Content-shaped skeleton placeholders (not a spinner). Multiple skeleton rows matching issue row shape.
- **Empty:** Empty state component with Duotone icon, heading, description, CTA button.
- **Populated:** Issue rows with all metadata visible. Correct colors for status dots, priority icons.
- **Filtered:** Filter dropdowns work. Selecting a status filter shows only matching issues. Combining filters works (status + priority). Search debounces and filters by title.
- **Hover:** Rows highlight bg-surface-alt on hover with smooth transition.
- **New issue:** Modal opens (triggered by button or 'c' key). Form has: title (required), description (kode WYSIWYG editor), priority dropdown, assignee dropdown, label multi-select. Submit creates issue, closes modal, issue appears in list without page refresh.

**Keyboard navigation (v1 requirement from DESIGN.md):**
- `j` / `k` — move selection down/up (highlighted row)
- `Enter` — open selected issue (navigate to detail page)
- `x` — toggle row selection (for future bulk actions)
- `c` — open new issue modal
- `Escape` — close modal

**New issue modal spec:**
- Modal size: lg
- Title input: required, auto-focused on open
- Description: kode MarkdownEditorComponent in WYSIWYG mode, themed to match Trakkt
- Priority: dropdown, defaults to None
- Labels: multi-select pills
- Assignee: dropdown with workspace member avatars
- Submit button: "Create Issue" primary button, disabled while submitting
- Cancel: ghost button or Escape key

**Acceptance criteria:**
- [ ] Boot server, login, see issue list page
- [ ] Loading skeletons appear while data loads
- [ ] Empty state shows when no issues exist
- [ ] Create issue via modal — issue appears in list immediately
- [ ] Issue row shows: status dot, team key + number, title, labels, priority, assignee
- [ ] Filters work: status, priority, label, assignee, search
- [ ] Hover highlighting works with smooth transition
- [ ] j/k keyboard navigation moves highlighted row
- [ ] Enter on highlighted row navigates to detail
- [ ] c opens new issue modal
- [ ] Escape closes modal
- [ ] kode WYSIWYG editor works in the new issue modal for description
- [ ] Dark mode: everything adapts correctly
- [ ] E2E tests cover: create issue, filter issues, keyboard nav

---

### Slice 6: Issue Detail Page

**Goal:** Full issue detail with inline editing, metadata controls, comments with threading.

**What to build:** `trakkt-ui/src/pages/issue_detail.rs`

**Layout (from DESIGN.md):**
```
bg-background (flex flex-col h-full)
├── Header (h-16): back button (ghost icon, Phosphor ArrowLeft) + issue number (Geist Mono)
├── Content (flex-1 overflow-y-auto p-4 md:p-6, max-w-[860px] mx-auto)
│   ├── Title: text-2xl font-display (Instrument Serif), click to edit inline
│   ├── Metadata bar: status dropdown, priority dropdown, assignee dropdown, label pills (click to add/remove), due date
│   ├── Description: kode MarkdownEditorComponent in WYSIWYG mode
│   │   Click to enter edit mode, save on blur or Cmd+Enter
│   ├── Divider (border-t border-border my-6)
│   └── Comments section
│       ├── Comment count heading
│       ├── Comment list (ordered by created_at)
│       │   Each comment: avatar (left) + name + timestamp (relative) + markdown body
│       │   Threaded replies: indented one level, connected with subtle line
│       │   Own comments: edit/delete actions on hover
│       └── New comment: kode WYSIWYG editor (compact), "Comment" primary button
└── Footer: created/updated timestamps (text-xs text-muted)
```

**Interactions:**
- **Title inline edit:** Click title → becomes input → Enter or blur saves → optimistic update
- **Status dropdown:** Click → dropdown with all 5 statuses, colored dots → select updates immediately
- **Priority dropdown:** Click → dropdown with 5 priorities, colored indicators → select updates immediately
- **Assignee dropdown:** Click → dropdown with workspace members, avatars → select updates immediately
- **Labels:** Displayed as pills. Click "+" to add. Click "x" on pill to remove. Label picker shows all workspace labels with colors.
- **Description edit:** Click to focus kode editor. Changes save on blur.
- **Comments:** Markdown rendered. Reply button on each comment opens indented reply form. Edit own comments inline.
- **Relative timestamps:** "2m ago", "1h ago", "3d ago", "May 5"

**Acceptance criteria:**
- [ ] Navigate to issue detail from list (click or Enter)
- [ ] Back button returns to issue list
- [ ] Title displays in Instrument Serif, editable inline
- [ ] All metadata dropdowns work (status, priority, assignee, labels)
- [ ] Description renders markdown, editable with kode WYSIWYG
- [ ] Comments display with threading (replies indented)
- [ ] Can add new comment with kode WYSIWYG editor
- [ ] Can reply to a comment (one level)
- [ ] Can edit/delete own comments
- [ ] Relative timestamps display correctly
- [ ] Changes persist (reload page, data still there)
- [ ] Dark mode works
- [ ] E2E tests cover: edit title, change status, add comment, reply to comment

---

### Slice 7: Board View (Kanban)

**Goal:** Drag-and-drop kanban board grouped by status.

**What to build:** `trakkt-ui/src/pages/board.rs`

**Layout (from DESIGN.md):**
```
bg-background (flex flex-col h-full)
├── Page header (h-16): "Board" title (Instrument Serif)
├── Content (flex-1 overflow-x-auto px-4 md:px-6 py-4)
│   └── Columns (flex gap-4)
│       Each column: min-w-[280px] max-w-[320px]
│       ├── Column header (sticky top): status name + issue count badge
│       └── Issue cards (flex flex-col gap-2)
│           Each card: bg-card border border-border rounded-md p-4 shadow-sm
│           ├── Issue number (Geist Mono, text-xs, text-muted)
│           ├── Title (DM Sans, text-sm, font-medium)
│           ├── Label pills (flex gap-1)
│           └── Footer: priority icon + assignee avatar
│           Card hover: shadow-md transition
```

**Columns:** Backlog, Todo, In Progress, Done, Cancelled (in that order)

**Drag and drop:**
- Drag a card between columns to change status
- Visual feedback: drop target column highlights, card shows ghost at insertion point
- On drop: update issue status via server function, optimistic UI update
- Use HTML5 drag API or minimal Rust drag library — no heavy JS dependency

**Acceptance criteria:**
- [ ] Board renders 5 columns with correct status names and counts
- [ ] Issue cards show number, title, labels, priority, assignee
- [ ] Cards are draggable between columns
- [ ] Dropping a card in a different column changes its status
- [ ] Status change persists (reload → card still in new column)
- [ ] Card hover: shadow-md
- [ ] Horizontal scroll works when columns overflow
- [ ] Click card navigates to issue detail
- [ ] Empty columns show "No issues" text
- [ ] Dark mode works
- [ ] E2E tests cover: drag card between columns, verify status change

---

### Slice 8: Settings — Labels + Teams Management

**Goal:** Manage labels (CRUD with color picker) and teams (create, view) in settings.

**What to build:** New tabs in settings pages.

**Labels tab:**
- List all workspace labels with color swatch + name
- Create label: name input + color picker (preset palette of 12 colors + custom hex input)
- Edit label: click to edit name/color inline
- Delete label: confirm dialog ("This will remove the label from all issues")
- Labels sorted alphabetically

**Teams tab:**
- List all teams with key + name
- Create team: name input + key input (auto-derived from name, uppercase, editable). Key validation: 2-5 uppercase letters, unique in workspace.
- Default team indicator
- Cannot delete the last team

**Acceptance criteria:**
- [ ] Labels tab: create, edit, delete labels with color picker
- [ ] Color picker: 12 preset colors + custom hex
- [ ] Label changes reflect immediately in issue list/detail/board
- [ ] Teams tab: create teams with auto-derived key
- [ ] Team key validation (uppercase, 2-5 chars, unique)
- [ ] New issues use current team's key for numbering
- [ ] Cannot delete last team
- [ ] Dark mode works
- [ ] E2E tests: create label, edit color, delete label, create team

---

### Slice 9: Sync Engine

**Goal:** Local-first sync — instant UI from IndexedDB, real-time updates via WebSocket.

Reference: `docs/plans/2026-05-01-phase5-sync-engine.md`

**Server-side:**
- WebSocket manager (trakkt-auth/src/websocket/)
- WebSocket route handler (apps/server/src/routes/ws.rs)
- Service layer broadcasts mutations to connected clients

**Client-side:**
- IndexedDB cache (trakkt-ui/src/cache/db.rs)
- SyncStore reactive signals (trakkt-ui/src/cache/store.rs)
- Sync engine + WebSocket client (trakkt-ui/src/cache/)

**Acceptance criteria:**
- [ ] First visit: bootstrap loads all data via WebSocket
- [ ] Return visit: data loads from IndexedDB instantly (no loading flash)
- [ ] Create issue in tab 1 → appears in tab 2 without refresh
- [ ] Status change in board → reflected in list view in another tab
- [ ] Page refresh: data loads from IDB cache, delta sync updates
- [ ] WebSocket reconnects on disconnect (exponential backoff)
- [ ] Schema version mismatch: wipes IDB, re-bootstraps
- [ ] E2E tests: multi-tab sync verification

---

### Slice 10: MCP Domain Tools

**Goal:** AI agents can manage issues via the built-in MCP server.

**Tools to implement:**
- list_issues (with filters)
- get_issue (by number)
- create_issue
- update_issue
- add_comment
- list_labels
- create_label
- search_issues

**All tools go through the same service layer as the UI — mutations hit sync_log and broadcast.**

**Acceptance criteria:**
- [ ] `claude mcp add trakkt http://localhost:8003/mcp` works
- [ ] Agent can list issues, create issues, update status, add comments
- [ ] Changes made by agent appear in browser instantly (via sync)
- [ ] E2E tests: MCP tool calls verify correct responses

---

### Slice 11: Keyboard Navigation + Command Palette

**Goal:** Full keyboard-driven UX matching DESIGN.md spec.

**Command palette (Cmd+K):**
- Modal overlay with search input
- Actions: navigate to issue by number, change view, create issue, go to settings
- Fuzzy search across issue titles
- Arrow keys navigate, Enter executes

**Global shortcuts:**
- j/k in list view (already in Slice 5)
- Escape closes any modal/overlay
- 1-5 set priority on issue detail
- s opens status picker on issue detail
- l opens label picker on issue detail
- a opens assignee picker on issue detail

**Acceptance criteria:**
- [ ] Cmd+K opens command palette from any page
- [ ] Can search and navigate to any issue by title or number
- [ ] All keyboard shortcuts from DESIGN.md work
- [ ] No shortcut conflicts with kode editor when editing
- [ ] E2E tests: keyboard navigation flows

---

### Slice 12: Polish Pass

**Goal:** Visual QA. Go through every page, every state, every interaction and fix anything that doesn't feel right.

**Checklist:**
- [ ] All transitions smooth (no janky state changes)
- [ ] All loading states have skeletons (no blank flashes)
- [ ] All empty states have proper messaging
- [ ] All error states show useful messages
- [ ] Dark mode consistent everywhere
- [ ] Responsive: works on mobile viewport (sidebar collapses)
- [ ] Scrollbars match theme (thin, transparent track)
- [ ] No orphaned console errors
- [ ] Typography consistent (Instrument Serif for headings, DM Sans for body, Geist Mono for data)
- [ ] Focus rings visible and teal on all interactive elements
- [ ] Full E2E regression pass: all tests green
