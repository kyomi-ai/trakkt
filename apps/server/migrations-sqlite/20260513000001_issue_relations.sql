-- Issue relations: typed, directional relationships between issues
CREATE TABLE IF NOT EXISTS issue_relations (
    relation_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    source_issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    target_issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    relation_type TEXT NOT NULL,
    created_by TEXT REFERENCES users(user_id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(source_issue_id, target_issue_id, relation_type)
);

CREATE INDEX IF NOT EXISTS idx_issue_relations_source ON issue_relations(source_issue_id);
CREATE INDEX IF NOT EXISTS idx_issue_relations_target ON issue_relations(target_issue_id);
CREATE INDEX IF NOT EXISTS idx_issue_relations_workspace ON issue_relations(workspace_id);
