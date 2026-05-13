-- Favorites: per-user pinned items (teams, projects, views) for quick sidebar access.

CREATE TABLE IF NOT EXISTS favorites (
    favorite_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    target_type TEXT NOT NULL,
    target_id TEXT NOT NULL,
    sort_order REAL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_favorites_unique ON favorites (user_id, workspace_id, target_type, target_id);
CREATE INDEX IF NOT EXISTS idx_favorites_user_ws ON favorites (user_id, workspace_id);
