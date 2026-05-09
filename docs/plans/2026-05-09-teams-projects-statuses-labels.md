# Teams, Projects, Statuses & Labels — Implementation Plan

**Date:** 2026-05-09
**Design doc:** `DESIGN_TEAMS_PROJECTS.md`
**Goal:** Full data model for teams (membership), projects (cross-team), first-class statuses (categories + custom per-team), and dual-scoped labels. Minimal UI integration — no new pages.

---

## Phase 1: First-class Statuses

Replace `issues.status VARCHAR(20)` with a `statuses` table and FK. This is the most invasive change — touches issues end-to-end.

### Task 1.1: SQL Migration — statuses table + issues.status_id

**Files to create:**
- `apps/server/migrations/20260509000000_statuses.sql`
- `apps/server/migrations-sqlite/20260509000000_statuses.sql`

**Steps:**
1. Create `statuses` table:
   ```sql
   CREATE TABLE IF NOT EXISTS statuses (
       status_id    VARCHAR(50) PRIMARY KEY,
       workspace_id VARCHAR(50) NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
       team_id      VARCHAR(50) REFERENCES teams(team_id) ON DELETE CASCADE,
       name         VARCHAR(100) NOT NULL,
       category     VARCHAR(20) NOT NULL,
       position     INTEGER DEFAULT 0,
       color        VARCHAR(20),
       created_at   TIMESTAMPTZ DEFAULT NOW()
   );
   ```
   Add unique index on `(workspace_id, COALESCE(team_id, ''), name)`.

2. Add `status_id VARCHAR(50)` column to `issues` (nullable initially for migration).

3. Seed global default statuses for every existing workspace. Use deterministic IDs based on workspace_id so the migration is idempotent:
   - `{workspace_id}::backlog` → name "Backlog", category "backlog", position 0
   - `{workspace_id}::triage` → name "Triage", category "backlog", position 1
   - `{workspace_id}::todo` → name "Todo", category "unstarted", position 0
   - `{workspace_id}::in_progress` → name "In Progress", category "started", position 0
   - `{workspace_id}::done` → name "Done", category "completed", position 0
   - `{workspace_id}::cancelled` → name "Cancelled", category "cancelled", position 0

4. Migrate existing issues: `UPDATE issues SET status_id = workspace_id || '::' || status` (maps "backlog" → `{ws}::backlog`, "in_progress" → `{ws}::in_progress`, etc.)

5. Make `status_id` NOT NULL, add FK constraint to statuses.

6. Drop the old `status` column.

**SQLite variant:** Same logic but use `datetime('now')` instead of `NOW()`, `TEXT` instead of `VARCHAR`, and no `TIMESTAMPTZ`. Use the same `sql_compat` patterns from the baseline migration.

**Verification:** After migration, every issue has a valid `status_id` pointing to a status row. No orphaned references.

---

### Task 1.2: Rust Types — StatusCategory enum + Status struct

**Files to modify:**
- `crates/trakkt-types/src/enums.rs`
- `crates/trakkt-types/src/models.rs`
- `crates/trakkt-types/src/sync.rs`

**Changes:**

1. **`trakkt-types/src/enums.rs`** — Replace `IssueStatus` with `StatusCategory`:
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum StatusCategory {
       Backlog,
       Unstarted,
       Started,
       Completed,
       Cancelled,
   }
   ```
   Add `all()`, `as_str()`, `Display`, `icon_name()` (returns a static key the UI maps to an SVG).

2. **`trakkt-types/src/models.rs`** — Add `Status` struct:
   ```rust
   pub struct Status {
       pub status_id: String,
       pub workspace_id: String,
       pub team_id: Option<String>,
       pub name: String,
       pub category: String,
       pub position: i32,
       pub color: Option<String>,
       pub created_at: String,
   }
   ```

3. **Update `Issue`:** Replace `status: String` with `status_id: String`.

4. **Update `IssueWithDetails`:** Replace `status: String` with:
   ```rust
   pub status_id: String,
   pub status_name: String,
   pub status_category: String,
   ```

5. **Update `IssueUpdate`:** Replace `status: Option<String>` with `status_id: Option<String>`.

6. **Update `IssueFilters`:** Replace `status: Option<String>` with `status_id: Option<String>`.

7. **`trakkt-types/src/sync.rs`** — Add `STATUS` to `entity_types` constants.

---

### Task 1.3: Service Layer — status_service + issue_service updates

**Files to create:**
- `crates/trakkt-auth/src/status_service.rs`

**Files to modify:**
- `crates/trakkt-auth/src/lib.rs` — add `pub mod status_service;`
- `crates/trakkt-auth/src/issue_service.rs`
- `crates/trakkt-auth/src/user_service.rs` — seed statuses on workspace creation

**status_service.rs:**
- `list_statuses(db, workspace_id, team_id: Option<&str>) -> Result<Vec<Status>>`
  Query: `WHERE workspace_id = $1 AND (team_id IS NULL OR team_id = $2) ORDER BY category, position`
  If `team_id` is None, return only global statuses.
- `get_default_status(db, workspace_id) -> Result<Status>`
  Returns the first status in the `backlog` category (for new issue creation).
- `create_status(db, workspace_id, team_id, name, category, position, color) -> Result<Status>`
- `seed_default_statuses(db, workspace_id) -> Result<()>`
  Creates the 6 default global statuses. Called during workspace provisioning.

**issue_service.rs:**
- Update `IssueRow`: replace `status: String` with `status_id: String`.
- Update `IssueDetailRow`: replace `status: String` with `status_id: String, status_name: String, status_category: String`.
- Update `ISSUE_DETAIL_SELECT`: JOIN statuses table, select `s.status_id, s.name AS status_name, s.category AS status_category`.
- Update `create_issue`: instead of hardcoding `'backlog'`, call `status_service::get_default_status()` to get the default `status_id`, and INSERT that.
- Update `update_issue`: the `status` field in `IssueUpdate` becomes `status_id` — write `status_id` to the column.
- Update `list_issues` filter: `status` filter becomes `status_id` filter on `i.status_id = $N`.

**user_service.rs:**
- In `create_workspace_for_user()`, after creating the default team, call `status_service::seed_default_statuses(db, workspace_id)`.

**auto_provision_personal_mode (apps/server/src/lib.rs):**
- After creating the default team, seed default statuses for `workspace-local` using the same deterministic IDs.

---

### Task 1.4: Server Functions + MCP — wire status_id through the API

**Files to modify:**
- `crates/trakkt-ui/src/server_fns/issues.rs` — `update_issue` param `status` becomes `status_id`; `list_issues` param `status` becomes `status_id`
- `crates/trakkt-ui/src/server_fns/mod.rs` — register new status module
- `apps/server/src/routes/mcp.rs` — update `list_issues` filter, `update_issue` tool, `create_issue` to reference status_id

**Files to create:**
- `crates/trakkt-ui/src/server_fns/statuses.rs`:
  - `list_statuses() -> Result<Vec<Status>, ServerFnError>` — returns statuses for current workspace (global + current team)

---

### Task 1.5: UI Wiring — badge, board, list, detail

**Files to modify:**

1. **`components/issue_status_badge.rs`** — `IssueStatusVariant` now maps to `StatusCategory`:
   - Rename `IssueStatusVariant` to keep the name but change `parse()` to accept a category string ("backlog", "unstarted", "started", "completed", "cancelled") instead of a status string.
   - Add `Unstarted` variant (was `Todo`), `Started` variant (was `InProgress`), `Completed` variant (was `Done`). Keep same SVG icons — Unstarted gets Todo's icon, Started gets InProgress's icon, Completed gets Done's icon.
   - Update all call sites to pass `status_category` instead of `status`.

2. **`pages/board.rs`**:
   - Remove hardcoded `STATUS_COLUMNS` const.
   - Fetch statuses via `list_statuses()` server function at page load.
   - Group statuses by category for column rendering. Each column shows statuses in that category.
   - Update `StatusColumn` struct to use `status_id: String` as the key (for drag-drop status assignment).
   - On drop: call `update_issue` with `status_id` (the target column's status_id).

3. **`pages/issues/issue_list.rs`**:
   - `StatusFilterDropdown`: fetch statuses from `list_statuses()` instead of using `IssueStatus::all()`.
   - Filter compares `issue.status_id` instead of `issue.status`.
   - `IssueRow`: pass `issue.status_category` to `IssueStatusBadge`.

4. **`pages/issues/issue_detail.rs`**:
   - `MetadataBar` status dropdown: populate from `list_statuses()`, show status names, send `status_id` on change.
   - Pass `status_category` to `IssueStatusBadge`.

5. **`cache/store.rs`** (SyncStore):
   - Update any field references from `status` to `status_id`. The SyncStore deserializes `IssueWithDetails` from JSON — field name change flows automatically from the struct update.
   - Add `Status` entity handling if the sync engine caches statuses.

---

## Phase 2: Team Membership

Add explicit team membership. No access control — membership is for defaults and notification routing.

### Task 2.1: SQL Migration — team_members + team enhancements

**Files to create:**
- `apps/server/migrations/20260509000001_team_members.sql`
- `apps/server/migrations-sqlite/20260509000001_team_members.sql`

**Steps:**
1. Add columns to `teams`:
   ```sql
   ALTER TABLE teams ADD COLUMN description TEXT;
   ALTER TABLE teams ADD COLUMN icon VARCHAR(50);
   ```

2. Create `team_members` table:
   ```sql
   CREATE TABLE IF NOT EXISTS team_members (
       team_id    VARCHAR(50) NOT NULL REFERENCES teams(team_id) ON DELETE CASCADE,
       user_id    VARCHAR(50) NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
       role       VARCHAR(20) DEFAULT 'member',
       created_at TIMESTAMPTZ DEFAULT NOW(),
       PRIMARY KEY (team_id, user_id)
   );
   ```

3. Seed: for each existing workspace, insert a `team_members` row for every `workspace_users` member, pointing to that workspace's default team (first team by `created_at`).

---

### Task 2.2: Rust Types — TeamMember struct, Team update

**Files to modify:**
- `crates/trakkt-types/src/models.rs`

**Changes:**
1. Update `Team` struct — add `description: Option<String>`, `icon: Option<String>`.
2. Add `TeamMember` struct:
   ```rust
   pub struct TeamMember {
       pub team_id: String,
       pub user_id: String,
       pub role: String,
       pub created_at: String,
   }
   ```
   Note: this is the issue-tracker team member, distinct from the existing `TeamMember` struct in trakkt-types which represents workspace members. Rename the existing one to `WorkspaceMember` to avoid collision, or namespace appropriately.

Check: the existing `TeamMember` in trakkt-types is actually a workspace member (used in settings/team.rs for member management). Rename it to `WorkspaceMember` and update all references in:
- `crates/trakkt-ui/src/pages/settings/team.rs`
- `crates/trakkt-ui/src/server_fns/team.rs`
- Any other files that reference it

Add the new `TeamMember` (issue-tracker team membership):
```rust
pub struct IssueTeamMember {
    pub team_id: String,
    pub user_id: String,
    pub user_name: Option<String>,
    pub user_email: String,
    pub role: String,
    pub created_at: String,
}
```

---

### Task 2.3: Service Layer — team membership functions

**Files to modify:**
- `crates/trakkt-auth/src/team_service.rs`

**Add functions:**
- `list_team_members(db, team_id) -> Result<Vec<IssueTeamMember>>`
- `add_team_member(db, team_id, user_id, role) -> Result<()>`
- `remove_team_member(db, team_id, user_id) -> Result<()>`
- `update_team_member_role(db, team_id, user_id, role) -> Result<()>`
- `get_user_teams(db, workspace_id, user_id) -> Result<Vec<Team>>` — teams the user belongs to

**Update existing:**
- `create_team`: also accept `description` and `icon` params. After creating the team, add the creating user as a member with role `lead`.
- Update `Team` row type to include `description` and `icon`.

---

### Task 2.4: Provisioning — seed team_members automatically

**Files to modify:**
- `crates/trakkt-auth/src/user_service.rs` — `create_workspace_for_user()`: after creating the default team, add the user as a `lead` member of that team.
- `crates/trakkt-auth/src/onboarding_service.rs` — if workspace creation happens here, same change.
- `apps/server/src/lib.rs` — `auto_provision_personal_mode()`: after creating team-local, add user-local as a team member.
- Workspace invitation acceptance flow: when a user joins a workspace, add them as a member of the default team.

---

## Phase 3: Label Scoping

Add optional team-level scoping to labels. Workspace-level labels remain available to all teams.

### Task 3.1: SQL Migration — team_id on labels

**Files to create:**
- `apps/server/migrations/20260509000002_label_scoping.sql`
- `apps/server/migrations-sqlite/20260509000002_label_scoping.sql`

**Steps:**
1. Add `team_id` column to `labels`:
   ```sql
   ALTER TABLE labels ADD COLUMN team_id VARCHAR(50) REFERENCES teams(team_id) ON DELETE CASCADE;
   ```

2. Drop existing unique constraint `(workspace_id, name)` and replace with partial indexes:
   ```sql
   CREATE UNIQUE INDEX idx_labels_workspace_unique
     ON labels (workspace_id, name) WHERE team_id IS NULL;
   CREATE UNIQUE INDEX idx_labels_team_unique
     ON labels (team_id, name) WHERE team_id IS NOT NULL;
   ```

3. Existing labels keep `team_id = NULL` (workspace-scoped). No data migration needed.

---

### Task 3.2: Rust Types — Label struct update

**Files to modify:**
- `crates/trakkt-types/src/models.rs`

**Changes:**
- Add `team_id: Option<String>` to `Label` struct.

---

### Task 3.3: Service + Server Functions — scoped label queries

**Files to modify:**
- `crates/trakkt-auth/src/label_service.rs`
- `crates/trakkt-ui/src/server_fns/labels.rs`

**label_service.rs:**
- `list_labels(db, workspace_id)` → change query to include `team_id` in SELECT.
- Add `list_labels_for_team(db, workspace_id, team_id) -> Result<Vec<Label>>` — returns workspace-level labels (team_id IS NULL) + team-specific labels (team_id = $2).
- `create_label` — accept optional `team_id` param.

**labels.rs (server fns):**
- `list_labels()` — optionally accept a `team_id` param. If provided, use `list_labels_for_team`. Otherwise return all workspace labels.
- `create_label` — accept optional `team_id`.

---

### Task 3.4: UI — label picker respects team scope

**Files to modify:**
- `crates/trakkt-ui/src/pages/issues/issue_detail.rs` — `LabelPicker`: call `list_labels(team_id)` with the issue's team_id so the picker shows workspace + team labels.
- `crates/trakkt-ui/src/pages/issues/issue_list.rs` — `NewIssueModal`: if label picker is present, scope it to the default team.
- `crates/trakkt-ui/src/pages/settings/labels.rs` — label management page shows all labels with a "(workspace)" or "(Team Name)" scope indicator. No team-scoped creation UI yet — just display scope.

---

## Phase 4: Projects

Full project data model. No UI — just schema, types, service, and server functions.

### Task 4.1: SQL Migration — project tables + issue FKs

**Files to create:**
- `apps/server/migrations/20260509000003_projects.sql`
- `apps/server/migrations-sqlite/20260509000003_projects.sql`

**Tables to create:**

1. `projects`:
   ```sql
   CREATE TABLE IF NOT EXISTS projects (
       project_id    VARCHAR(50) PRIMARY KEY,
       workspace_id  VARCHAR(50) NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
       name          VARCHAR(255) NOT NULL,
       description   TEXT,
       icon          VARCHAR(50),
       color         VARCHAR(20),
       status        VARCHAR(20) DEFAULT 'planned',
       lead_id       VARCHAR(50) REFERENCES users(user_id),
       start_date    DATE,
       target_date   DATE,
       sort_order    REAL DEFAULT 0,
       created_at    TIMESTAMPTZ DEFAULT NOW(),
       updated_at    TIMESTAMPTZ DEFAULT NOW()
   );
   ```

2. `project_members`:
   ```sql
   CREATE TABLE IF NOT EXISTS project_members (
       project_id VARCHAR(50) NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
       user_id    VARCHAR(50) NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
       role       VARCHAR(20) DEFAULT 'member',
       created_at TIMESTAMPTZ DEFAULT NOW(),
       PRIMARY KEY (project_id, user_id)
   );
   ```

3. `project_milestones`:
   ```sql
   CREATE TABLE IF NOT EXISTS project_milestones (
       milestone_id VARCHAR(50) PRIMARY KEY,
       project_id   VARCHAR(50) NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
       name         VARCHAR(255) NOT NULL,
       description  TEXT,
       target_date  DATE,
       sort_order   INTEGER DEFAULT 0,
       created_at   TIMESTAMPTZ DEFAULT NOW()
   );
   ```

4. `project_updates`:
   ```sql
   CREATE TABLE IF NOT EXISTS project_updates (
       update_id  VARCHAR(50) PRIMARY KEY,
       project_id VARCHAR(50) NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
       user_id    VARCHAR(50) NOT NULL REFERENCES users(user_id),
       health     VARCHAR(20) NOT NULL,
       body       TEXT,
       created_at TIMESTAMPTZ DEFAULT NOW()
   );
   ```

5. Add FKs to `issues`:
   ```sql
   ALTER TABLE issues ADD COLUMN project_id VARCHAR(50) REFERENCES projects(project_id) ON DELETE SET NULL;
   ALTER TABLE issues ADD COLUMN milestone_id VARCHAR(50) REFERENCES project_milestones(milestone_id) ON DELETE SET NULL;
   ```

---

### Task 4.2: Rust Types — project structs

**Files to modify:**
- `crates/trakkt-types/src/models.rs`
- `crates/trakkt-types/src/sync.rs`

**Add to models.rs:**
```rust
pub struct Project {
    pub project_id: String,
    pub workspace_id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub status: String,
    pub lead_id: Option<String>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub sort_order: f64,
    pub created_at: String,
    pub updated_at: String,
}

pub struct ProjectMember {
    pub project_id: String,
    pub user_id: String,
    pub role: String,
    pub created_at: String,
}

pub struct ProjectMilestone {
    pub milestone_id: String,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub target_date: Option<String>,
    pub sort_order: i32,
    pub created_at: String,
}

pub struct ProjectUpdate {
    pub update_id: String,
    pub project_id: String,
    pub user_id: String,
    pub health: String,
    pub body: Option<String>,
    pub created_at: String,
}
```

**Update `Issue`:** Add `project_id: Option<String>`, `milestone_id: Option<String>`.

**Update `IssueWithDetails`:** Add `project_id: Option<String>`, `project_name: Option<String>`, `milestone_id: Option<String>`.

**Add to `CreateIssueParams`:** `project_id: Option<String>`, `milestone_id: Option<String>`.

**Add to `IssueUpdate`:** `project_id: Option<Option<String>>`, `milestone_id: Option<Option<String>>`.

**`sync.rs`:** Add `PROJECT` and `PROJECT_MILESTONE` to entity_types.

---

### Task 4.3: Service Layer — project_service

**Files to create:**
- `crates/trakkt-auth/src/project_service.rs`

**Files to modify:**
- `crates/trakkt-auth/src/lib.rs` — add `pub mod project_service;`
- `crates/trakkt-auth/src/issue_service.rs` — update queries to SELECT project_id, milestone_id; update ISSUE_DETAIL_SELECT to LEFT JOIN projects for project_name; update create/update to handle project_id and milestone_id.

**project_service.rs functions:**
- `list_projects(db, workspace_id) -> Result<Vec<Project>>`
- `get_project(db, project_id) -> Result<Option<Project>>`
- `create_project(db, workspace_id, name, description, icon, color, lead_id, start_date, target_date) -> Result<Project>`
- `update_project(db, project_id, ...) -> Result<Project>`
- `delete_project(db, project_id) -> Result<()>`
- `list_project_members(db, project_id) -> Result<Vec<ProjectMember>>`
- `add_project_member(db, project_id, user_id, role) -> Result<()>`
- `remove_project_member(db, project_id, user_id) -> Result<()>`
- `list_milestones(db, project_id) -> Result<Vec<ProjectMilestone>>`
- `create_milestone(db, project_id, name, description, target_date) -> Result<ProjectMilestone>`
- `update_milestone(db, milestone_id, ...) -> Result<ProjectMilestone>`
- `delete_milestone(db, milestone_id) -> Result<()>`
- `create_project_update(db, project_id, user_id, health, body) -> Result<ProjectUpdate>`
- `list_project_updates(db, project_id) -> Result<Vec<ProjectUpdate>>`
- `get_project_progress(db, project_id) -> Result<ProjectProgress>` — computes % of issues in completed/cancelled category

---

### Task 4.4: Server Functions — project CRUD (no UI)

**Files to create:**
- `crates/trakkt-ui/src/server_fns/projects.rs`

**Files to modify:**
- `crates/trakkt-ui/src/server_fns/mod.rs` — register projects module
- `crates/trakkt-ui/src/server_fns/issues.rs` — `create_issue` and `update_issue` accept optional `project_id` and `milestone_id`

**projects.rs server functions:**
- `list_projects() -> Result<Vec<Project>>`
- `get_project(project_id: String) -> Result<Option<Project>>`
- `create_project(name, description, icon, color, lead_id, start_date, target_date) -> Result<Project>`
- `update_project(project_id, name, description, ...) -> Result<Project>`
- `delete_project(project_id) -> Result<()>`
- `list_project_members(project_id) -> Result<Vec<ProjectMember>>`
- `add_project_member(project_id, user_id, role) -> Result<()>`
- `remove_project_member(project_id, user_id) -> Result<()>`
- `list_milestones(project_id) -> Result<Vec<ProjectMilestone>>`
- `create_milestone(project_id, name, description, target_date) -> Result<ProjectMilestone>`

No UI pages — these server functions exist so future UI work is just wiring components.

---

## Execution Notes

- Each phase is a single commit after all its tasks pass code review.
- Tasks within a phase are sequential (each depends on the previous).
- Phases are sequential (2 depends on 1's migration being in place, etc.).
- Both Postgres and SQLite migrations must be written for every schema change.
- All service functions follow the existing pattern: raw SQL via `db_*!` macros, `sql_compat` for dialect differences.
- ID generation: use `uuid::Uuid::new_v4().to_string()` for new entities (statuses use deterministic IDs during seed only).
- The build must compile and the app must run after each phase. No partial migrations.
