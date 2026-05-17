-- Activity log for tracking all changes to issues.

CREATE TABLE issue_activities (
    activity_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    actor_id TEXT NOT NULL REFERENCES users(user_id),
    action_type TEXT NOT NULL,
    field TEXT,
    old_value TEXT,
    new_value TEXT,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_issue_activities_issue ON issue_activities(issue_id, created_at);
CREATE INDEX idx_issue_activities_workspace ON issue_activities(workspace_id);
