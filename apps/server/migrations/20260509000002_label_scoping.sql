-- Label scoping: add optional team_id to labels for team-level label scoping.
-- Labels with team_id = NULL remain workspace-scoped (available to all teams).
-- Labels with a team_id are team-scoped (only available within that team).
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Add team_id column to labels
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.labels ADD COLUMN IF NOT EXISTS team_id character varying(50);

DO $$ BEGIN
    ALTER TABLE ONLY public.labels ADD CONSTRAINT labels_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.teams(team_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Replace unique constraint with partial indexes
-- ─────────────────────────────────────────────────────────────────────────────
-- Old constraint: (workspace_id, name) — no longer correct since team-scoped
-- labels can reuse names across teams.

ALTER TABLE public.labels DROP CONSTRAINT IF EXISTS labels_workspace_name_unique;

-- Workspace-scoped labels: unique name per workspace (team_id IS NULL).
CREATE UNIQUE INDEX IF NOT EXISTS idx_labels_workspace_unique
    ON public.labels (workspace_id, name) WHERE team_id IS NULL;

-- Team-scoped labels: unique name per team.
CREATE UNIQUE INDEX IF NOT EXISTS idx_labels_team_unique
    ON public.labels (team_id, name) WHERE team_id IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Index on team_id for filtered queries
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_labels_team ON public.labels USING btree (team_id);
