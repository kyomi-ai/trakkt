# Milestones

## `DELETE /milestones/{id}`

**Operation:** `delete_milestone`

Delete a milestone. Issues linked to this milestone will be unlinked.

**Scope:** `projects:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/milestones/ms_abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /milestones/{id}`

**Operation:** `update_milestone`

Update fields on an existing milestone.

**Scope:** `projects:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | string (nullable) | No | New markdown description |
| `milestone_id` | string (nullable) | No | The milestone ID. Optional so REST can inject from path. |
| `name` | string (nullable) | No | New milestone name |
| `target_date` | string (nullable) | No | Target date in ISO 8601 format, or null to clear |

### Example

```bash
curl -X PATCH "https://your-trakkt-instance.com/api/v1/milestones/ms_abc123" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /projects/{id}/milestones`

**Operation:** `list_milestones`

List all milestones in a project.

**Scope:** `projects:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/projects/proj_abc123/milestones" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /projects/{id}/milestones`

**Operation:** `create_milestone`

Create a new milestone in a project.

**Scope:** `projects:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | string (nullable) | No | Markdown description of the milestone |
| `name` | string | Yes | Milestone name (required) |
| `project_id` | string (nullable) | No | The project ID to create the milestone in. Optional so REST can inject from path. |
| `target_date` | string (nullable) | No | Target date in ISO 8601 format (YYYY-MM-DD) |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/projects/proj_abc123/milestones" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "My Project"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

