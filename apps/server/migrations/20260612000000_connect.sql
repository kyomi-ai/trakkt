-- Connect agent and session tables for the Trakkt Connect terminal relay.
-- Tracks connected agents and their terminal sessions for audit and status.
--
-- NOTE: These tables are reserved for future audit logging and session history.
-- The ConnectManager currently operates entirely in-memory. No Rust code reads
-- or writes these tables yet — they are created now so the schema is in place
-- when the audit/persistence layer is implemented.

-- ---------------------------------------------------------------------------
-- Tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS public.connect_agents (
    agent_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    hostname character varying(255),
    agent_version character varying(50),
    os character varying(50),
    last_seen_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.connect_sessions (
    session_id character varying(50) NOT NULL,
    agent_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    command text,
    working_dir text,
    status character varying(20) NOT NULL DEFAULT 'active',
    exit_code integer,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    ended_at timestamp with time zone
);

-- ---------------------------------------------------------------------------
-- Primary keys
-- ---------------------------------------------------------------------------

DO $$ BEGIN
    ALTER TABLE ONLY public.connect_agents ADD CONSTRAINT connect_agents_pkey PRIMARY KEY (agent_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.connect_sessions ADD CONSTRAINT connect_sessions_pkey PRIMARY KEY (session_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ---------------------------------------------------------------------------
-- Indexes
-- ---------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_connect_agents_workspace ON public.connect_agents USING btree (workspace_id);
CREATE INDEX IF NOT EXISTS idx_connect_sessions_workspace ON public.connect_sessions USING btree (workspace_id);
CREATE INDEX IF NOT EXISTS idx_connect_sessions_user ON public.connect_sessions USING btree (user_id);
