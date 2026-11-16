# Issues

## `GET /issues`

**Operation:** `list_issues`

Find issues in the workspace with optional filters. Returns `{issues, total, has_more, next_cursor, limit, sort_by, sort_dir, matched_count, returned_count, truncated}`. `total` is the exact number of matching issues ignoring `limit`; a response is complete only when `has_more` is false. Pagination: `limit` is the page size (default 50, max 100; out-of-range values are clamped and the applied value is echoed as `limit`). When `has_more` is true, call again with the same filters and `cursor` set to `next_cursor` (an opaque string; changing any filter or sort invalidates it) until `has_more` is false. The cursor resumes after the last row served by its sort key, so unchanged issues appear exactly once even if other issues are created, deleted or edited between pages; an issue edited so that it now sorts before the cursor (for example a `sort_by=updated_at` walk) can be missed. Sorting: `sort_by` is priority (default), created_at, updated_at or number; `sort_dir` is asc or desc (default asc for priority, so Urgent (1), High (2), Medium (3), Low (4), None (0) last, newest first within a priority; default desc for created_at, updated_at and number). Ties are broken deterministically so pages are stable. By default, completed and cancelled issues are excluded — pass include_closed=true to include them. Supports a `filters` parameter: a JSON array of `{field, operator, values}` clauses AND-ed together. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations. Operators: any_of, none_of, all_of, not_any_of, not_all_of. `matched_count` equals `total` and `truncated` equals `has_more` (kept for older callers). Each row is lean by design — `number`, `key` (e.g. 'TRA-35'), `title`, `priority`, `priority_name` (Urgent, High, Medium, Low, or No priority), `status_id`, `status_name`, `created_at`, `updated_at`, and `labels` (id and name only) — enough to find, sort, and triage. Rows never include the issue description, comments, or activities, and there is no option to add them: descriptions are multi-KB and would dominate the payload. To read a ticket, call get_issue with the row's `key`.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `assignee` | query | string (nullable) | No | Filter by assignee user ID |
| `cursor` | query | string (nullable) | No | Opaque pagination cursor: the `next_cursor` of a previous response to the same query (same filters, sort_by and sort_dir). Omit for page 1. |
| `filters` | query | string (nullable) | No | JSON array of composable filter clauses, AND-ed together. Each clause is `{"field","operator","values"}`. Fields: status, priority, label, project, is_sub_issue, is_parent, is_blocked, is_blocking, has_relations. Operators: any_of, none_of, all_of, not_any_of, not_all_of. Example: `[{"field":"label","operator":"none_of","values":["label-id-1"]}]` |
| `include_closed` | query | boolean (nullable) | No | If true, include completed and cancelled issues |
| `label` | query | string (nullable) | No | Filter by label ID(s). Comma-separated for multiple (OR logic) |
| `limit` | query | integer (int64) (nullable) | No | Page size (default: 50, max: 100; values outside 1..=100 are clamped). The response always carries `total` and `has_more`; when `has_more` is true, pass `next_cursor` as `cursor` to fetch the next page. |
| `priority` | query | integer (int32) (nullable) | No | Filter by priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low |
| `search` | query | string (nullable) | No | Search text to match against issue titles |
| `sort_by` | query | string (nullable) | No | Sort field: priority (default), created_at, updated_at or number. Ties are always broken deterministically (by creation time, number, then issue id) so pagination is stable. |
| `sort_dir` | query | string (nullable) | No | Sort direction: asc or desc. Default: asc for priority (Urgent first), desc for created_at, updated_at and number (newest first). |
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

Search for issues by text query. Uses full-text search across titles, descriptions, and comments (Postgres) or LIKE matching (SQLite). Returns `{results, total, has_more, next_offset, truncated}`; results are ranked by relevance with snippet context. `limit` is the page size (default 20, max 100, clamped). `total` is the number of matches ignoring limit/offset; when `has_more` is true, repeat the request with `offset` set to `next_offset` until `has_more` is false. A search considers at most its 1000 best-ranked matches: when `truncated` is true, `total` is a lower bound and lower-ranked matches cannot be reached by paging, so narrow the query (team, status filters, include_comments). Search applies only the filters you pass and no hidden defaults: with no status filter it covers every status, including Done and Cancelled, and archived issues. Narrow with status_id, status_category (comma-separated: backlog, unstarted, started, completed, cancelled), team_key/team_id, include_archived=false, or include_comments=false (comments are searched by default). An exact identifier such as 'TRA-35' always matches that issue unless a filter you set excludes it. include_closed is deprecated and kept for compatibility: omitted or true means no status filtering; only an explicit false excludes completed and cancelled issues.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `include_archived` | query | boolean (nullable) | No | Omitted or true: archived issues are searched. Pass false to exclude them. |
| `include_closed` | query | boolean (nullable) | No | Deprecated, kept for compatibility. Omitted or true: no status filtering (completed and cancelled issues are searched). Only an explicit false excludes completed and cancelled issues; prefer `status_category`. |
| `include_comments` | query | boolean (nullable) | No | Also search comment bodies (default: true) |
| `limit` | query | integer (int64) (nullable) | No | Page size (default: 20, max: 100; values outside 1..=100 are clamped) |
| `offset` | query | integer (int64) (nullable) | No | Offset of the first result (default: 0). When the response has `has_more`, repeat the request with `offset` = `next_offset`. |
| `query` | query | string | Yes | Search text (required) |
| `status_category` | query | string (nullable) | No | Comma-separated status categories to restrict to: backlog, unstarted, started, completed, cancelled. Omit to search every status, including completed and cancelled. |
| `status_id` | query | string (nullable) | No | Filter by status ID. Omit to search every status. |
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

