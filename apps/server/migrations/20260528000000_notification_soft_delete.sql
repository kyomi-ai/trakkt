-- Add soft-delete support to notifications.
ALTER TABLE notifications ADD COLUMN deleted_at TIMESTAMPTZ NULL;
