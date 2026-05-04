-- Tane baseline migration — auth and workspace tables only.
-- Extracted from Kyomi production schema.
-- Idempotent: safe to run on both empty and populated databases.

-- ─────────────────────────────────────────────────────────────────────────────
-- Extensions
-- ─────────────────────────────────────────────────────────────────────────────

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

-- ─────────────────────────────────────────────────────────────────────────────
-- Tables
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS public.users (
    user_id character varying(50) NOT NULL,
    email character varying(255) NOT NULL,
    name character varying(255),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login timestamp with time zone,
    active boolean DEFAULT true NOT NULL,
    verified boolean DEFAULT false NOT NULL,
    terms_accepted_at timestamp with time zone,
    terms_accepted_version character varying(50),
    marketing_consent boolean DEFAULT false NOT NULL,
    oauth_data text,
    extra_metadata json,
    last_workspace_id character varying(50)
);

CREATE TABLE IF NOT EXISTS public.workspaces (
    workspace_id character varying(50) NOT NULL,
    name character varying(255),
    domain character varying(255),
    status character varying(50) DEFAULT 'active'::character varying NOT NULL,
    admin_email character varying(255),
    owner_user_id character varying(50) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    user_limit integer,
    settings json
);

CREATE TABLE IF NOT EXISTS public.workspace_users (
    id integer NOT NULL,
    workspace_id character varying(50) NOT NULL,
    user_id character varying(50) NOT NULL,
    role character varying(50) DEFAULT 'workspace_user'::character varying NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_active timestamp with time zone,
    extra_metadata json
);

CREATE SEQUENCE IF NOT EXISTS public.workspace_users_id_seq
    AS integer START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1;
ALTER SEQUENCE public.workspace_users_id_seq OWNED BY public.workspace_users.id;
ALTER TABLE ONLY public.workspace_users ALTER COLUMN id SET DEFAULT nextval('public.workspace_users_id_seq'::regclass);

CREATE TABLE IF NOT EXISTS public.refresh_tokens (
    token_id character varying(100) NOT NULL,
    user_id character varying(50) NOT NULL,
    token_hash character varying(255) NOT NULL,
    demo_token_value character varying(500),
    expires_at timestamp with time zone NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used timestamp with time zone,
    user_agent character varying(500),
    ip_address character varying(45),
    oauth_client_id character varying(255),
    country_code character varying(10),
    family_id character varying(100) NOT NULL,
    replaced_at timestamp with time zone
);

COMMENT ON COLUMN public.refresh_tokens.demo_token_value IS 'DEMO mode only: stores unhashed refresh token for e2e testing. NULL in production.';

CREATE TABLE IF NOT EXISTS public.user_auth_methods (
    id integer NOT NULL,
    user_id character varying(50) NOT NULL,
    auth_type character varying(50) NOT NULL,
    auth_data json NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used timestamp with time zone,
    active boolean DEFAULT true NOT NULL
);

CREATE SEQUENCE IF NOT EXISTS public.user_auth_methods_id_seq
    AS integer START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1;
ALTER SEQUENCE public.user_auth_methods_id_seq OWNED BY public.user_auth_methods.id;
ALTER TABLE ONLY public.user_auth_methods ALTER COLUMN id SET DEFAULT nextval('public.user_auth_methods_id_seq'::regclass);

CREATE TABLE IF NOT EXISTS public.verification_tokens (
    token_id character varying(100) NOT NULL,
    email character varying(255) NOT NULL,
    token_hash character varying(255) NOT NULL,
    token_type character varying(50) NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    used_at timestamp with time zone
);

CREATE TABLE IF NOT EXISTS public.oauth_clients (
    id uuid NOT NULL,
    client_id character varying(255) NOT NULL,
    client_secret_hash character varying(255),
    name character varying(255) NOT NULL,
    redirect_uris jsonb DEFAULT '[]'::jsonb NOT NULL,
    scopes jsonb DEFAULT '[]'::jsonb NOT NULL,
    client_type character varying(50) DEFAULT 'public'::character varying NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS public.oauth_states (
    state character varying(64) NOT NULL,
    user_id character varying(64) NOT NULL,
    action character varying(32) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

COMMENT ON TABLE public.oauth_states IS 'Stores OAuth flow state parameters for CSRF protection across multiple workers';

CREATE TABLE IF NOT EXISTS public.workspace_invitations (
    invitation_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    email character varying(255) NOT NULL,
    role character varying(20) DEFAULT 'workspace_user'::character varying NOT NULL,
    invited_by_user_id character varying(50) NOT NULL,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '7 days'::interval) NOT NULL,
    accepted_at timestamp with time zone,
    accepted_by_user_id character varying(50)
);

CREATE TABLE IF NOT EXISTS public.ownership_transfers (
    transfer_id character varying(50) NOT NULL,
    workspace_id character varying(50) NOT NULL,
    from_user_id character varying(50) NOT NULL,
    to_user_id character varying(50) NOT NULL,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '7 days'::interval) NOT NULL,
    completed_at timestamp with time zone
);

CREATE TABLE IF NOT EXISTS public.api_tokens (
    token_id character varying(100) NOT NULL,
    user_id character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    token_hash character varying(255) NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    last_used timestamp with time zone,
    revoked_at timestamp with time zone,
    created_by character varying(255),
    revoked_by character varying(255)
);

CREATE TABLE IF NOT EXISTS public.sync_log (
    sync_id BIGSERIAL PRIMARY KEY,
    entity_type VARCHAR(50) NOT NULL,
    entity_id VARCHAR(100) NOT NULL,
    workspace_id VARCHAR(100) NOT NULL,
    action VARCHAR(10) NOT NULL,
    data JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ─────────────────────────────────────────────────────────────────────────────
-- Primary keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.users ADD CONSTRAINT users_pkey PRIMARY KEY (user_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces ADD CONSTRAINT workspaces_pkey PRIMARY KEY (workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users ADD CONSTRAINT workspace_users_pkey PRIMARY KEY (id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.refresh_tokens ADD CONSTRAINT refresh_tokens_pkey PRIMARY KEY (token_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.user_auth_methods ADD CONSTRAINT user_auth_methods_pkey PRIMARY KEY (id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.verification_tokens ADD CONSTRAINT verification_tokens_pkey PRIMARY KEY (token_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.oauth_clients ADD CONSTRAINT oauth_clients_pkey PRIMARY KEY (id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.oauth_states ADD CONSTRAINT oauth_states_pkey PRIMARY KEY (state);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations ADD CONSTRAINT workspace_invitations_pkey PRIMARY KEY (invitation_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers ADD CONSTRAINT ownership_transfers_pkey PRIMARY KEY (transfer_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.api_tokens ADD CONSTRAINT api_tokens_pkey PRIMARY KEY (token_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

-- ─────────────────────────────────────────────────────────────────────────────
-- Unique constraints
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.users ADD CONSTRAINT users_email_key UNIQUE (email);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces ADD CONSTRAINT workspaces_domain_key UNIQUE (domain);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.oauth_clients ADD CONSTRAINT oauth_clients_client_id_key UNIQUE (client_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users ADD CONSTRAINT uq_workspace_user UNIQUE (workspace_id, user_id);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.user_auth_methods ADD CONSTRAINT uq_user_auth_type UNIQUE (user_id, auth_type);
EXCEPTION WHEN duplicate_object THEN NULL; WHEN invalid_table_definition THEN NULL;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS unique_pending_transfer ON public.ownership_transfers USING btree (workspace_id) WHERE ((status)::text = 'pending'::text);

-- ─────────────────────────────────────────────────────────────────────────────
-- Indexes
-- ─────────────────────────────────────────────────────────────────────────────

-- users
CREATE INDEX IF NOT EXISTS idx_users_email ON public.users USING btree (email);
CREATE INDEX IF NOT EXISTS idx_users_active ON public.users USING btree (active);
CREATE INDEX IF NOT EXISTS idx_users_created_at ON public.users USING btree (created_at);
CREATE INDEX IF NOT EXISTS idx_users_last_workspace_id ON public.users USING btree (last_workspace_id);
CREATE INDEX IF NOT EXISTS idx_users_terms_accepted_at ON public.users USING btree (terms_accepted_at);

-- workspaces
CREATE INDEX IF NOT EXISTS idx_workspaces_created_at ON public.workspaces USING btree (created_at);
CREATE INDEX IF NOT EXISTS idx_workspaces_domain ON public.workspaces USING btree (domain);
CREATE INDEX IF NOT EXISTS idx_workspaces_owner_user_id ON public.workspaces USING btree (owner_user_id);
CREATE INDEX IF NOT EXISTS idx_workspaces_status ON public.workspaces USING btree (status);

-- workspace_users
CREATE INDEX IF NOT EXISTS idx_workspace_users_role ON public.workspace_users USING btree (role);
CREATE INDEX IF NOT EXISTS idx_workspace_users_user ON public.workspace_users USING btree (user_id);
CREATE INDEX IF NOT EXISTS idx_workspace_users_workspace ON public.workspace_users USING btree (workspace_id);

-- refresh_tokens
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_active ON public.refresh_tokens USING btree (is_active);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires ON public.refresh_tokens USING btree (expires_at);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON public.refresh_tokens USING btree (token_hash);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON public.refresh_tokens USING btree (user_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family ON public.refresh_tokens USING btree (family_id);

-- user_auth_methods
CREATE INDEX IF NOT EXISTS idx_auth_methods_type ON public.user_auth_methods USING btree (auth_type);
CREATE INDEX IF NOT EXISTS idx_auth_methods_user ON public.user_auth_methods USING btree (user_id);

-- verification_tokens
CREATE INDEX IF NOT EXISTS idx_verification_tokens_email ON public.verification_tokens USING btree (email);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_expires ON public.verification_tokens USING btree (expires_at);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_hash ON public.verification_tokens USING btree (token_hash);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_type ON public.verification_tokens USING btree (token_type);

-- oauth_clients
CREATE INDEX IF NOT EXISTS idx_oauth_clients_client_id ON public.oauth_clients USING btree (client_id);

-- oauth_states
CREATE INDEX IF NOT EXISTS idx_oauth_states_created_at ON public.oauth_states USING btree (created_at);

-- workspace_invitations
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_email ON public.workspace_invitations USING btree (email);
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_status ON public.workspace_invitations USING btree (status);
CREATE INDEX IF NOT EXISTS idx_workspace_invitations_workspace ON public.workspace_invitations USING btree (workspace_id);

-- ownership_transfers
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_expires ON public.ownership_transfers USING btree (expires_at);
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_status ON public.ownership_transfers USING btree (status);
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_to_user ON public.ownership_transfers USING btree (to_user_id);
CREATE INDEX IF NOT EXISTS idx_ownership_transfers_workspace ON public.ownership_transfers USING btree (workspace_id);

-- api_tokens
CREATE INDEX IF NOT EXISTS idx_api_tokens_active ON public.api_tokens USING btree (active);
CREATE INDEX IF NOT EXISTS idx_api_tokens_expires ON public.api_tokens USING btree (expires_at);
CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON public.api_tokens USING btree (token_hash);
CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON public.api_tokens USING btree (user_id);

-- sync_log
CREATE INDEX IF NOT EXISTS idx_sync_log_workspace_id ON public.sync_log (workspace_id, sync_id);
CREATE INDEX IF NOT EXISTS idx_sync_log_created_at ON public.sync_log (created_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- Foreign keys
-- ─────────────────────────────────────────────────────────────────────────────

DO $$ BEGIN
    ALTER TABLE ONLY public.workspaces ADD CONSTRAINT workspaces_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users ADD CONSTRAINT workspace_users_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_users ADD CONSTRAINT workspace_users_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.refresh_tokens ADD CONSTRAINT refresh_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.user_auth_methods ADD CONSTRAINT user_auth_methods_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.api_tokens ADD CONSTRAINT api_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers ADD CONSTRAINT ownership_transfers_from_user_id_fkey FOREIGN KEY (from_user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers ADD CONSTRAINT ownership_transfers_to_user_id_fkey FOREIGN KEY (to_user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.ownership_transfers ADD CONSTRAINT ownership_transfers_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations ADD CONSTRAINT workspace_invitations_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(workspace_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations ADD CONSTRAINT workspace_invitations_invited_by_user_id_fkey FOREIGN KEY (invited_by_user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE ONLY public.workspace_invitations ADD CONSTRAINT workspace_invitations_accepted_by_user_id_fkey FOREIGN KEY (accepted_by_user_id) REFERENCES public.users(user_id);
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;
