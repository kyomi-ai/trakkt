-- GitHub-authored activities (commits, PRs) may have no matching Trakkt user.
ALTER TABLE issue_activities ALTER COLUMN actor_id DROP NOT NULL;
