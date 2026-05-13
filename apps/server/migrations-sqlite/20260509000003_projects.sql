-- Project management tables — SQLite variant for personal/self-hosted mode.

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    icon TEXT,
    color TEXT,
    status TEXT DEFAULT 'planned',
    lead_id TEXT REFERENCES users(user_id) ON DELETE SET NULL,
    start_date TEXT,
    target_date TEXT,
    sort_order REAL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS project_members (
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    role TEXT DEFAULT 'member',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (project_id, user_id)
);

CREATE TABLE IF NOT EXISTS project_milestones (
    milestone_id TEXT NOT NULL PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    target_date TEXT,
    sort_order INTEGER DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS project_updates (
    update_id TEXT NOT NULL PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    health TEXT NOT NULL,
    body TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ─────────────────────────────────────────────────────────────────────────────
-- ALTER issues — add project/milestone columns
-- ─────────────────────────────────────────────────────────────────────────────
-- Note: SQLite lacks ADD COLUMN IF NOT EXISTS before 3.35.0. These are safe
-- because sqlx runs each migration exactly once (tracked in _sqlx_migrations).

ALTER TABLE issues ADD COLUMN project_id TEXT REFERENCES projects(project_id) ON DELETE SET NULL;
ALTER TABLE issues ADD COLUMN milestone_id TEXT REFERENCES project_milestones(milestone_id) ON DELETE SET NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

-- projects
CREATE INDEX IF NOT EXISTS idx_projects_workspace ON projects (workspace_id);

-- project_members
CREATE INDEX IF NOT EXISTS idx_project_members_user ON project_members (user_id);

-- project_milestones
CREATE INDEX IF NOT EXISTS idx_project_milestones_project ON project_milestones (project_id);

-- project_updates
CREATE INDEX IF NOT EXISTS idx_project_updates_project ON project_updates (project_id, created_at);

-- issues (new FK columns)
CREATE INDEX IF NOT EXISTS idx_issues_project ON issues (project_id);
CREATE INDEX IF NOT EXISTS idx_issues_milestone ON issues (milestone_id);
