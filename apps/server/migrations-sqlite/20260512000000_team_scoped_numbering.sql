-- Team-scoped issue numbering: renumber existing issues per-team and change
-- the unique constraint from (workspace_id, number) to (team_id, number).
-- SQLite cannot ALTER CONSTRAINT, so we use the rename-copy-drop pattern.

-- Disable foreign key checks during the rename-copy-drop migration to prevent
-- constraint violations when the old table is dropped and the new one renamed.
PRAGMA foreign_keys = OFF;

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Renumber existing issues per-team based on creation order
-- ─────────────────────────────────────────────────────────────────────────────
-- SQLite supports UPDATE with subquery via correlated subqueries.

UPDATE issues SET number = (
    SELECT COUNT(*)
    FROM issues AS i2
    WHERE i2.team_id = issues.team_id
      AND (i2.created_at < issues.created_at
           OR (i2.created_at = issues.created_at AND i2.issue_id <= issues.issue_id))
);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Recreate issues table with team-scoped unique constraint
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE issues_new (
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
    project_id TEXT REFERENCES projects(project_id) ON DELETE SET NULL,
    milestone_id TEXT REFERENCES project_milestones(milestone_id) ON DELETE SET NULL,
    parent_issue_id TEXT,
    sort_order REAL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(team_id, number)
);

INSERT INTO issues_new (
    issue_id, workspace_id, team_id, number, title, description,
    status_id, priority, assignee_id, creator_id,
    due_date, project_id, milestone_id, parent_issue_id, sort_order,
    created_at, updated_at
)
SELECT
    issue_id, workspace_id, team_id, number, title, description,
    status_id, priority, assignee_id, creator_id,
    due_date, project_id, milestone_id, parent_issue_id, sort_order,
    created_at, updated_at
FROM issues;

DROP TABLE issues;

ALTER TABLE issues_new RENAME TO issues;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Recreate indexes on the new issues table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_issues_workspace_status_id ON issues (workspace_id, status_id);
CREATE INDEX IF NOT EXISTS idx_issues_workspace_team_number ON issues (workspace_id, team_id, number);
CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues (assignee_id);
CREATE INDEX IF NOT EXISTS idx_issues_creator ON issues (creator_id);
CREATE INDEX IF NOT EXISTS idx_issues_project ON issues (project_id);
CREATE INDEX IF NOT EXISTS idx_issues_milestone ON issues (milestone_id);
CREATE INDEX IF NOT EXISTS idx_issues_parent ON issues (parent_issue_id);

-- Re-enable foreign key checks after the migration is complete.
PRAGMA foreign_keys = ON;
