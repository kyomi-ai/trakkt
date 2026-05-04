-- Tane baseline migration — SQLite variant for personal/self-hosted mode.

CREATE TABLE IF NOT EXISTS users (
    user_id TEXT NOT NULL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    name TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_login TEXT,
    active INTEGER NOT NULL DEFAULT 1,
    verified INTEGER NOT NULL DEFAULT 0,
    terms_accepted_at TEXT,
    terms_accepted_version TEXT,
    marketing_consent INTEGER NOT NULL DEFAULT 0,
    oauth_data TEXT,
    extra_metadata TEXT,
    last_workspace_id TEXT
);

CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT NOT NULL PRIMARY KEY,
    name TEXT,
    domain TEXT UNIQUE,
    status TEXT NOT NULL DEFAULT 'active',
    admin_email TEXT,
    owner_user_id TEXT NOT NULL REFERENCES users(user_id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    user_limit INTEGER,
    settings TEXT
);

CREATE TABLE IF NOT EXISTS workspace_users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id TEXT NOT NULL REFERENCES users(user_id),
    role TEXT NOT NULL DEFAULT 'workspace_user',
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_active TEXT,
    extra_metadata TEXT,
    UNIQUE(workspace_id, user_id)
);

CREATE TABLE IF NOT EXISTS refresh_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    token_hash TEXT NOT NULL,
    demo_token_value TEXT,
    expires_at TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    revoked_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_used TEXT,
    user_agent TEXT,
    ip_address TEXT,
    oauth_client_id TEXT,
    country_code TEXT,
    family_id TEXT NOT NULL,
    replaced_at TEXT
);

CREATE TABLE IF NOT EXISTS user_auth_methods (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    auth_type TEXT NOT NULL,
    auth_data TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_used TEXT,
    active INTEGER NOT NULL DEFAULT 1,
    UNIQUE(user_id, auth_type)
);

CREATE TABLE IF NOT EXISTS verification_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    email TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    token_type TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    used_at TEXT
);

CREATE TABLE IF NOT EXISTS oauth_clients (
    id TEXT NOT NULL PRIMARY KEY,
    client_id TEXT NOT NULL UNIQUE,
    client_secret_hash TEXT,
    name TEXT NOT NULL,
    redirect_uris TEXT NOT NULL DEFAULT '[]',
    scopes TEXT NOT NULL DEFAULT '[]',
    client_type TEXT NOT NULL DEFAULT 'public',
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS oauth_states (
    state TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    action TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS workspace_invitations (
    invitation_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    email TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'workspace_user',
    invited_by_user_id TEXT NOT NULL REFERENCES users(user_id),
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at TEXT NOT NULL,
    accepted_at TEXT,
    accepted_by_user_id TEXT REFERENCES users(user_id)
);

CREATE TABLE IF NOT EXISTS ownership_transfers (
    transfer_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    from_user_id TEXT NOT NULL REFERENCES users(user_id),
    to_user_id TEXT NOT NULL REFERENCES users(user_id),
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE IF NOT EXISTS api_tokens (
    token_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at TEXT,
    last_used TEXT,
    revoked_at TEXT,
    created_by TEXT,
    revoked_by TEXT
);

CREATE TABLE IF NOT EXISTS sync_log (
    sync_id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    action TEXT NOT NULL,
    data TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_users_email ON users (email);
CREATE INDEX IF NOT EXISTS idx_users_active ON users (active);
CREATE INDEX IF NOT EXISTS idx_users_last_workspace_id ON users (last_workspace_id);
CREATE INDEX IF NOT EXISTS idx_workspaces_owner_user_id ON workspaces (owner_user_id);
CREATE INDEX IF NOT EXISTS idx_workspaces_domain ON workspaces (domain);
CREATE INDEX IF NOT EXISTS idx_workspace_users_user ON workspace_users (user_id);
CREATE INDEX IF NOT EXISTS idx_workspace_users_workspace ON workspace_users (workspace_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON refresh_tokens (token_hash);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_active ON refresh_tokens (is_active);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family ON refresh_tokens (family_id);
CREATE INDEX IF NOT EXISTS idx_auth_methods_user ON user_auth_methods (user_id);
CREATE INDEX IF NOT EXISTS idx_auth_methods_type ON user_auth_methods (auth_type);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_email ON verification_tokens (email);
CREATE INDEX IF NOT EXISTS idx_verification_tokens_hash ON verification_tokens (token_hash);
CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON api_tokens (token_hash);
CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON api_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_sync_log_workspace_id ON sync_log (workspace_id, sync_id);
