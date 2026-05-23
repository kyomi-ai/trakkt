# Activities

## `GET /activities`

**Operation:** `list_workspace_activities`

List activity entries across all teams in the workspace, ordered by most recent first.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `action_type` | query | any | No |  |
| `limit` | query | any | No |  |
| `offset` | query | any | No |  |
| `team_key` | query | any | No |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}/activities`

**Operation:** `list_issue_activities`

List all activity entries for an issue, ordered chronologically.

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

