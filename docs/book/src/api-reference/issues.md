# Issues

## `GET /issues`

**Operation:** `list_issues`

Find issues in the workspace with optional filters. Returns issues ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them. Supports a `filters` parameter: a JSON array of `{field, operator, values}` clauses AND-ed together. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations. Operators: any_of, none_of, all_of, not_any_of, not_all_of. Response shape: `{issues, matched_count, returned_count, truncated}`. Each row is lean by design — `number`, `key` (e.g. 'TRA-35'), `title`, `priority`, `status_id`, `status_name`, `updated_at`, and `labels` (id and name only) — enough to find, sort, and triage. Rows never include the issue description, comments, or activities, and there is no option to add them: descriptions are multi-KB and would dominate the payload. To read a ticket, call get_issue with the row's `key`.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `assignee` | query | string (nullable) | No | Filter by assignee user ID |
| `filters` | query | string (nullable) | No | JSON array of composable filter clauses, AND-ed together. Each clause is `{"field","operator","values"}`. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations. Operators: any_of, none_of, all_of, not_any_of, not_all_of. Example: `[{"field":"label","operator":"none_of","values":["label-id-1"]}]` |
| `include_closed` | query | boolean (nullable) | No | If true, include completed and cancelled issues |
| `label` | query | string (nullable) | No | Filter by label ID(s). Comma-separated for multiple (OR logic) |
| `limit` | query | integer (int64) (nullable) | No | Maximum number of issues to return (default: 50, max: 100) |
| `priority` | query | integer (int32) (nullable) | No | Filter by priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low |
| `search` | query | string (nullable) | No | Search text to match against issue titles |
| `status_category` | query | string (nullable) | No | Comma-separated status categories: backlog, unstarted, started, completed, cancelled |
| `status_id` | query | string (nullable) | No | Filter by status ID |
| `team_id` | query | string (nullable) | No | Filter by team ID |
| `team_key` | query | string (nullable) | No | Filter by team key (e.g. 'TRA') |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /issues`

**Operation:** `create_issue`

Create a new issue in the workspace. Specify team_id or team_key to assign to a specific team, otherwise uses the default team. Starts in 'backlog' status.

**Scope:** `issues:write`

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `assignee` | string (nullable) | No | User ID to assign |
| `description` | string (nullable) | No | Markdown description |
| `due_date` | string (nullable) | No | Due date in ISO 8601 format (YYYY-MM-DD) |
| `estimate` | integer (int32) (nullable) | No | Estimate points value (integer) |
| `labels` | array<string> (nullable) | No | Array of label IDs |
| `milestone_id` | string (nullable) | No | Milestone ID to associate with |
| `parent_issue_id` | string (nullable) | No | Parent issue ID for sub-issues |
| `priority` | integer (int32) (nullable) | No | Priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low |
| `project_id` | string (nullable) | No | Project ID to associate with |
| `relations` | array<InlineRelation> (nullable) | No | Relations to create after issue creation. Each entry links the new issue to an existing issue. Supports directional sugar: "blocked_by" creates a "blocks" relation with the referenced issue as the blocker. |
| `team_id` | string (nullable) | No | Team ID to assign issue to |
| `team_key` | string (nullable) | No | Team key to assign issue to |
| `title` | string | Yes | Issue title (required) |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/issues" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"title": "Bug report"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `GET /issues/search`

**Operation:** `search_issues`

Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns `{results, total}` where results are ranked by relevance with snippet context, and total is the full match count for pagination. Supports `offset` for pagination. By default searches comments too — pass include_comments=false to search only titles and descriptions. By default excludes archived issues — pass include_archived=true to include them.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `include_archived` | query | boolean (nullable) | No | Include archived issues in results (default: false) |
| `include_closed` | query | boolean (nullable) | No | If true, include completed and cancelled issues |
| `include_comments` | query | boolean (nullable) | No | Also search comment bodies (default: true) |
| `limit` | query | integer (int64) (nullable) | No | Max results (default: 20, max: 100) |
| `offset` | query | integer (int64) (nullable) | No | Offset for pagination (default: 0) |
| `query` | query | string | Yes | Search text (required) |
| `team_id` | query | string (nullable) | No | Filter by team ID |
| `team_key` | query | string (nullable) | No | Filter by team key |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues/search?query=search+term" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `DELETE /issues/{identifier}`

**Operation:** `delete_issue`

Delete an issue by its team-scoped identifier (e.g. 'TRA-35'). This permanently removes the issue and all associated comments and labels.

**Scope:** `issues:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/issues/TRA-35" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}`

**Operation:** `get_issue`

Read a single issue in full by its team-scoped identifier (e.g. 'TRA-35'). This is the way to get an issue's content: it returns the complete record (description, labels, assignee, creator), all comments, the activity log, and relations. list_issues returns lean rows without descriptions, so the normal pattern is to list first, then call get_issue for each ticket you actually need to read.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `issue_number` | query | integer (int64) (nullable) | No | Issue number within the team |
| `team_key` | query | string (nullable) | No | Team key. Required if issue_identifier is not provided |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues/TRA-35" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /issues/{identifier}`

**Operation:** `update_issue`

Update an existing issue. Only provided fields are changed; omitted fields remain unchanged. Set a field to null to clear it.

**Scope:** `issues:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `assignee` | string (nullable) | No | User ID to assign, or null to unassign |
| `description` | string (nullable) | No | New markdown description, or null to clear |
| `due_date` | string (nullable) | No | Due date in ISO 8601 format, or null to clear |
| `estimate` | integer (int32) (nullable) | No | Estimate points value, or null to clear |
| `issue_identifier` | string (nullable) | No | Issue identifier in 'TRA-35' format |
| `issue_number` | integer (int64) (nullable) | No | Issue number within the team |
| `labels` | array<string> (nullable) | No | Replace all labels with this list of label IDs |
| `milestone_id` | string (nullable) | No | Milestone ID, or null to clear |
| `move_to_team_id` | string (nullable) | No | Team ID to move the issue to |
| `move_to_team_key` | string (nullable) | No | Team key to move the issue to |
| `parent_issue_id` | string (nullable) | No | Parent issue ID, or null to clear |
| `priority` | integer (int32) (nullable) | No | New priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low |
| `project_id` | string (nullable) | No | Project ID, or null to clear |
| `sort_order` | number (nullable) | No | Sort order, or null to clear |
| `status_id` | string (nullable) | No | New status ID |
| `team_key` | string (nullable) | No | Team key. Required if issue_identifier is not provided |
| `title` | string (nullable) | No | New title for the issue |

### Example

```bash
curl -X PATCH "https://your-trakkt-instance.com/api/v1/issues/TRA-35" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `DELETE /issues/{id}/star`

**Operation:** `unstar_issue`

Unstar an issue for the current user.

**Scope:** `issues:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/issues/abc123/star" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /issues/{id}/star`

**Operation:** `star_issue`

Star an issue for the current user.

**Scope:** `issues:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `issue_id` | string | Yes | The issue ID to star (e.g. 'iss_abc123') |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/issues/abc123/star" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"issue_id": "example-issue_id"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

