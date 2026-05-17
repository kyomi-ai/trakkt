-- GitHub integration tables

CREATE TABLE IF NOT EXISTS github_apps (
    github_app_id TEXT NOT NULL PRIMARY KEY,
    app_id INTEGER NOT NULL,
    app_name TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret_encrypted TEXT NOT NULL,
    private_key_encrypted TEXT NOT NULL,
    webhook_secret_encrypted TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS github_installations (
    installation_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    github_app_id TEXT NOT NULL,
    github_installation_id INTEGER NOT NULL UNIQUE,
    account_login TEXT NOT NULL,
    account_type TEXT NOT NULL,
    target_repos TEXT,
    access_token_encrypted TEXT,
    token_expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    suspended_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_github_installations_workspace ON github_installations(workspace_id);
CREATE INDEX IF NOT EXISTS idx_github_installations_github_id ON github_installations(github_installation_id);

CREATE TABLE IF NOT EXISTS github_links (
    link_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    issue_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    link_type TEXT NOT NULL,
    github_id INTEGER,
    github_node_id TEXT,
    repo_full_name TEXT NOT NULL,
    ref_identifier TEXT NOT NULL,
    title TEXT,
    state TEXT,
    url TEXT NOT NULL,
    author_login TEXT,
    close_intent INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_github_links_issue ON github_links(issue_id);
CREATE INDEX IF NOT EXISTS idx_github_links_repo ON github_links(repo_full_name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_github_links_dedup ON github_links(workspace_id, link_type, repo_full_name, ref_identifier);

CREATE TABLE IF NOT EXISTS github_events (
    event_id TEXT NOT NULL PRIMARY KEY,
    github_delivery_id TEXT NOT NULL UNIQUE,
    installation_id TEXT,
    event_type TEXT NOT NULL,
    action TEXT,
    payload_summary TEXT,
    processed_at TEXT,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_github_events_delivery ON github_events(github_delivery_id);

CREATE TABLE IF NOT EXISTS github_transition_rules (
    rule_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    trigger_event TEXT NOT NULL,
    close_intent_required INTEGER NOT NULL DEFAULT 0,
    target_status_category TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(workspace_id, trigger_event, close_intent_required)
);
