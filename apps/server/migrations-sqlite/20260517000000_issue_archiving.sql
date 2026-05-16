-- Server-side auto-archiving: add archived_at to issues, settings to teams.

ALTER TABLE issues ADD COLUMN archived_at TEXT;
CREATE INDEX IF NOT EXISTS idx_issues_archived_at ON issues (archived_at);
CREATE INDEX IF NOT EXISTS idx_issues_archive_candidate ON issues (team_id, updated_at);
ALTER TABLE teams ADD COLUMN settings TEXT;
