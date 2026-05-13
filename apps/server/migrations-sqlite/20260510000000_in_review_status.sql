-- Add "In Review" status (started category, position 1) for all existing workspaces.
-- Sits between "In Progress" (started, position 0) and "Done" (completed).

INSERT OR IGNORE INTO statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    workspace_id || '::in_review',
    workspace_id,
    NULL,
    'In Review',
    'started',
    1,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspaces;
