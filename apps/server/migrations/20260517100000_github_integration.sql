-- GitHub integration tables

CREATE TABLE github_apps (
    github_app_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    app_id BIGINT NOT NULL,
    app_name TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret_encrypted TEXT NOT NULL,
    private_key_encrypted TEXT NOT NULL,
    webhook_secret_encrypted TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE github_installations (
    installation_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    github_app_id TEXT NOT NULL REFERENCES github_apps(github_app_id) ON DELETE CASCADE,
    github_installation_id BIGINT NOT NULL UNIQUE,
    account_login TEXT NOT NULL,
    account_type TEXT NOT NULL,
    target_repos JSONB,
    access_token_encrypted TEXT,
    token_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    suspended_at TIMESTAMPTZ
);

CREATE INDEX idx_github_installations_workspace ON github_installations(workspace_id);
CREATE INDEX idx_github_installations_github_id ON github_installations(github_installation_id);

CREATE TABLE github_links (
    link_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL REFERENCES github_installations(installation_id) ON DELETE CASCADE,
    link_type TEXT NOT NULL,
    github_id BIGINT,
    github_node_id TEXT,
    repo_full_name TEXT NOT NULL,
    ref_identifier TEXT NOT NULL,
    title TEXT,
    state TEXT,
    url TEXT NOT NULL,
    author_login TEXT,
    close_intent BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_github_links_issue ON github_links(issue_id);
CREATE INDEX idx_github_links_repo ON github_links(repo_full_name);
CREATE UNIQUE INDEX idx_github_links_dedup ON github_links(workspace_id, link_type, repo_full_name, ref_identifier);

CREATE TABLE github_events (
    event_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    github_delivery_id TEXT NOT NULL UNIQUE,
    installation_id TEXT REFERENCES github_installations(installation_id) ON DELETE SET NULL,
    event_type TEXT NOT NULL,
    action TEXT,
    payload_summary JSONB,
    processed_at TIMESTAMPTZ,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_github_events_delivery ON github_events(github_delivery_id);

CREATE TABLE github_transition_rules (
    rule_id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    trigger_event TEXT NOT NULL,
    close_intent_required BOOLEAN NOT NULL DEFAULT false,
    target_status_category TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(workspace_id, trigger_event, close_intent_required)
);
