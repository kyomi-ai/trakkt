-- GitHub-authored activities (commits, PRs) may have no matching Trakkt user.
-- SQLite can't drop NOT NULL in place, so rebuild the table preserving all columns.
CREATE TABLE issue_activities_new (
    activity_id TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    actor_id TEXT,
    action_type TEXT NOT NULL,
    field TEXT,
    old_value TEXT,
    new_value TEXT,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    action_source TEXT NOT NULL DEFAULT 'user',
    action_source_label TEXT
);
INSERT INTO issue_activities_new (activity_id, issue_id, workspace_id, actor_id, action_type, field, old_value, new_value, metadata, created_at, action_source, action_source_label)
    SELECT activity_id, issue_id, workspace_id, actor_id, action_type, field, old_value, new_value, metadata, created_at, action_source, action_source_label FROM issue_activities;
DROP TABLE issue_activities;
ALTER TABLE issue_activities_new RENAME TO issue_activities;
CREATE INDEX IF NOT EXISTS idx_issue_activities_issue ON issue_activities(issue_id, created_at);
CREATE INDEX IF NOT EXISTS idx_issue_activities_workspace ON issue_activities(workspace_id);
