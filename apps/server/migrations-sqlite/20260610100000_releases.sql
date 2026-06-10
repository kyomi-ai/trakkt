-- Release tracking tables — SQLite variant for personal/self-hosted mode.

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS releases (
    release_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    team_key TEXT NOT NULL,
    tag_name TEXT NOT NULL,
    previous_tag TEXT,
    title TEXT,
    notes TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (workspace_id, tag_name)
);

CREATE TABLE IF NOT EXISTS release_issues (
    release_id TEXT NOT NULL REFERENCES releases(release_id) ON DELETE CASCADE,
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    PRIMARY KEY (release_id, issue_id)
);

-- ─────────────────────────────────────────────────────────────────────────────
-- ALTER issues — add released_at
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE issues ADD COLUMN released_at TEXT;

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_releases_workspace ON releases (workspace_id);
