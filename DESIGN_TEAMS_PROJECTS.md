# Data Model: Teams, Projects, Statuses & Labels

Source of truth for the Trakkt workspace → teams → issues → projects relationship model.

---

## Entity Relationships

```
workspace
│
├── workspace_users (N:N users ↔ workspaces, with role)
│
├── teams
│   ├── team_members (N:N users ↔ teams, with role)
│   ├── statuses (team-specific workflow statuses)
│   ├── labels (team-scoped labels)
│   └── issues
│       ├── status_id → statuses
│       ├── assignee_id → users
│       ├── creator_id → users
│       ├── project_id → projects (nullable, cross-team)
│       ├── milestone_id → project_milestones (nullable)
│       ├── issue_labels (N:N with labels)
│       ├── comments (threaded, self-referential via parent_id)
│       ├── issue_watchers (N:N with users)
│       └── notifications
│
├── statuses (global defaults — team_id IS NULL)
├── labels (workspace-level — team_id IS NULL)
│
├── projects (cross-team initiatives)
│   ├── project_members (N:N users ↔ projects, with role)
│   ├── project_milestones (phases/checkpoints)
│   ├── project_updates (periodic health reports)
│   └── issues (via project_id FK on issues — zero or one project per issue)
│
└── api_tokens, notifications, invitations, ownership_transfers, sync_log
```

---

## Teams

A team is a permanent organizational group within a workspace (e.g. "Engineering",
"Design", "Platform"). Teams own issues and define the key prefix used in issue
identifiers (`ENG-42`, `DES-17`).

```sql
teams
├── team_id       VARCHAR(50) PK
├── workspace_id  VARCHAR(50) FK → workspaces ON DELETE CASCADE
├── name          VARCHAR(255) NOT NULL
├── key           VARCHAR(10) NOT NULL
├── description   TEXT
├── icon          VARCHAR(50)                   -- emoji or icon identifier
├── created_at    TIMESTAMPTZ DEFAULT now()

UNIQUE(workspace_id, key)
```

### Team Members

Explicit membership controls default views and notification routing. Membership does not
restrict access — any workspace member can browse all teams and be assigned to any issue.
Membership determines what a user sees by default.

```sql
team_members
├── team_id     VARCHAR(50) FK → teams ON DELETE CASCADE
├── user_id     VARCHAR(50) FK → users ON DELETE CASCADE
├── role        VARCHAR(20) DEFAULT 'member'
├── created_at  TIMESTAMPTZ DEFAULT now()

PRIMARY KEY (team_id, user_id)
```

**Roles:**
- `member` — standard team participant
- `lead` — team point person; shown in sidebar, receives notifications for unassigned issues

When a new workspace is provisioned, all workspace members are seeded as members of the
default team.

---

## Statuses

Statuses are first-class entities rather than free-text strings. Each status belongs to a
**category** that encodes its semantic meaning — the system always knows what "done" means
regardless of the display name a team chooses.

```sql
statuses
├── status_id     VARCHAR(50) PK
├── workspace_id  VARCHAR(50) FK → workspaces ON DELETE CASCADE
├── team_id       VARCHAR(50) FK → teams ON DELETE CASCADE, NULLABLE
├── name          VARCHAR(100) NOT NULL
├── category      VARCHAR(20) NOT NULL
├── position      INTEGER DEFAULT 0
├── color         VARCHAR(20)
├── created_at    TIMESTAMPTZ DEFAULT now()

UNIQUE(workspace_id, COALESCE(team_id, ''), name)
```

### Scoping

- `team_id IS NULL` — global workspace default, available to all teams
- `team_id IS NOT NULL` — team-specific status, only applies to that team

### Categories

| Category     | Meaning              | Default statuses seeded |
|-------------|----------------------|-------------------------|
| `backlog`    | Not yet planned      | Backlog, Triage         |
| `unstarted`  | Planned, not started | Todo                    |
| `started`    | Active work          | In Progress             |
| `completed`  | Finished             | Done                    |
| `cancelled`  | Won't do             | Cancelled               |

### Resolution

When resolving a team's available statuses, query both global and team-specific:

```sql
WHERE workspace_id = ? AND (team_id IS NULL OR team_id = ?)
ORDER BY category, position
```

Whether team statuses replace or augment globals is a UI-layer decision — the data model
supports both modes.

### Issues FK

Issues reference `status_id` (FK → statuses) instead of a status string. This ensures
referential integrity and makes status renames safe.

---

## Labels

Labels support two scopes: workspace-level (shared across all teams) and team-level
(specific to one team).

```sql
labels
├── label_id      VARCHAR(50) PK
├── workspace_id  VARCHAR(50) FK → workspaces ON DELETE CASCADE
├── team_id       VARCHAR(50) FK → teams ON DELETE CASCADE, NULLABLE
├── name          VARCHAR(100) NOT NULL
├── color         VARCHAR(20) NOT NULL
├── created_at    TIMESTAMPTZ DEFAULT now()
```

### Scoping

- `team_id IS NULL` — workspace-level label, available to all teams
- `team_id IS NOT NULL` — team-scoped label, only available within that team

### Uniqueness

```sql
CREATE UNIQUE INDEX idx_labels_workspace_unique
  ON labels (workspace_id, name) WHERE team_id IS NULL;

CREATE UNIQUE INDEX idx_labels_team_unique
  ON labels (team_id, name) WHERE team_id IS NOT NULL;
```

A team label may shadow a workspace label of the same name. When assigning labels to an
issue, the label picker shows workspace-level labels plus the current team's labels. If
names collide, the team's version takes precedence in that team's context.

---

## Projects

Projects are cross-team initiatives that group issues from any team toward a shared goal.
An issue belongs to zero or one project. Progress is computed dynamically from the ratio
of project issues whose status is in the `completed` or `cancelled` category.

```sql
projects
├── project_id    VARCHAR(50) PK
├── workspace_id  VARCHAR(50) FK → workspaces ON DELETE CASCADE
├── name          VARCHAR(255) NOT NULL
├── description   TEXT
├── icon          VARCHAR(50)
├── color         VARCHAR(20)
├── status        VARCHAR(20) DEFAULT 'planned'
├── lead_id       VARCHAR(50) FK → users, NULLABLE
├── start_date    DATE
├── target_date   DATE
├── sort_order    REAL DEFAULT 0
├── created_at    TIMESTAMPTZ DEFAULT now()
├── updated_at    TIMESTAMPTZ DEFAULT now()
```

**Project statuses:** `planned` | `in_progress` | `paused` | `completed` | `cancelled`

### Project Members

People involved in a project. Independent of team membership — a project pulls people
from across teams.

```sql
project_members
├── project_id  VARCHAR(50) FK → projects ON DELETE CASCADE
├── user_id     VARCHAR(50) FK → users ON DELETE CASCADE
├── role        VARCHAR(20) DEFAULT 'member'
├── created_at  TIMESTAMPTZ DEFAULT now()

PRIMARY KEY (project_id, user_id)
```

**Roles:** `member` | `lead`

### Project Milestones

Named checkpoints within a project. Issues can optionally be grouped under a milestone
for phased tracking.

```sql
project_milestones
├── milestone_id  VARCHAR(50) PK
├── project_id    VARCHAR(50) FK → projects ON DELETE CASCADE
├── name          VARCHAR(255) NOT NULL
├── description   TEXT
├── target_date   DATE
├── sort_order    INTEGER DEFAULT 0
├── created_at    TIMESTAMPTZ DEFAULT now()
```

### Project Updates

Periodic status reports on project health, posted by project leads or members.

```sql
project_updates
├── update_id   VARCHAR(50) PK
├── project_id  VARCHAR(50) FK → projects ON DELETE CASCADE
├── user_id     VARCHAR(50) FK → users
├── health      VARCHAR(20) NOT NULL
├── body        TEXT
├── created_at  TIMESTAMPTZ DEFAULT now()
```

**Health values:** `on_track` | `at_risk` | `off_track`

---

## Issues

Issues are the core work item. Each issue belongs to exactly one team and optionally to
one project and one milestone.

```sql
issues
├── issue_id      VARCHAR(50) PK
├── workspace_id  VARCHAR(50) FK → workspaces ON DELETE CASCADE
├── team_id       VARCHAR(50) FK → teams ON DELETE CASCADE
├── number        INTEGER NOT NULL
├── title         VARCHAR(500) NOT NULL
├── description   TEXT
├── status_id     VARCHAR(50) FK → statuses NOT NULL
├── priority      INTEGER DEFAULT 0
├── assignee_id   VARCHAR(50) FK → users, NULLABLE
├── creator_id    VARCHAR(50) FK → users NOT NULL
├── project_id    VARCHAR(50) FK → projects ON DELETE SET NULL
├── milestone_id  VARCHAR(50) FK → project_milestones ON DELETE SET NULL
├── due_date      TIMESTAMPTZ
├── created_at    TIMESTAMPTZ DEFAULT now()
├── updated_at    TIMESTAMPTZ DEFAULT now()

UNIQUE(workspace_id, number)
```

Issue numbers are sequential per workspace. The display identifier combines the team key
with the issue number: `{team.key}-{issue.number}` (e.g. `ENG-42`).

### Constraints

- If `milestone_id` is set, the milestone's `project_id` must match the issue's
  `project_id`. Enforced at the application layer.
- `status_id` must reference a status that is either global (`team_id IS NULL`) or belongs
  to the issue's team (`team_id = issue.team_id`). Enforced at the application layer.

---

## Future Considerations

These are not currently modeled but are anticipated extensions:

- **Issue templates** — per-team default labels, assignees, and description templates
- **Cycles / Sprints** — time-boxed iteration planning
- **Saved views** — persisted filters and sort orders per team or project
- **Estimates** — story points or t-shirt sizing on issues
- **Sub-issues** — parent/child issue relationships
- **SLAs** — due date policies per team or priority level
