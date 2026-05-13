ALTER TABLE notifications ADD COLUMN actor_id TEXT REFERENCES users(user_id);
