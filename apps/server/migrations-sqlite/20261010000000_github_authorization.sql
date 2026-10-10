-- Workspace-bound GitHub user authorization and permanent ownership claims.
ALTER TABLE github_installations ADD COLUMN github_account_id BIGINT;
CREATE UNIQUE INDEX github_installations_account_owner ON github_installations(github_app_id, github_account_id) WHERE github_account_id IS NOT NULL;
CREATE TABLE github_connection_states (
    state_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('connect', 'reconnect')),
    expected_installation_id BIGINT,
    expected_account_id BIGINT,
    installation_id BIGINT,
    verifier_encrypted TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('setup', 'oauth', 'consumed'))
);
CREATE TABLE github_account_claims (
    app_id BIGINT NOT NULL,
    account_id BIGINT NOT NULL,
    account_type TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY (app_id, account_id)
);
CREATE TABLE github_installation_claims (
    installation_id BIGINT PRIMARY KEY NOT NULL,
    app_id BIGINT NOT NULL,
    account_id BIGINT NOT NULL,
    workspace_id TEXT NOT NULL
);
-- Retain existing installation ownership, including disconnected rows.
INSERT INTO github_installation_claims (installation_id, app_id, account_id, workspace_id)
SELECT gi.github_installation_id, ga.app_id, 0, gi.workspace_id
FROM github_installations gi JOIN github_apps ga ON ga.github_app_id = gi.github_app_id;
