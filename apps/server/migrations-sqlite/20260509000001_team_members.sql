-- Team members — SQLite variant.
-- Adds description/icon columns to teams and creates the team_members join table.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Add columns to teams
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE teams ADD COLUMN description TEXT;
ALTER TABLE teams ADD COLUMN icon TEXT;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Create team_members table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS team_members (
    team_id TEXT NOT NULL REFERENCES teams(team_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    role TEXT DEFAULT 'member',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (team_id, user_id)
);

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Indexes
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_team_members_user ON team_members (user_id);
CREATE INDEX IF NOT EXISTS idx_team_members_team ON team_members (team_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Seed: add all workspace members to the default team of each workspace
-- ─────────────────────────────────────────────────────────────────────────────

INSERT OR IGNORE INTO team_members (team_id, user_id, role, created_at)
SELECT
    dt.team_id,
    wu.user_id,
    'member',
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM workspace_users wu
JOIN (
    SELECT t.team_id, t.workspace_id
    FROM teams t
    WHERE t.created_at = (
        SELECT MIN(t2.created_at) FROM teams t2 WHERE t2.workspace_id = t.workspace_id
    )
) dt ON dt.workspace_id = wu.workspace_id;
