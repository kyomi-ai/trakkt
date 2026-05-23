# MCP Server

Trakkt includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server that lets AI agents interact with your issue tracker programmatically. The MCP server exposes the same 36 operations as the REST API, so agents have full access to create issues, update statuses, search, and more.

## Connecting to the MCP Server

### Claude Code

Add Trakkt as an MCP server in your Claude Code configuration:

```json
{
  "mcpServers": {
    "trakkt": {
      "type": "sse",
      "url": "https://your-trakkt-instance.com/mcp/sse",
      "headers": {
        "Authorization": "Bearer <your-api-token>"
      }
    }
  }
}
```

### Other MCP Clients

Any MCP-compatible client can connect to Trakkt's SSE transport at `/mcp/sse`. The server implements the MCP tools protocol, exposing each operation as a callable tool with typed JSON Schema parameters.

## Available Tools

The MCP server exposes the following 36 tools, organized by domain:

### Issues (6 tools)

| Tool | Description |
|------|-------------|
| `list_issues` | List issues in the workspace with optional filters. Returns issues ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded -- pass `include_closed=true` to include them. |
| `search_issues` | Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns results ranked by relevance with snippet context. |
| `get_issue` | Get a single issue by its team-scoped identifier (e.g. `TRA-35`), including full details, all comments, activity log, and relations. |
| `create_issue` | Create a new issue in the workspace. Specify `team_id` or `team_key` to assign to a specific team, otherwise uses the default team. Starts in backlog status. |
| `update_issue` | Update an existing issue. Only provided fields are changed; omitted fields remain unchanged. Set a field to `null` to clear it. |
| `delete_issue` | Delete an issue by its team-scoped identifier (e.g. `TRA-35`). Permanently removes the issue and all associated comments and labels. |

### Comments (1 tool)

| Tool | Description |
|------|-------------|
| `add_comment` | Add a comment to an issue. Comments support markdown formatting. |

### Labels (2 tools)

| Tool | Description |
|------|-------------|
| `list_labels` | List all labels in the workspace, ordered alphabetically by name. |
| `create_label` | Create a new label in the workspace. Optionally scope it to a team by providing `team_key` or `team_id`. |

### Teams (2 tools)

| Tool | Description |
|------|-------------|
| `list_teams` | List teams the authenticated user belongs to, ordered alphabetically by name. |
| `update_team_settings` | Update a team's settings including estimation scale, auto-archive, and other configuration. Provide `team_key` or `team_id` to identify the team. |

### Statuses (1 tool)

| Tool | Description |
|------|-------------|
| `list_statuses` | List all statuses in the workspace, grouped by category (backlog, unstarted, started, completed, cancelled). Returns both global and optionally team-specific statuses. |

### Relations (3 tools)

| Tool | Description |
|------|-------------|
| `add_relation` | Add a relation between two issues. Supports `blocks` (source blocks target), `parent` (source is parent of target), and `duplicate` (source is duplicate of target) relation types. |
| `remove_relation` | Remove a relation between two issues by its relation ID. |
| `list_issue_relations` | List all relations for an issue (both directions -- blocks and blocked-by). |

### Projects (5 tools)

| Tool | Description |
|------|-------------|
| `list_projects` | List all projects in the workspace. |
| `get_project` | Get a single project by its ID, including milestones. |
| `create_project` | Create a new project in the workspace. |
| `update_project` | Update fields on an existing project. Only provided fields are changed. |
| `delete_project` | Permanently delete a project and its milestones. |

### Milestones (4 tools)

| Tool | Description |
|------|-------------|
| `list_milestones` | List all milestones in a project. |
| `create_milestone` | Create a new milestone in a project. |
| `update_milestone` | Update fields on an existing milestone. |
| `delete_milestone` | Delete a milestone. Issues linked to this milestone will be unlinked. |

### Attachments (4 tools)

| Tool | Description |
|------|-------------|
| `upload_attachment` | Upload a file attachment. Returns the attachment metadata including download URL. |
| `download_attachment` | Download an attachment file by ID. |
| `delete_attachment` | Delete an attachment by ID. Only the original uploader can delete. |
| `list_attachments` | List all attachments in the workspace. |

### Issue Attachments (3 tools)

| Tool | Description |
|------|-------------|
| `list_issue_attachments` | List all attachments linked to an issue. |
| `attach_to_issue` | Attach an existing attachment to an issue. Idempotent -- re-attaching is a no-op. |
| `detach_from_issue` | Detach an attachment from an issue. Does not delete the attachment itself. |

### Activities (2 tools)

| Tool | Description |
|------|-------------|
| `list_issue_activities` | List all activity entries for an issue, ordered chronologically. |
| `list_workspace_activities` | List activity entries across all teams in the workspace, ordered by most recent first. |

### GitHub (3 tools)

| Tool | Description |
|------|-------------|
| `list_issue_github_links` | List all GitHub links (PRs, branches, commits) associated with an issue. |
| `lookup_commit` | Look up which issues are linked to a given commit SHA. Uses prefix matching so abbreviated SHAs (7+ characters) work. Returns issue details including identifier, title, status, and description. |
| `lookup_branch` | Look up which issues are linked to a given branch name. Returns issue details including identifier, title, status, and description. |

## Agent Workflow Example

A typical AI agent workflow using the MCP server:

1. `list_teams` -- discover available teams and their keys
2. `list_issues` -- see current work items with filters
3. `create_issue` -- file a new bug or feature request
4. `update_issue` -- change status, priority, or assignee
5. `add_comment` -- leave notes or status updates
6. `lookup_branch` -- find which issues relate to a git branch
