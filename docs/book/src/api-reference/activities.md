# Activities

## `GET /activities`

**Operation:** `list_workspace_activities`

List activity entries across all teams in the workspace, ordered by most recent first.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `action_source` | query | string (nullable) | No | Filter by action source: "user", "agent", or "api" |
| `action_type` | query | string (nullable) | No | Filter by action type (e.g. "status_changed", "comment_added") |
| `actor_id` | query | string (nullable) | No | Filter by actor user ID |
| `created_after` | query | string (nullable) | No | Filter to activities created at or after this ISO 8601 datetime |
| `created_before` | query | string (nullable) | No | Filter to activities created at or before this ISO 8601 datetime |
| `limit` | query | integer (int64) (nullable) | No | Maximum number of activities to return (default: 50, max: 200) |
| `offset` | query | integer (int64) (nullable) | No | Offset for pagination |
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

