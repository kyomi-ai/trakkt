-- Issue tracker domain tables — SQLite variant for personal/self-hosted mode.

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS teams (
    team_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    name TEXT NOT NULL,
    key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(workspace_id, key)
);

CREATE TABLE IF NOT EXISTS issues (
    issue_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    team_id TEXT NOT NULL REFERENCES teams(team_id),
    number INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'backlog',
    priority INTEGER NOT NULL DEFAULT 0,
    assignee_id TEXT REFERENCES users(user_id),
    creator_id TEXT NOT NULL REFERENCES users(user_id),
    due_date TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(workspace_id, number)
);

CREATE TABLE IF NOT EXISTS labels (
    label_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(workspace_id, name)
);

CREATE TABLE IF NOT EXISTS issue_labels (
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    label_id TEXT NOT NULL REFERENCES labels(label_id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, label_id)
);

CREATE TABLE IF NOT EXISTS comments (
    comment_id TEXT NOT NULL PRIMARY KEY,
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    body TEXT NOT NULL,
    parent_id TEXT REFERENCES comments(comment_id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS notifications (
    notification_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id TEXT NOT NULL REFERENCES users(user_id),
    issue_id TEXT NOT NULL REFERENCES issues(issue_id),
    type TEXT NOT NULL,
    read INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS issue_watchers (
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    PRIMARY KEY (issue_id, user_id)
);

-- ─────────────────────────────────────────────────────────────────────────────
-- ALTER existing api_tokens table — add issue-tracker columns
-- ─────────────────────────────────────────────────────────────────────────────
-- Note: SQLite lacks ADD COLUMN IF NOT EXISTS before 3.35.0. These are safe
-- because sqlx runs each migration exactly once (tracked in _sqlx_migrations).
-- The CREATE TABLE IF NOT EXISTS clauses above are defensive convention.

ALTER TABLE api_tokens ADD COLUMN workspace_id TEXT;
ALTER TABLE api_tokens ADD COLUMN token_prefix TEXT;
ALTER TABLE api_tokens ADD COLUMN scopes TEXT DEFAULT '[]';

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

-- teams
CREATE INDEX IF NOT EXISTS idx_teams_workspace ON teams (workspace_id);

-- issues
CREATE INDEX IF NOT EXISTS idx_issues_workspace_status ON issues (workspace_id, status);
CREATE INDEX IF NOT EXISTS idx_issues_workspace_team_number ON issues (workspace_id, team_id, number);
CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues (assignee_id);
CREATE INDEX IF NOT EXISTS idx_issues_creator ON issues (creator_id);

-- comments
CREATE INDEX IF NOT EXISTS idx_comments_issue ON comments (issue_id, created_at);
CREATE INDEX IF NOT EXISTS idx_comments_parent ON comments (parent_id);

-- notifications
CREATE INDEX IF NOT EXISTS idx_notifications_user_unread ON notifications (user_id, read, created_at);

-- labels
CREATE INDEX IF NOT EXISTS idx_labels_workspace ON labels (workspace_id);

-- issue_watchers
CREATE INDEX IF NOT EXISTS idx_issue_watchers_user ON issue_watchers (user_id);

-- api_tokens (new column)
CREATE INDEX IF NOT EXISTS idx_api_tokens_workspace ON api_tokens (workspace_id);
