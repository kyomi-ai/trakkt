# Projects

## `GET /projects`

**Operation:** `list_projects`

List all projects in the workspace.

**Scope:** `projects:read`

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/projects" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /projects`

**Operation:** `create_project`

Create a new project in the workspace.

**Scope:** `projects:write`

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `color` | string (nullable) | No | Hex color code (e.g. '#0D9488') |
| `description` | string (nullable) | No | Markdown description of the project |
| `icon` | string (nullable) | No | Icon identifier for the project |
| `lead_id` | string (nullable) | No | User ID to set as project lead |
| `name` | string | Yes | Project name (required) |
| `start_date` | string (nullable) | No | Start date in ISO 8601 format (YYYY-MM-DD) |
| `target_date` | string (nullable) | No | Target completion date in ISO 8601 format (YYYY-MM-DD) |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/projects" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "My Project"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /projects/{id}`

**Operation:** `delete_project`

Permanently delete a project and its milestones.

**Scope:** `projects:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/projects/proj_abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /projects/{id}`

**Operation:** `get_project`

Get a single project by its ID, including milestones.

**Scope:** `projects:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/projects/proj_abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /projects/{id}`

**Operation:** `update_project`

Update fields on an existing project. Only provided fields are changed.

**Scope:** `projects:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `color` | string (nullable) | No | New hex color code |
| `description` | string (nullable) | No | New markdown description |
| `icon` | string (nullable) | No | New icon identifier |
| `lead_id` | string (nullable) | No | User ID to set as project lead, or null to clear |
| `name` | string (nullable) | No | New project name |
| `project_id` | string (nullable) | No | The project ID. Optional so REST can inject it from the path parameter. |
| `start_date` | string (nullable) | No | Start date in ISO 8601 format, or null to clear |
| `status` | string (nullable) | No | New project status (e.g. 'planned', 'in_progress', 'paused', 'completed', 'cancelled') |
| `target_date` | string (nullable) | No | Target date in ISO 8601 format, or null to clear |

### Example

```bash
curl -X PATCH "https://your-trakkt-instance.com/api/v1/projects/proj_abc123" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

