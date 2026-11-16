-- TRA-10115: link and automate by intent, the way Linear does.
--
-- A key found in text is not a link. `link_kind` records what the link is
-- allowed to do; status automation reads it instead of "a key was found".
--   closing       a closing magic word: starts the issue and completes it on merge
--   contributing  a non-closing magic word: starts the issue, never completes it
--   branch_title  the key is in the branch name or PR title: starts the issue
--   related       shown on the issue and the PR, never changes status
ALTER TABLE github_links
    ADD COLUMN link_kind TEXT NOT NULL DEFAULT 'related'
    CHECK (link_kind IN ('closing', 'contributing', 'branch_title', 'related'));

-- Conservative backfill: only what can be shown to be a closing or branch link
-- keeps its power. A PR link without close intent might have come from the
-- body, which is a mention, so it becomes related.
UPDATE github_links SET link_kind = 'closing' WHERE close_intent;
UPDATE github_links SET link_kind = 'branch_title' WHERE link_type = 'branch' AND NOT close_intent;

ALTER TABLE github_links DROP COLUMN close_intent;

-- Per-team automation. A row replaces the workspace default rule for one
-- trigger on that team's issues. A NULL target means "no action".
CREATE TABLE github_team_transition_rules (
    rule_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    team_id TEXT NOT NULL REFERENCES teams(team_id) ON DELETE CASCADE,
    trigger_event TEXT NOT NULL,
    target_status_category TEXT
        CHECK (target_status_category IN ('backlog', 'unstarted', 'started', 'completed', 'cancelled')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(team_id, trigger_event)
);

-- Workspace automation settings that are not a status rule. No row = defaults.
CREATE TABLE github_automation_settings (
    workspace_id TEXT NOT NULL PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    linkback_enabled INTEGER NOT NULL DEFAULT 1
);
