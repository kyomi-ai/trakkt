-- Add team scoping and position ordering to views.
--
-- team_id is nullable: NULL means the view is workspace-scoped (visible
-- everywhere), non-NULL scopes the view to a specific team's sidebar.
-- position controls tab ordering within a team.

ALTER TABLE views ADD COLUMN team_id TEXT REFERENCES teams(team_id) ON DELETE CASCADE;
ALTER TABLE views ADD COLUMN position INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_views_team ON views (team_id);
