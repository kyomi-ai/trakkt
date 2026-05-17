-- Fix dedup index to include issue_id so a single PR/commit can link to
-- multiple issues in the same workspace.
DROP INDEX IF EXISTS idx_github_links_dedup;
CREATE UNIQUE INDEX idx_github_links_dedup ON github_links(workspace_id, issue_id, link_type, repo_full_name, ref_identifier);
