# Attachments

## `GET /attachments`

**Operation:** `list_attachments`

List all attachments in the workspace.

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /attachments`

**Operation:** `upload_attachment`

Upload a file attachment. Returns the attachment metadata including download URL.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `content_base64` | string | Yes | Base64-encoded file content |
| `content_type` | string | Yes | MIME content type (e.g. "image/png") |
| `filename` | string | Yes | Original filename (e.g. "screenshot.png") |
| `issue_id` | any | No | Optional issue ID to auto-link the attachment to an issue after upload. |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /attachments/{attachment_id}`

**Operation:** `delete_attachment`

Delete an attachment by ID. Only the original uploader can delete.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `attachment_id` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /attachments/{attachment_id}/download`

**Operation:** `download_attachment`

Download an attachment file by ID.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `attachment_id` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}/attachments`

**Operation:** `list_issue_attachments`

List all attachments linked to an issue.

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

## `POST /issues/{identifier}/attachments`

**Operation:** `attach_to_issue`

Attach an existing attachment to an issue. Idempotent — re-attaching is a no-op.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `attachment_id` | string | Yes | The attachment ID to link to the issue |
| `issue_identifier` | any | No | Issue identifier in 'TRA-35' format |
| `issue_number` | any | No | Issue number within the team. Required if issue_identifier is not provided |
| `team_key` | any | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /issues/{identifier}/attachments/{attachment_id}`

**Operation:** `detach_from_issue`

Detach an attachment from an issue. Does not delete the attachment itself.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `attachment_id` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

