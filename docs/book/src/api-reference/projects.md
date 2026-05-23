# Projects

## `GET /projects`

**Operation:** `list_projects`

List all projects in the workspace.

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /projects`

**Operation:** `create_project`

Create a new project in the workspace.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `color` | any | No | Hex color code (e.g. '#0D9488') |
| `description` | any | No | Markdown description of the project |
| `icon` | any | No | Icon identifier for the project |
| `lead_id` | any | No | User ID to set as project lead |
| `name` | string | Yes | Project name (required) |
| `start_date` | any | No | Start date in ISO 8601 format (YYYY-MM-DD) |
| `target_date` | any | No | Target completion date in ISO 8601 format (YYYY-MM-DD) |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /projects/{id}`

**Operation:** `delete_project`

Permanently delete a project and its milestones.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /projects/{id}`

**Operation:** `get_project`

Get a single project by its ID, including milestones.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |
| `project_id` | query | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /projects/{id}`

**Operation:** `update_project`

Update fields on an existing project. Only provided fields are changed.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `color` | any | No | New hex color code |
| `description` | any | No | New markdown description |
| `icon` | any | No | New icon identifier |
| `lead_id` | any | No | User ID to set as project lead, or null to clear |
| `name` | any | No | New project name |
| `project_id` | any | No | The project ID. Optional so REST can inject it from the path parameter. |
| `start_date` | any | No | Start date in ISO 8601 format, or null to clear |
| `status` | any | No | New project status (e.g. 'planned', 'in_progress', 'paused', 'completed', 'cancelled') |
| `target_date` | any | No | Target date in ISO 8601 format, or null to clear |

### Response

Returns `200 OK` on success with the result as JSON.

---

