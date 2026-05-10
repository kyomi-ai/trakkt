-- Saved views: named filter + display presets for issues.
CREATE TABLE IF NOT EXISTS views (
    view_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    created_by TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    icon TEXT,
    filters TEXT NOT NULL DEFAULT '{}',
    display_options TEXT NOT NULL DEFAULT '{}',
    sort_order REAL DEFAULT 0,
    is_shared INTEGER DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_views_workspace ON views (workspace_id);
CREATE INDEX IF NOT EXISTS idx_views_created_by ON views (created_by);
