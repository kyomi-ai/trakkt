-- Label scoping — SQLite variant.
-- Add optional team_id to labels for team-level label scoping.
-- Labels with team_id = NULL remain workspace-scoped (available to all teams).
-- Labels with a team_id are team-scoped (only available within that team).

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Recreate labels table with team_id and without the old UNIQUE(workspace_id, name)
-- ─────────────────────────────────────────────────────────────────────────────
-- SQLite cannot DROP CONSTRAINT on auto-indexes. The old inline
-- UNIQUE(workspace_id, name) is more restrictive than the new partial indexes
-- and would block team-scoped labels sharing a name with workspace-scoped ones.
-- Use the rename-copy-drop pattern to rebuild the table.

CREATE TABLE IF NOT EXISTS labels_new (
    label_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    team_id TEXT REFERENCES teams(team_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT OR IGNORE INTO labels_new (label_id, workspace_id, team_id, name, color, created_at)
SELECT label_id, workspace_id, NULL, name, color, created_at
FROM labels;

DROP TABLE labels;

ALTER TABLE labels_new RENAME TO labels;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Recreate issue_labels FK references (dropped with old labels table)
-- ─────────────────────────────────────────────────────────────────────────────
-- issue_labels rows survive because SQLite DROP TABLE does not cascade deletes.
-- The FK declarations in the original issue_labels CREATE TABLE reference the
-- labels table by name, which now points to the renamed table.

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Partial unique indexes for scoped uniqueness
-- ─────────────────────────────────────────────────────────────────────────────

-- Workspace-scoped labels: unique name per workspace (team_id IS NULL).
CREATE UNIQUE INDEX IF NOT EXISTS idx_labels_workspace_unique
    ON labels (workspace_id, name) WHERE team_id IS NULL;

-- Team-scoped labels: unique name per team.
CREATE UNIQUE INDEX IF NOT EXISTS idx_labels_team_unique
    ON labels (team_id, name) WHERE team_id IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Recreate indexes on the new labels table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS idx_labels_workspace ON labels (workspace_id);
CREATE INDEX IF NOT EXISTS idx_labels_team ON labels (team_id);
