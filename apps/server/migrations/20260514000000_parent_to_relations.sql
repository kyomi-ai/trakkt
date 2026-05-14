-- Migrate parent-child relationships from parent_issue_id column to issue_relations table.
-- Semantics: source_issue_id = parent, target_issue_id = child, relation_type = 'parent'.

INSERT INTO issue_relations (relation_id, workspace_id, source_issue_id, target_issue_id, relation_type, created_at)
SELECT gen_random_uuid()::text, i.workspace_id, i.parent_issue_id, i.issue_id, 'parent', now()
FROM issues i
WHERE i.parent_issue_id IS NOT NULL;

DROP INDEX IF EXISTS idx_issues_parent;
ALTER TABLE issues DROP COLUMN parent_issue_id;
