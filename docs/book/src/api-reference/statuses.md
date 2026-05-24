# Statuses

## `GET /statuses`

**Operation:** `list_statuses`

List all statuses in the workspace, grouped by category (backlog, unstarted, started, completed, cancelled). Returns both global and optionally team-specific statuses.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `team_id` | query | string (nullable) | No | Team ID to include team-specific statuses |
| `team_key` | query | string (nullable) | No | Team key (e.g. 'TRA') as alternative to team_id |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/statuses" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

