-- Add started_at timestamp to issues for tracking when issues enter a started status.
ALTER TABLE issues ADD COLUMN started_at TEXT;

-- Backfill: set started_at = created_at for issues already in started/completed/cancelled status
UPDATE issues SET started_at = created_at
WHERE status_id IN (SELECT status_id FROM statuses WHERE category IN ('started', 'completed', 'cancelled'));
