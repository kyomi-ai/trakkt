# Teams

## `GET /teams`

**Operation:** `list_teams`

List teams the authenticated user belongs to, ordered alphabetically by name.

### Response

Returns `200 OK` on success with the result as JSON.

---

## `PATCH /teams/{identifier}/settings`

**Operation:** `update_team_settings`

Update a team's settings including estimation scale, auto-archive, and other configuration. Provide team_key or team_id to identify the team.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `settings` | TeamSettings | Yes | New team settings (full replace) |
| `team_id` | any | No | Team ID |
| `team_key` | any | No | Team key (e.g. 'TRA') |

### Response

Returns `200 OK` on success with the result as JSON.

---

