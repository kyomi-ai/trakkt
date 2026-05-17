-- Activity log for tracking all changes to issues.

CREATE TABLE IF NOT EXISTS issue_activities (
    activity_id TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    action_type TEXT NOT NULL,
    field TEXT,
    old_value TEXT,
    new_value TEXT,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_issue_activities_issue ON issue_activities(issue_id, created_at);
CREATE INDEX IF NOT EXISTS idx_issue_activities_workspace ON issue_activities(workspace_id);
