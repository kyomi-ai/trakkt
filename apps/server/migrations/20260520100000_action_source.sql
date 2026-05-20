-- Add action_source tracking to user-attributable entities.
-- Tracks whether an action came from a user session, an agent (MCP/OAuth), or an API token.

ALTER TABLE issue_activities
    ADD COLUMN action_source TEXT NOT NULL DEFAULT 'user',
    ADD COLUMN action_source_label TEXT;

ALTER TABLE comments
    ADD COLUMN action_source TEXT NOT NULL DEFAULT 'user',
    ADD COLUMN action_source_label TEXT;

ALTER TABLE notifications
    ADD COLUMN action_source TEXT NOT NULL DEFAULT 'user',
    ADD COLUMN action_source_label TEXT;
