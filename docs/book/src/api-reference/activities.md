# Activities

## `GET /activities`

**Operation:** `list_workspace_activities`

List activity entries across all teams in the workspace, most recent first (ties broken by activity id so pages are stable). Returns `{activities, limit, offset, has_more, next_offset}`. `limit` is the page size (default 50, max 200, clamped; the applied value is echoed). A response is complete only when `has_more` is false; otherwise repeat the request with `offset` set to `next_offset`. Paging is by offset over a newest-first feed, so activities recorded between requests can shift later pages and repeat a row.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `action_source` | query | string (nullable) | No | Filter by action source: "user", "agent", or "api" |
| `action_type` | query | string (nullable) | No | Filter by action type (e.g. "status_changed", "comment_added") |
| `actor_id` | query | string (nullable) | No | Filter by actor user ID |
| `created_after` | query | string (nullable) | No | Filter to activities created at or after this ISO 8601 datetime |
| `created_before` | query | string (nullable) | No | Filter to activities created at or before this ISO 8601 datetime |
| `limit` | query | integer (int64) (nullable) | No | Page size (default: 50, max: 200; values outside 1..=200 are clamped) |
| `offset` | query | integer (int64) (nullable) | No | Offset of the first activity (default: 0). When the response has `has_more`, repeat the request with `offset` = `next_offset`. |
| `team_key` | query | string (nullable) | No | Filter by team key (e.g. "TRA") |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/activities" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}/activities`

**Operation:** `list_issue_activities`

List all activity entries for an issue, ordered chronologically.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `issue_number` | query | integer (int64) (nullable) | No | Issue number within the team |
| `team_key` | query | string (nullable) | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues/TRA-35/activities" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

