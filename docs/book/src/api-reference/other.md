# Other

## `GET /releases`

**Operation:** `list_releases`

List all releases in the workspace, optionally filtered by team key.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `team_key` | query | string (nullable) | No | Filter by team key (e.g. 'TRA') |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/releases" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /releases`

**Operation:** `create_release`

Create a new release. Auto-links issues by looking up commit SHAs in github_links and stamps released_at on matched issues.

**Scope:** `issues:write`

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `commit_shas` | array<string> | Yes | List of full commit SHAs included in this release. Used to auto-link issues via github_links. |
| `notes` | string (nullable) | No | Release notes / changelog markdown |
| `previous_tag` | string (nullable) | No | Previous tag (for commit range context) |
| `tag_name` | string | Yes | Git tag name (e.g. 'v2026.05.20.1') |
| `team_key` | string | Yes | Team key this release belongs to (e.g. 'TRA') |
| `title` | string (nullable) | No | Optional human-readable title |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/releases" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"team_key": "TRA", "tag_name": "example-tag_name", "commit_shas": null}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `GET /releases/{id}`

**Operation:** `get_release`

Get a single release by ID, including linked issues with details.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/releases/abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /starred-issues`

**Operation:** `list_starred_issues`

List all issue IDs starred by the current user in the active workspace.

**Scope:** `issues:read`

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/starred-issues" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /unreleased-issues`

**Operation:** `list_unreleased_issues`

List issues that are completed/cancelled but not yet included in any release (completed_at IS NOT NULL, released_at IS NULL).

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `team_key` | query | string (nullable) | No | Filter by team key (e.g. 'TRA') |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/unreleased-issues" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

