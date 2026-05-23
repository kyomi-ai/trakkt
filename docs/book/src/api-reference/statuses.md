# Statuses

## `GET /statuses`

**Operation:** `list_statuses`

List all statuses in the workspace, grouped by category (backlog, unstarted, started, completed, cancelled). Returns both global and optionally team-specific statuses.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `team_id` | query | any | No |  |
| `team_key` | query | any | No |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

