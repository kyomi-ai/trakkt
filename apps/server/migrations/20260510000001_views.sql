-- Saved views: named filter + display presets for issues.
CREATE TABLE IF NOT EXISTS views (
    view_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    created_by TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    icon TEXT,
    filters JSONB NOT NULL DEFAULT '{}',
    display_options JSONB NOT NULL DEFAULT '{}',
    sort_order DOUBLE PRECISION DEFAULT 0,
    is_shared BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_views_workspace ON views (workspace_id);
CREATE INDEX IF NOT EXISTS idx_views_created_by ON views (created_by);
