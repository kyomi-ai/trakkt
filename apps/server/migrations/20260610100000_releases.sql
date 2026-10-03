-- Release tracking tables.
-- Adds releases, release_issues junction, and released_at to issues.
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS public.releases (
    release_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    team_key character varying(10) NOT NULL,
    tag_name character varying(100) NOT NULL,
    previous_tag character varying(100),
    title character varying(255),
    notes text,
    created_by character varying(50) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.release_issues (
    release_id character varying(50) NOT NULL,
    issue_id character varying(50) NOT NULL
);

-- ─────────────────────────────────────────────────────────────────────────────
-- ALTER issues — add released_at
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.issues ADD COLUMN IF NOT EXISTS released_at timestamp with time zone;

-- ─────────────────────────────────────────────────────────────────────────────
-- Primary keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.releases ADD CONSTRAINT releases_pkey PRIMARY KEY (release_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.release_issues ADD CONSTRAINT release_issues_pkey PRIMARY KEY (release_id, issue_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- Unique constraints
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.releases ADD CONSTRAINT releases_workspace_tag_unique UNIQUE (workspace_id, tag_name);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_releases_workspace ON public.releases USING btree (workspace_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Foreign keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.releases ADD CONSTRAINT releases_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.release_issues ADD CONSTRAINT release_issues_release_id_fkey FOREIGN KEY (release_id) REFERENCES public.releases(release_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.release_issues ADD CONSTRAINT release_issues_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;
