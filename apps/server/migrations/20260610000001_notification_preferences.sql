CREATE TABLE notification_preferences (
    preference_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    notify_status_changes BOOLEAN NOT NULL DEFAULT TRUE,
    notify_comments BOOLEAN NOT NULL DEFAULT TRUE,
    notify_assignments BOOLEAN NOT NULL DEFAULT TRUE,
    notify_priority_changes BOOLEAN NOT NULL DEFAULT TRUE,
    notify_own_agent_actions BOOLEAN NOT NULL DEFAULT FALSE,
    notify_own_api_actions BOOLEAN NOT NULL DEFAULT FALSE,
    delivery_channel TEXT NOT NULL DEFAULT 'in_app',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, workspace_id)
);
