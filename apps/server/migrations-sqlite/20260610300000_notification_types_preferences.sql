-- Add preference columns for new notification types.
ALTER TABLE notification_preferences ADD COLUMN notify_label_changes BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE notification_preferences ADD COLUMN notify_due_date_changes BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE notification_preferences ADD COLUMN notify_estimate_changes BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE notification_preferences ADD COLUMN notify_milestone_changes BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE notification_preferences ADD COLUMN notify_project_changes BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE notification_preferences ADD COLUMN notify_team_changes BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE notification_preferences ADD COLUMN notify_relation_changes BOOLEAN NOT NULL DEFAULT 1;
