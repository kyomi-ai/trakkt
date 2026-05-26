# Tools Reference

The MCP server exposes 36 tools organized by domain. Each tool maps to a REST API operation -- see the linked API reference pages for complete parameter documentation.

## Issues (6 tools)

See the [Issues API Reference](../api-reference/issues.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_issues` | List issues with optional filters. Ordered by priority (urgent first), then creation date. Excludes completed/cancelled by default -- pass `include_closed=true` to include them. | `issues:read` |
| `search_issues` | Full-text search across titles, descriptions, and comments. Returns ranked results with snippet context. | `issues:read` |
| `get_issue` | Get a single issue by identifier (e.g., `ENG-42`), including description, comments, activity log, and relations. | `issues:read` |
| `create_issue` | Create a new issue. Specify `team_key` (e.g., `"ENG"`) or `team_id` to assign to a team. Starts in backlog status. | `issues:write` |
| `update_issue` | Update an existing issue. Only provided fields change; omit a field to leave it unchanged, set to `null` to clear it. | `issues:write` |
| `delete_issue` | Permanently delete an issue by identifier (e.g., `ENG-42`) and all associated comments and labels. | `issues:write` |

## Comments (1 tool)

See the [Comments API Reference](../api-reference/comments.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `add_comment` | Add a markdown comment to an issue. Supports threaded replies via `parent_id`. | `comments:write` |

## Labels (2 tools)

See the [Labels API Reference](../api-reference/labels.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_labels` | List all labels in the workspace, ordered alphabetically. | `labels:read` |
| `create_label` | Create a new label. Optionally scope it to a team via `team_key` or `team_id`. | `labels:write` |

## Teams (2 tools)

See the [Teams API Reference](../api-reference/teams.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_teams` | List teams the authenticated user belongs to, ordered alphabetically. | `teams:read` |
| `update_team_settings` | Update a team's settings (estimation scale, auto-archive, etc.). | `teams:write` |

## Statuses (1 tool)

See the [Statuses API Reference](../api-reference/statuses.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_statuses` | List all statuses grouped by category (backlog, unstarted, started, completed, cancelled). Returns both global and optionally team-specific statuses. | `issues:read` |

## Relations (3 tools)

See the [Relations API Reference](../api-reference/relations.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `add_relation` | Add a relation between two issues. Types: `blocks`, `parent`, `duplicate`, `relates_to`. | `issues:write` |
| `remove_relation` | Remove a relation by its relation ID. | `issues:write` |
| `list_issue_relations` | List all relations for an issue (both directions). | `issues:read` |

## Projects (5 tools)

See the [Projects API Reference](../api-reference/projects.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_projects` | List all projects in the workspace. | `projects:read` |
| `get_project` | Get a single project by ID, including its milestones. | `projects:read` |
| `create_project` | Create a new project. | `projects:write` |
| `update_project` | Update fields on an existing project. Only provided fields change. | `projects:write` |
| `delete_project` | Permanently delete a project and its milestones. | `projects:write` |

## Milestones (4 tools)

See the [Milestones API Reference](../api-reference/milestones.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_milestones` | List all milestones in a project. | `projects:read` |
| `create_milestone` | Create a new milestone in a project. | `projects:write` |
| `update_milestone` | Update fields on an existing milestone. | `projects:write` |
| `delete_milestone` | Delete a milestone. Linked issues are unlinked but not deleted. | `projects:write` |

## Attachments (4 tools)

See the [Attachments API Reference](../api-reference/attachments.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `upload_attachment` | Upload a file. Returns attachment metadata including download URL. | `attachments:write` |
| `download_attachment` | Download an attachment file by ID. | `attachments:read` |
| `delete_attachment` | Delete an attachment by ID. Only the original uploader can delete. | `attachments:write` |
| `list_attachments` | List all attachments in the workspace. | `attachments:read` |

## Issue Attachments (3 tools)

See the [Attachments API Reference](../api-reference/attachments.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_issue_attachments` | List all attachments linked to an issue. | `attachments:read` |
| `attach_to_issue` | Attach an existing attachment to an issue. Idempotent -- re-attaching is a no-op. | `attachments:write` |
| `detach_from_issue` | Detach an attachment from an issue. Does not delete the attachment itself. | `attachments:write` |

## Activities (2 tools)

See the [Activities API Reference](../api-reference/activities.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_issue_activities` | List all activity entries for an issue, ordered chronologically. | `issues:read` |
| `list_workspace_activities` | List activity entries across all teams, ordered by most recent first. | `issues:read` |

## GitHub (3 tools)

See the [GitHub API Reference](../api-reference/github.md) for parameter details.

| Tool | Description | Scope |
|------|-------------|-------|
| `list_issue_github_links` | List all GitHub links (PRs, branches, commits) associated with an issue. | `issues:read` |
| `lookup_commit` | Look up which issues are linked to a commit SHA. Prefix matching works (7+ characters). | `issues:read` |
| `lookup_branch` | Look up which issues are linked to a branch name. Returns issue details. | `issues:read` |
