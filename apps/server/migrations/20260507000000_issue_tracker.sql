-- Issue tracker domain tables.
-- Adds teams, issues, labels, comments, notifications, and watchers
-- on top of the baseline auth schema.
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS public.teams (
    team_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    key character varying(10) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.issues (
    issue_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    team_id character varying(50) NOT NULL,
    number integer NOT NULL,
    title character varying(500) NOT NULL,
    description text,
    status character varying(20) NOT NULL DEFAULT 'backlog',
    priority integer NOT NULL DEFAULT 0,
    assignee_id character varying(50),
    creator_id character varying(50) NOT NULL,
    due_date timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.labels (
    label_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    name character varying(100) NOT NULL,
    color character varying(20) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.issue_labels (
    issue_id character varying(50) NOT NULL,
    label_id character varying(50) NOT NULL
);

CREATE TABLE IF NOT EXISTS public.comments (
    comment_id character varying(50) NOT NULL,
    issue_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    body text NOT NULL,
    parent_id character varying(50),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.notifications (
    notification_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    issue_id character varying(50) NOT NULL,
    type character varying(30) NOT NULL,
    read boolean NOT NULL DEFAULT false,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.issue_watchers (
    issue_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL
);

-- ─────────────────────────────────────────────────────────────────────────────
-- ALTER existing api_tokens table — add issue-tracker columns
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.api_tokens ADD COLUMN IF NOT EXISTS workspace_id character varying(50);
ALTER TABLE public.api_tokens ADD COLUMN IF NOT EXISTS token_prefix character varying(20);
ALTER TABLE public.api_tokens ADD COLUMN IF NOT EXISTS scopes text DEFAULT '[]';

-- ─────────────────────────────────────────────────────────────────────────────
-- Primary keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.teams ADD CONSTRAINT teams_pkey PRIMARY KEY (team_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_pkey PRIMARY KEY (issue_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.labels ADD CONSTRAINT labels_pkey PRIMARY KEY (label_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issue_labels ADD CONSTRAINT issue_labels_pkey PRIMARY KEY (issue_id, label_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.comments ADD CONSTRAINT comments_pkey PRIMARY KEY (comment_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications ADD CONSTRAINT notifications_pkey PRIMARY KEY (notification_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issue_watchers ADD CONSTRAINT issue_watchers_pkey PRIMARY KEY (issue_id, user_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- Unique constraints
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.teams ADD CONSTRAINT teams_workspace_key_unique UNIQUE (workspace_id, key);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_workspace_number_unique UNIQUE (workspace_id, number);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.labels ADD CONSTRAINT labels_workspace_name_unique UNIQUE (workspace_id, name);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

-- teams
CREATE INDEX IF NOT EXISTS idx_teams_workspace ON public.teams USING btree (workspace_id);

-- issues
CREATE INDEX IF NOT EXISTS idx_issues_workspace_status ON public.issues USING btree (workspace_id, status);
CREATE INDEX IF NOT EXISTS idx_issues_workspace_team_number ON public.issues USING btree (workspace_id, team_id, number);
CREATE INDEX IF NOT EXISTS idx_issues_assignee ON public.issues USING btree (assignee_id);
CREATE INDEX IF NOT EXISTS idx_issues_creator ON public.issues USING btree (creator_id);

-- comments
CREATE INDEX IF NOT EXISTS idx_comments_issue ON public.comments USING btree (issue_id, created_at);
CREATE INDEX IF NOT EXISTS idx_comments_parent ON public.comments USING btree (parent_id);

-- notifications
CREATE INDEX IF NOT EXISTS idx_notifications_user_unread ON public.notifications USING btree (user_id, read, created_at);

-- labels
CREATE INDEX IF NOT EXISTS idx_labels_workspace ON public.labels USING btree (workspace_id);

-- issue_watchers
CREATE INDEX IF NOT EXISTS idx_issue_watchers_user ON public.issue_watchers USING btree (user_id);

-- api_tokens (new column)
CREATE INDEX IF NOT EXISTS idx_api_tokens_workspace ON public.api_tokens USING btree (workspace_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Foreign keys
-- ─────────────────────────────────────────────────────────────────────────────

-- teams
DO $$ BEGIN
    ALTER TABLE ONLY public.teams ADD CONSTRAINT teams_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- issues
DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.teams(team_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_assignee_id_fkey FOREIGN KEY (assignee_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issues ADD CONSTRAINT issues_creator_id_fkey FOREIGN KEY (creator_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- labels
DO $$ BEGIN
    ALTER TABLE ONLY public.labels ADD CONSTRAINT labels_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- issue_labels
DO $$ BEGIN
    ALTER TABLE ONLY public.issue_labels ADD CONSTRAINT issue_labels_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issue_labels ADD CONSTRAINT issue_labels_label_id_fkey FOREIGN KEY (label_id) REFERENCES public.labels(label_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- comments
DO $$ BEGIN
    ALTER TABLE ONLY public.comments ADD CONSTRAINT comments_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.comments ADD CONSTRAINT comments_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.comments ADD CONSTRAINT comments_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.comments(comment_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- notifications
DO $$ BEGIN
    ALTER TABLE ONLY public.notifications ADD CONSTRAINT notifications_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.notifications ADD CONSTRAINT notifications_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- issue_watchers
DO $$ BEGIN
    ALTER TABLE ONLY public.issue_watchers ADD CONSTRAINT issue_watchers_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id) ON DELETE CASCADE;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.issue_watchers ADD CONSTRAINT issue_watchers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- api_tokens (new workspace_id FK)
DO $$ BEGIN
    ALTER TABLE ONLY public.api_tokens ADD CONSTRAINT api_tokens_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;
