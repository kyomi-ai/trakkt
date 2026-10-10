-- Installation IDs are immutable; account claims retain ownership across reinstalls.
DROP INDEX github_installations_account_owner;
ALTER TABLE github_connection_states ADD COLUMN expected_token_generation BIGINT;
ALTER TABLE github_installations ADD COLUMN disconnected_at TEXT;
ALTER TABLE github_installations ADD COLUMN uninstalled_at TEXT;
ALTER TABLE github_installations ADD COLUMN authorization_verified_at TEXT;
ALTER TABLE github_installations ADD COLUMN repository_selection TEXT NOT NULL DEFAULT 'selected' CHECK (repository_selection IN ('all', 'selected'));
ALTER TABLE github_installations ADD COLUMN token_generation BIGINT NOT NULL DEFAULT 0;
ALTER TABLE github_installations ADD COLUMN repository_scope_pending INTEGER NOT NULL DEFAULT 0;
-- Keep encrypted historical credentials, but quarantine them until verified reconnect.
-- Legacy NULL represented all repositories, including possible old overwritten rows.
UPDATE github_installations SET repository_selection = CASE WHEN target_repos IS NULL THEN 'all' ELSE 'selected' END;
-- Old suspension conflated local disconnect and GitHub suspension. Fail closed until
-- verified reconnect; a GitHub unsuspend must not revive an old local disconnect.
UPDATE github_installations SET disconnected_at = suspended_at WHERE suspended_at IS NOT NULL;
CREATE INDEX github_installations_workspace_list ON github_installations(workspace_id, account_login, installation_id);

CREATE TRIGGER github_installations_immutable_identity BEFORE UPDATE ON github_installations
WHEN NEW.installation_id <> OLD.installation_id
        OR NEW.github_installation_id <> OLD.github_installation_id
    OR NEW.workspace_id <> OLD.workspace_id
    OR NEW.github_app_id <> OLD.github_app_id
    OR (OLD.github_account_id IS NOT NULL AND NEW.github_account_id IS NOT OLD.github_account_id)
BEGIN SELECT RAISE(ABORT, 'GitHub installation identity and ownership are immutable'); END;
