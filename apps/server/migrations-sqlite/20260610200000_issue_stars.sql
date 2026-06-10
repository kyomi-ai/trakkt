CREATE TABLE IF NOT EXISTS issue_stars (
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (issue_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_issue_stars_user ON issue_stars (user_id);
