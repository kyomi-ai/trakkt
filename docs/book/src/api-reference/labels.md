# Labels

## `GET /labels`

**Operation:** `list_labels`

List labels in the workspace. Optionally filter by team_key or team_id to get workspace-level + team-scoped labels.

**Scope:** `labels:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `team_id` | query | string (nullable) | No | Filter by team ID — returns workspace-level + team-scoped labels |
| `team_key` | query | string (nullable) | No | Filter by team key (e.g. 'TRA') — resolved to team_id server-side |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/labels" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /labels`

**Operation:** `create_label`

Create a new label in the workspace. Optionally scope it to a team by providing team_key or team_id.

**Scope:** `labels:write`

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `color` | string | Yes | Hex color code (e.g. '#FF5733' or 'FF5733') |
| `name` | string | Yes | Label name (must be unique within the workspace) |
| `team_id` | string (nullable) | No | Team ID to scope the label to a specific team |
| `team_key` | string (nullable) | No | Team key to scope the label to a specific team |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/labels" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "My Project", "color": "#0D9488"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

