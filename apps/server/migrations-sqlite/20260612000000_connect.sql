-- Connect agent and session tables — SQLite variant for personal/self-hosted mode.
--
-- NOTE: These tables are reserved for future audit logging and session history.
-- The ConnectManager currently operates entirely in-memory. No Rust code reads
-- or writes these tables yet.

-- ---------------------------------------------------------------------------
-- Tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS connect_agents (
    agent_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    hostname TEXT,
    agent_version TEXT,
    os TEXT,
    last_seen_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS connect_sessions (
    session_id TEXT NOT NULL PRIMARY KEY,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    command TEXT,
    working_dir TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    exit_code INTEGER,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    ended_at TEXT
);

-- ---------------------------------------------------------------------------
-- Indexes
-- ---------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_connect_agents_workspace ON connect_agents (workspace_id);
CREATE INDEX IF NOT EXISTS idx_connect_sessions_workspace ON connect_sessions (workspace_id);
CREATE INDEX IF NOT EXISTS idx_connect_sessions_user ON connect_sessions (user_id);
