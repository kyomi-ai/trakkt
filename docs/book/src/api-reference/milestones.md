# Milestones

## `DELETE /milestones/{id}`

**Operation:** `delete_milestone`

Delete a milestone. Issues linked to this milestone will be unlinked.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /milestones/{id}`

**Operation:** `update_milestone`

Update fields on an existing milestone.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | any | No | New markdown description |
| `milestone_id` | any | No | The milestone ID. Optional so REST can inject from path. |
| `name` | any | No | New milestone name |
| `target_date` | any | No | Target date in ISO 8601 format, or null to clear |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /projects/{id}/milestones`

**Operation:** `list_milestones`

List all milestones in a project.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |
| `project_id` | query | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /projects/{id}/milestones`

**Operation:** `create_milestone`

Create a new milestone in a project.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | any | No | Markdown description of the milestone |
| `name` | string | Yes | Milestone name (required) |
| `project_id` | any | No | The project ID to create the milestone in. Optional so REST can inject from path. |
| `target_date` | any | No | Target date in ISO 8601 format (YYYY-MM-DD) |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

