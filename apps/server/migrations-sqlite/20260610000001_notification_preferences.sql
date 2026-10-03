CREATE TABLE notification_preferences (
    preference_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    notify_status_changes INTEGER NOT NULL DEFAULT 1,
    notify_comments INTEGER NOT NULL DEFAULT 1,
    notify_assignments INTEGER NOT NULL DEFAULT 1,
    notify_priority_changes INTEGER NOT NULL DEFAULT 1,
    notify_own_agent_actions INTEGER NOT NULL DEFAULT 0,
    notify_own_api_actions INTEGER NOT NULL DEFAULT 0,
    delivery_channel TEXT NOT NULL DEFAULT 'in_app',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, workspace_id)
);
