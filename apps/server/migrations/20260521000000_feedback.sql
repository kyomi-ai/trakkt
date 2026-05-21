-- User feedback: bug reports, feature requests, and questions.
CREATE TABLE feedback (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    feedback_type TEXT NOT NULL CHECK (feedback_type IN ('bug', 'feature', 'question')),
    description TEXT NOT NULL,
    screenshot_url TEXT,
    include_context BOOLEAN NOT NULL DEFAULT true,
    context JSONB,
    status TEXT NOT NULL DEFAULT 'new' CHECK (status IN ('new', 'reviewed', 'resolved', 'closed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    resolution_notes TEXT,
    resolved_by TEXT REFERENCES users(user_id)
);

CREATE INDEX idx_feedback_workspace ON feedback(workspace_id, created_at DESC);
CREATE INDEX idx_feedback_user ON feedback(user_id);
CREATE INDEX idx_feedback_status ON feedback(workspace_id, status);
