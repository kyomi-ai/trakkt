-- Issue stars: per-user starred issues (ephemeral user preference, like watchers).
CREATE TABLE IF NOT EXISTS public.issue_stars (
    issue_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- Primary key
DO $$ BEGIN
    ALTER TABLE ONLY public.issue_stars ADD CONSTRAINT issue_stars_pkey PRIMARY KEY (issue_id, user_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- Foreign keys
DO $$ BEGIN
    ALTER TABLE ONLY public.issue_stars ADD CONSTRAINT issue_stars_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issue_stars ADD CONSTRAINT issue_stars_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- Index for user lookups
CREATE INDEX IF NOT EXISTS idx_issue_stars_user ON public.issue_stars USING btree (user_id);
