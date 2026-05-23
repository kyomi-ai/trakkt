# Labels

## `GET /labels`

**Operation:** `list_labels`

List all labels in the workspace, ordered alphabetically by name.

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /labels`

**Operation:** `create_label`

Create a new label in the workspace. Optionally scope it to a team by providing team_key or team_id.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `color` | string | Yes | Hex color code (e.g. '#FF5733' or 'FF5733') |
| `name` | string | Yes | Label name (must be unique within the workspace) |
| `team_id` | any | No | Team ID to scope the label to a specific team |
| `team_key` | any | No | Team key to scope the label to a specific team |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

