-- Add completed_at timestamp to issues for tracking when issues were completed/cancelled.
ALTER TABLE issues ADD COLUMN completed_at TEXT;

-- Backfill: set completed_at = updated_at for issues already in completed/cancelled status
UPDATE issues SET completed_at = updated_at
WHERE status_id IN (SELECT status_id FROM statuses WHERE category IN ('completed', 'cancelled'));
