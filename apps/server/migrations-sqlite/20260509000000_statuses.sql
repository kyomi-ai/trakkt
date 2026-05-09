-- First-class statuses — SQLite variant.
-- Replace issues.status TEXT with a statuses table and issues.status_id FK.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Create statuses table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS statuses (
    status_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    team_id TEXT REFERENCES teams(team_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    position INTEGER DEFAULT 0,
    color TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_statuses_workspace_team_name
    ON statuses (workspace_id, COALESCE(team_id, ''), name);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Seed global default statuses for every existing workspace
-- ─────────────────────────────────────────────────────────────────────────────

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::backlog',
    w.workspace_id,
    NULL,
    'Backlog',
    'backlog',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces w;

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::triage',
    w.workspace_id,
    NULL,
    'Triage',
    'backlog',
    1,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces w;

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::todo',
    w.workspace_id,
    NULL,
    'Todo',
    'unstarted',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces w;

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::in_progress',
    w.workspace_id,
    NULL,
    'In Progress',
    'started',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces w;

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::done',
    w.workspace_id,
    NULL,
    'Done',
    'completed',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces w;

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::cancelled',
    w.workspace_id,
    NULL,
    'Cancelled',
    'cancelled',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces w;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Recreate issues table with status_id instead of status
-- ─────────────────────────────────────────────────────────────────────────────
-- SQLite cannot ALTER COLUMN or DROP COLUMN reliably, so we use the
-- rename-copy-drop pattern.

CREATE TABLE IF NOT EXISTS issues_new (
    issue_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    team_id TEXT NOT NULL REFERENCES teams(team_id),
    number INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    status_id TEXT NOT NULL REFERENCES statuses(status_id),
    priority INTEGER NOT NULL DEFAULT 0,
    assignee_id TEXT REFERENCES users(user_id),
    creator_id TEXT NOT NULL REFERENCES users(user_id),
    due_date TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(workspace_id, number)
);

INSERT OR IGNORE INTO issues_new (
    issue_id, workspace_id, team_id, number, title, description,
    status_id, priority, assignee_id, creator_id,
    due_date, created_at, updated_at
)
SELECT
    issue_id, workspace_id, team_id, number, title, description,
    workspace_id || '::' || status, priority, assignee_id, creator_id,
    due_date, created_at, updated_at
FROM issues;

DROP TABLE issues;

ALTER TABLE issues_new RENAME TO issues;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Recreate indexes on the new issues table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_issues_workspace_status_id ON issues (workspace_id, status_id);
CREATE INDEX IF NOT EXISTS idx_issues_workspace_team_number ON issues (workspace_id, team_id, number);
CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues (assignee_id);
CREATE INDEX IF NOT EXISTS idx_issues_creator ON issues (creator_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. Index on statuses for workspace lookup
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_statuses_workspace ON statuses (workspace_id);
