# Comments

## `POST /issues/{identifier}/comments`

**Operation:** `add_comment`

Add a comment to an issue. Comments support markdown formatting.

**Scope:** `comments:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `body` | string | Yes | Markdown body of the comment (required) |
| `issue_identifier` | string (nullable) | No | Issue identifier in 'TRA-35' format |
| `issue_number` | integer (int64) (nullable) | No | Issue number within the team. Required if issue_identifier is not provided |
| `parent_id` | string (nullable) | No | Parent comment ID for threaded replies |
| `team_key` | string (nullable) | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/issues/TRA-35/comments" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"body": "This is the content"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

