-- Team members: explicit team membership for defaults and notification routing.
-- Adds description/icon columns to teams and creates the team_members join table.
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Add columns to teams
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.teams ADD COLUMN IF NOT EXISTS description TEXT;
ALTER TABLE public.teams ADD COLUMN IF NOT EXISTS icon character varying(50);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Create team_members table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS public.team_members (
    team_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    role character varying(20) DEFAULT 'member',
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

DO $$ BEGIN
    ALTER TABLE ONLY public.team_members ADD CONSTRAINT team_members_pkey PRIMARY KEY (team_id, user_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Foreign keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.team_members ADD CONSTRAINT team_members_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.teams(team_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.team_members ADD CONSTRAINT team_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Indexes
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_team_members_user ON public.team_members USING btree (user_id);
CREATE INDEX IF NOT EXISTS idx_team_members_team ON public.team_members USING btree (team_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. Seed: add all workspace members to the default team of each workspace
-- ─────────────────────────────────────────────────────────────────────────────
-- For each workspace, find the first team (by created_at), then insert a
-- team_members row for every workspace_users member.

INSERT INTO public.team_members (team_id, user_id, role, created_at)
SELECT
    dt.team_id,
    wu.user_id,
    'member',
    now()
FROM public.workspace_users wu
JOIN (
    SELECT DISTINCT ON (workspace_id) team_id, workspace_id
    FROM public.teams
    ORDER BY workspace_id, created_at ASC
) dt ON dt.workspace_id = wu.workspace_id
ON CONFLICT DO NOTHING;
