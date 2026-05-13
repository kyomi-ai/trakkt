-- Issue relations: typed, directional relationships between issues
CREATE TABLE issue_relations (
    relation_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    source_issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    target_issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    relation_type TEXT NOT NULL,
    created_by TEXT REFERENCES users(user_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(source_issue_id, target_issue_id, relation_type)
);

CREATE INDEX idx_issue_relations_source ON issue_relations(source_issue_id);
CREATE INDEX idx_issue_relations_target ON issue_relations(target_issue_id);
CREATE INDEX idx_issue_relations_workspace ON issue_relations(workspace_id);
