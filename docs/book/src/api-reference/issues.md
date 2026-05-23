# Issues

## `GET /issues`

**Operation:** `list_issues`

List issues in the workspace with optional filters. Returns issues ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `assignee` | query | any | No |  |
| `include_closed` | query | any | No |  |
| `label` | query | any | No |  |
| `limit` | query | any | No |  |
| `priority` | query | any | No |  |
| `search` | query | any | No |  |
| `status_category` | query | any | No |  |
| `status_id` | query | any | No |  |
| `team_id` | query | any | No |  |
| `team_key` | query | any | No |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /issues`

**Operation:** `create_issue`

Create a new issue in the workspace. Specify team_id or team_key to assign to a specific team, otherwise uses the default team. Starts in 'backlog' status.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `assignee` | any | No | User ID to assign |
| `description` | any | No | Markdown description |
| `due_date` | any | No | Due date in ISO 8601 format (YYYY-MM-DD) |
| `estimate` | any | No | Estimate points value (integer) |
| `labels` | any | No | Array of label IDs |
| `milestone_id` | any | No | Milestone ID to associate with |
| `parent_issue_id` | any | No | Parent issue ID for sub-issues |
| `priority` | any | No | Priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low |
| `project_id` | any | No | Project ID to associate with |
| `relations` | any | No | Relations to create after issue creation. Each entry links the new issue to an existing issue. Supports directional sugar: "blocked_by" creates a "blocks" relation with the referenced issue as the blocker. |
| `team_id` | any | No | Team ID to assign issue to |
| `team_key` | any | No | Team key to assign issue to |
| `title` | string | Yes | Issue title (required) |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `GET /issues/search`

**Operation:** `search_issues`

Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns results ranked by relevance with snippet context showing where the match was found. By default searches comments too — pass include_comments=false to search only titles and descriptions. By default excludes archived issues — pass include_archived=true to include them.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `include_archived` | query | any | No |  |
| `include_closed` | query | any | No |  |
| `include_comments` | query | any | No |  |
| `limit` | query | any | No |  |
| `query` | query | string | Yes |  |
| `team_id` | query | any | No |  |
| `team_key` | query | any | No |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `DELETE /issues/{identifier}`

**Operation:** `delete_issue`

Delete an issue by its team-scoped identifier (e.g. 'TRA-35'). This permanently removes the issue and all associated comments and labels.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}`

**Operation:** `get_issue`

Get a single issue by its team-scoped identifier (e.g. 'TRA-35'), including full details (description, labels, assignee, creator), all comments, activity log, and relations.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `issue_identifier` | query | any | No |  |
| `issue_number` | query | any | No |  |
| `team_key` | query | any | No |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /issues/{identifier}`

**Operation:** `update_issue`

Update an existing issue. Only provided fields are changed; omitted fields remain unchanged. Set a field to null to clear it.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `assignee` | any | No | User ID to assign, or null to unassign |
| `description` | any | No | New markdown description, or null to clear |
| `due_date` | any | No | Due date in ISO 8601 format, or null to clear |
| `estimate` | any | No | Estimate points value, or null to clear |
| `issue_identifier` | any | No | Issue identifier in 'TRA-35' format |
| `issue_number` | any | No | Issue number within the team |
| `labels` | any | No | Replace all labels with this list of label IDs |
| `milestone_id` | any | No | Milestone ID, or null to clear |
| `move_to_team_id` | any | No | Team ID to move the issue to |
| `move_to_team_key` | any | No | Team key to move the issue to |
| `parent_issue_id` | any | No | Parent issue ID, or null to clear |
| `priority` | any | No | New priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low |
| `project_id` | any | No | Project ID, or null to clear |
| `sort_order` | any | No | Sort order, or null to clear |
| `status_id` | any | No | New status ID |
| `team_key` | any | No | Team key. Required if issue_identifier is not provided |
| `title` | any | No | New title for the issue |

### Response

Returns `200 OK` on success with the result as JSON.

---

