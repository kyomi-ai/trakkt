-- Soft removal preserves historical installations and permanent account claims.
ALTER TABLE github_installations ADD COLUMN archived_at TIMESTAMPTZ;
-- Add-account intent records only already archived generations eligible for restoration.
ALTER TABLE github_connection_states ADD COLUMN archived_installation_generations TEXT;
