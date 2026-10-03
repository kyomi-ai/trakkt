-- TRA-9989: make notifications.issue_id cascade when its issue is deleted.
--
-- The Postgres counterpart of this migration carries the full reasoning for
-- choosing CASCADE over SET NULL, including the trade-off it accepts (a user's
-- inbox silently loses an entry they may never have read). Read
-- `apps/server/migrations/20260803000000_notification_issue_cascade.sql`; this
-- file only differs in how the constraint is changed.
--
-- SQLite cannot ALTER a constraint, so this uses the rename-copy-drop pattern
-- of 20260512000000_team_scoped_numbering.sql and
-- 20260514000000_parent_to_relations.sql.
--
-- The column list below is the table as it actually exists after every earlier
-- migration that touches it, not as 20260507000000_issue_tracker.sql declares
-- it. Verified against a database with the full migration set replayed
-- (`PRAGMA table_info(notifications)`): the seven original columns plus
-- `actor_id` (20260513000002_notification_actor), `action_source` and
-- `action_source_label` (20260520100000_action_source), `deleted_at`
-- (20260528000000_notification_soft_delete) and `context_id`
-- (20260610400000_notification_context_id) — twelve in all. Dropping one here
-- would silently destroy that column's production data.
--
-- No `PRAGMA foreign_keys = OFF` guard: `notifications` is a child table only.
-- `PRAGMA foreign_key_list` on the migrated schema shows its four outbound keys
-- (issues, users twice, workspaces) and nothing in `sqlite_master` references
-- `notifications` — no other table, no view, no trigger — so neither the DROP
-- nor the RENAME can leave a dangling reference. The rows copied below already
-- satisfy the outbound keys, being the same rows under the same constraints.

CREATE TABLE notifications_new (
    notification_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    user_id TEXT NOT NULL REFERENCES users(user_id),
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    read INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    actor_id TEXT REFERENCES users(user_id),
    action_source TEXT NOT NULL DEFAULT 'user',
    action_source_label TEXT,
    deleted_at TEXT NULL,
    context_id TEXT NULL
);

INSERT INTO notifications_new (
    notification_id, workspace_id, user_id, issue_id, type, read, created_at,
    actor_id, action_source, action_source_label, deleted_at, context_id
)
SELECT
    notification_id, workspace_id, user_id, issue_id, type, read, created_at,
    actor_id, action_source, action_source_label, deleted_at, context_id
FROM notifications;

DROP TABLE notifications;

ALTER TABLE notifications_new RENAME TO notifications;

-- The one index the table carried, from 20260507000000_issue_tracker.sql.
-- Dropping the old table dropped it; no later migration added another.
CREATE INDEX IF NOT EXISTS idx_notifications_user_unread ON notifications (user_id, read, created_at);
