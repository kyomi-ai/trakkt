-- First-class statuses: replace issues.status VARCHAR(20) with a statuses
-- table and issues.status_id FK.
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Create statuses table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS public.statuses (
    status_id character varying(100) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    team_id character varying(50),
    name character varying(100) NOT NULL,
    category character varying(20) NOT NULL,
    position integer DEFAULT 0,
    color character varying(20),
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

DO $$ BEGIN
    ALTER TABLE ONLY public.statuses ADD CONSTRAINT statuses_pkey PRIMARY KEY (status_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS idx_statuses_workspace_team_name
    ON public.statuses (workspace_id, COALESCE(team_id, ''), name);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Foreign keys on statuses
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.statuses ADD CONSTRAINT statuses_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.statuses ADD CONSTRAINT statuses_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.teams(team_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Add status_id column to issues (nullable for migration)
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.issues ADD COLUMN IF NOT EXISTS status_id character varying(100);

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Seed global default statuses for every existing workspace
-- ─────────────────────────────────────────────────────────────────────────────

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::backlog',
    w.workspace_id,
    NULL,
    'Backlog',
    'backlog',
    0,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::triage',
    w.workspace_id,
    NULL,
    'Triage',
    'backlog',
    1,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::todo',
    w.workspace_id,
    NULL,
    'Todo',
    'unstarted',
    0,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::in_progress',
    w.workspace_id,
    NULL,
    'In Progress',
    'started',
    0,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::done',
    w.workspace_id,
    NULL,
    'Done',
    'completed',
    0,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;

INSERT INTO public.statuses (status_id, workspace_id, team_id, name, category, position, created_at)
SELECT
    w.workspace_id || '::cancelled',
    w.workspace_id,
    NULL,
    'Cancelled',
    'cancelled',
    0,
    now()
FROM public.workspaces w
ON CONFLICT DO NOTHING;

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. Migrate existing issues: map old status string to status_id
-- ─────────────────────────────────────────────────────────────────────────────

UPDATE public.issues SET status_id = workspace_id || '::' || status WHERE status_id IS NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- 6. Make status_id NOT NULL
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.issues ALTER COLUMN status_id SET NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- 7. Add FK from issues.status_id to statuses
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_status_id_fkey FOREIGN KEY (status_id) REFERENCES public.statuses(status_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- 8. Drop old status column
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.issues DROP COLUMN IF EXISTS status;

-- ─────────────────────────────────────────────────────────────────────────────
-- 9. Update indexes
-- ─────────────────────────────────────────────────────────────────────────────

DROP INDEX IF EXISTS idx_issues_workspace_status;
CREATE INDEX IF NOT EXISTS idx_issues_workspace_status_id ON public.issues USING btree (workspace_id, status_id);

CREATE INDEX IF NOT EXISTS idx_statuses_workspace ON public.statuses USING btree (workspace_id);
