# Teams

## `GET /teams`

**Operation:** `list_teams`

List teams the authenticated user belongs to, ordered alphabetically by name.

**Scope:** `teams:read`

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/teams" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /teams/{identifier}/settings`

**Operation:** `update_team_settings`

Update a team's settings including estimation scale, auto-archive, and other configuration. Provide team_key or team_id to identify the team.

**Scope:** `teams:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `settings` | TeamSettings | Yes | New team settings (full replace) |
| `team_id` | string (nullable) | No | Team ID |
| `team_key` | string (nullable) | No | Team key (e.g. 'TRA') |

### Example

```bash
curl -X PATCH "https://your-trakkt-instance.com/api/v1/teams/TRA/settings" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"settings": {}}'
```

### Response

Returns `200 OK` on success with the result as JSON.

---

