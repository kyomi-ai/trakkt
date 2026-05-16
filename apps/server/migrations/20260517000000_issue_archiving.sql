-- Server-side auto-archiving: add archived_at to issues, settings to teams.

ALTER TABLE public.issues ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_issues_archived_at ON public.issues (archived_at)
    WHERE archived_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_issues_archive_candidate
    ON public.issues (team_id, updated_at)
    WHERE archived_at IS NULL;

ALTER TABLE public.teams ADD COLUMN IF NOT EXISTS settings JSONB;
