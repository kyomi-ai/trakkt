# Relations

## `GET /issues/{identifier}/relations`

**Operation:** `list_issue_relations`

List all relations for an issue (both directions — blocks and blocked-by).

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `issue_number` | query | integer (int64) (nullable) | No | Issue number within the team. Required if issue_identifier is not provided |
| `team_key` | query | string (nullable) | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues/TRA-35/relations" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `POST /issues/{identifier}/relations`

**Operation:** `add_relation`

Add a relation between two issues. Supports 'blocks' (source blocks target), 'parent' (source is parent of target), and 'duplicate' (source is duplicate of target) relation types.

**Scope:** `issues:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `relation_type` | string | Yes | Relation type: 'blocks', 'parent', or 'duplicate' |
| `source_issue` | string (nullable) | No | Source issue identifier in 'TRA-35' format. For 'blocks': the blocker. For 'parent': the parent issue. For 'duplicate': the duplicate issue. Optional so REST can inject it from the path parameter. |
| `target_issue` | string | Yes | Target issue identifier in 'TRA-35' format. For 'blocks': the blocked issue. For 'parent': the child issue. For 'duplicate': the original issue. |

### Example

```bash
curl -X POST "https://your-trakkt-instance.com/api/v1/issues/TRA-35/relations" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"target_issue": "TRA-42", "relation_type": "blocks"}'
```

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /relations/{id}`

**Operation:** `remove_relation`

Remove a relation between two issues by its relation ID.

**Scope:** `issues:write`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Example

```bash
curl -X DELETE "https://your-trakkt-instance.com/api/v1/relations/rel_abc123" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

