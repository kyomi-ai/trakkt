CREATE TABLE IF NOT EXISTS feedback (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    feedback_type TEXT NOT NULL CHECK (feedback_type IN ('bug', 'feature', 'question')),
    description TEXT NOT NULL,
    screenshot_url TEXT,
    include_context INTEGER NOT NULL DEFAULT 1,
    context TEXT,
    status TEXT NOT NULL DEFAULT 'new' CHECK (status IN ('new', 'reviewed', 'resolved', 'closed')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    resolved_at TEXT,
    resolution_notes TEXT,
    resolved_by TEXT REFERENCES users(user_id)
);

CREATE INDEX IF NOT EXISTS idx_feedback_workspace ON feedback(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON feedback(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_status ON feedback(workspace_id, status);
