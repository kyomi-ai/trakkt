-- Issue attachments: junction table linking issues to attachments.
CREATE TABLE issue_attachments (
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    attachment_id TEXT NOT NULL REFERENCES attachments(attachment_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issue_id, attachment_id)
);

CREATE INDEX idx_issue_attachments_issue ON issue_attachments(issue_id);
CREATE INDEX idx_issue_attachments_attachment ON issue_attachments(attachment_id);
