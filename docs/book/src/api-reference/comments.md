# Comments

## `POST /issues/{identifier}/comments`

**Operation:** `add_comment`

Add a comment to an issue. Comments support markdown formatting.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `body` | string | Yes | Markdown body of the comment (required) |
| `issue_identifier` | any | No | Issue identifier in 'TRA-35' format |
| `issue_number` | any | No | Issue number within the team. Required if issue_identifier is not provided |
| `parent_id` | any | No | Parent comment ID for threaded replies |
| `team_key` | any | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

