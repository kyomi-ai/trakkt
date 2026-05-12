-- Team-scoped issue numbering: renumber existing issues per-team and change
-- the unique constraint from (workspace_id, number) to (team_id, number).

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Drop old workspace-scoped constraint (must happen before renumbering)
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE issues DROP CONSTRAINT issues_workspace_number_unique;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Renumber existing issues per-team based on creation order
-- ─────────────────────────────────────────────────────────────────────────────

WITH numbered AS (
    SELECT issue_id, ROW_NUMBER() OVER (PARTITION BY team_id ORDER BY created_at) AS new_number
    FROM issues
)
UPDATE issues SET number = numbered.new_number FROM numbered WHERE issues.issue_id = numbered.issue_id;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Add team-scoped constraint
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE issues ADD CONSTRAINT issues_team_number_unique UNIQUE (team_id, number);
