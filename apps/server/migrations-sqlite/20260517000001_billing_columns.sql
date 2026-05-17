-- Add billing columns to workspaces table for Stripe integration.
-- NOTE: SQLite does not support ALTER COLUMN; user_limit DEFAULT remains NULL.
-- Trakkt requires Postgres for production; SQLite is dev/test only.

ALTER TABLE workspaces ADD COLUMN subscription_status VARCHAR(20) DEFAULT 'free';
ALTER TABLE workspaces ADD COLUMN stripe_customer_id VARCHAR(100);
ALTER TABLE workspaces ADD COLUMN stripe_subscription_id VARCHAR(100);
ALTER TABLE workspaces ADD COLUMN subscription_period_start TEXT;
ALTER TABLE workspaces ADD COLUMN subscription_period_end TEXT;
