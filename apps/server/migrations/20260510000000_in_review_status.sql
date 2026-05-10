-- Add "In Review" status (started category, position 1) for all existing workspaces.
-- Sits between "In Progress" (started, position 0) and "Done" (completed).

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::in_review',
    w.workspace_id,
    NULL,
    'In Review',
    'started',
    1,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;
