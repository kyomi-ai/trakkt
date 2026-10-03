# Attachments

## `GET /attachments`

**Operation:** `list_attachments`

List all attachments in the workspace.

**Scope:** `attachments:read`

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/attachments" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /attachments`

**Operation:** `upload_attachment`

Upload a file attachment. Returns the attachment metadata including download URL.

**Scope:** `attachments:write`

### Request Body

This endpoint accepts `multipart/form-data` with a `file` field containing the file to upload.

Optional query parameters: `issue_id` (attach the file to an issue after upload).

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/attachments" \
  -H "Authorization: Bearer <token>" \
  -F "file=@/path/to/document.pdf"
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /attachments/{attachment_id}`

**Operation:** `delete_attachment`

Delete an attachment by ID. Only the original uploader can delete.

**Scope:** `attachments:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `attachment_id` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/attachments/att_abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /attachments/{attachment_id}/download`

**Operation:** `download_attachment`

Download an attachment file by ID.

**Scope:** `attachments:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `attachment_id` | path | string | Yes |  |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/attachments/att_abc123/download" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}/attachments`

**Operation:** `list_issue_attachments`

List all attachments linked to an issue.

**Scope:** `attachments:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `issue_number` | query | integer (int64) (nullable) | No | Issue number within the team. Required if issue_identifier is not provided |
| `team_key` | query | string (nullable) | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues/TRA-35/attachments" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /issues/{identifier}/attachments`

**Operation:** `attach_to_issue`

Attach an existing attachment to an issue. Idempotent — re-attaching is a no-op.

**Scope:** `attachments:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `attachment_id` | string | Yes | The attachment ID to link to the issue |
| `issue_identifier` | string (nullable) | No | Issue identifier in 'TRA-35' format |
| `issue_number` | integer (int64) (nullable) | No | Issue number within the team. Required if issue_identifier is not provided |
| `team_key` | string (nullable) | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/issues/TRA-35/attachments" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"attachment_id": "example-attachment_id"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /issues/{identifier}/attachments/{attachment_id}`

**Operation:** `detach_from_issue`

Detach an attachment from an issue. Does not delete the attachment itself.

**Scope:** `attachments:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `attachment_id` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/issues/TRA-35/attachments/att_abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

