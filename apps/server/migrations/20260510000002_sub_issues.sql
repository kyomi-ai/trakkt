-- Sub-issues: parent-child issue hierarchy (Phase 1)
ALTER TABLE issues ADD COLUMN parent_issue_id TEXT REFERENCES issues(issue_id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_issues_parent ON issues (parent_issue_id);
