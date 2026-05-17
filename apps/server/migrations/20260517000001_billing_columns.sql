-- Add billing columns to workspaces table for Stripe integration.

ALTER TABLE workspaces ADD COLUMN subscription_status VARCHAR(20) DEFAULT 'free';
ALTER TABLE workspaces ADD COLUMN stripe_customer_id VARCHAR(100);
ALTER TABLE workspaces ADD COLUMN stripe_subscription_id VARCHAR(100);
ALTER TABLE workspaces ADD COLUMN subscription_period_start TIMESTAMPTZ;
ALTER TABLE workspaces ADD COLUMN subscription_period_end TIMESTAMPTZ;
ALTER TABLE workspaces ALTER COLUMN user_limit SET DEFAULT 1;
