-- Project management tables.
-- Adds projects, project_members, project_milestones, project_updates,
-- and links issues to projects/milestones.
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS public.projects (
    project_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    icon character varying(50),
    color character varying(20),
    status character varying(20) DEFAULT 'planned',
    lead_id character varying(50),
    start_date date,
    target_date date,
    sort_order real DEFAULT 0,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.project_members (
    project_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    role character varying(20) DEFAULT 'member',
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.project_milestones (
    milestone_id character varying(50) NOT NULL,
    project_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    target_date date,
    sort_order integer DEFAULT 0,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.project_updates (
    update_id character varying(50) NOT NULL,
    project_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    health character varying(20) NOT NULL,
    body text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- ─────────────────────────────────────────────────────────────────────────────
-- ALTER issues — add project/milestone FKs
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.issues ADD COLUMN IF NOT EXISTS project_id character varying(50);
ALTER TABLE public.issues ADD COLUMN IF NOT EXISTS milestone_id character varying(50);

-- ─────────────────────────────────────────────────────────────────────────────
-- Primary keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.projects ADD CONSTRAINT projects_pkey PRIMARY KEY (project_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.project_members ADD CONSTRAINT project_members_pkey PRIMARY KEY (project_id, user_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.project_milestones ADD CONSTRAINT project_milestones_pkey PRIMARY KEY (milestone_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.project_updates ADD CONSTRAINT project_updates_pkey PRIMARY KEY (update_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

-- projects
CREATE INDEX IF NOT EXISTS idx_projects_workspace ON public.projects USING btree (workspace_id);

-- project_members
CREATE INDEX IF NOT EXISTS idx_project_members_user ON public.project_members USING btree (user_id);

-- project_milestones
CREATE INDEX IF NOT EXISTS idx_project_milestones_project ON public.project_milestones USING btree (project_id);

-- project_updates
CREATE INDEX IF NOT EXISTS idx_project_updates_project ON public.project_updates USING btree (project_id, created_at);

-- issues (new FK columns)
CREATE INDEX IF NOT EXISTS idx_issues_project ON public.issues USING btree (project_id);
CREATE INDEX IF NOT EXISTS idx_issues_milestone ON public.issues USING btree (milestone_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Foreign keys
-- ─────────────────────────────────────────────────────────────────────────────

-- projects
DO $$ BEGIN
    ALTER TABLE ONLY public.projects ADD CONSTRAINT projects_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.projects ADD CONSTRAINT projects_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.users(user_id) ON DELETE SET NULL;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- project_members
DO $$ BEGIN
    ALTER TABLE ONLY public.project_members ADD CONSTRAINT project_members_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(project_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.project_members ADD CONSTRAINT project_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- project_milestones
DO $$ BEGIN
    ALTER TABLE ONLY public.project_milestones ADD CONSTRAINT project_milestones_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(project_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- project_updates
DO $$ BEGIN
    ALTER TABLE ONLY public.project_updates ADD CONSTRAINT project_updates_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(project_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.project_updates ADD CONSTRAINT project_updates_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- issues (new FK columns)
DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(project_id) ON DELETE SET NULL;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_milestone_id_fkey FOREIGN KEY (milestone_id) REFERENCES public.project_milestones(milestone_id) ON DELETE SET NULL;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;
