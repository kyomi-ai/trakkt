-- Migrate parent-child relationships from parent_issue_id column to issue_relations table.
-- Semantics: source_issue_id = parent, target_issue_id = child, relation_type = 'parent'.

-- SQLite UUID generation: matches the pattern used in other SQLite migrations.
INSERT INTO issue_relations (relation_id, workspace_id, source_issue_id, target_issue_id, relation_type, created_at)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab', abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6))),
    i.workspace_id,
    i.parent_issue_id,
    i.issue_id,
    'parent',
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM issues i
WHERE i.parent_issue_id IS NOT NULL;

-- SQLite does not support ALTER TABLE DROP COLUMN before 3.35.0.
-- Rebuild the issues table without parent_issue_id.
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
    milestone_id TEXT,
    sort_order REAL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(team_id, number)
);

INSERT INTO issues_new (
    issue_id, workspace_id, team_id, number, title, description,
    status_id, priority, assignee_id, creator_id,
    due_date, project_id, milestone_id, sort_order,
    created_at, updated_at
)
SELECT
    issue_id, workspace_id, team_id, number, title, description,
    status_id, priority, assignee_id, creator_id,
    due_date, project_id, milestone_id, sort_order,
    created_at, updated_at
FROM issues;

DROP TABLE issues;
ALTER TABLE issues_new RENAME TO issues;

-- Re-create indexes that existed on the original table (minus idx_issues_parent which is intentionally dropped).
CREATE INDEX IF NOT EXISTS idx_issues_workspace_status_id ON issues (workspace_id, status_id);
CREATE INDEX IF NOT EXISTS idx_issues_workspace_team_number ON issues (workspace_id, team_id, number);
CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues (assignee_id);
CREATE INDEX IF NOT EXISTS idx_issues_creator ON issues (creator_id);
CREATE INDEX IF NOT EXISTS idx_issues_project ON issues (project_id);
CREATE INDEX IF NOT EXISTS idx_issues_milestone ON issues (milestone_id);
