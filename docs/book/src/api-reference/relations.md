# Relations

## `GET /issues/{identifier}/relations`

**Operation:** `list_issue_relations`

List all relations for an issue (both directions — blocks and blocked-by).

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

## `POST /issues/{identifier}/relations`

**Operation:** `add_relation`

Add a relation between two issues. Supports 'blocks' (source blocks target), 'parent' (source is parent of target), and 'duplicate' (source is duplicate of target) relation types.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `relation_type` | string | Yes | Relation type: 'blocks', 'parent', or 'duplicate' |
| `source_issue` | any | No | Source issue identifier in 'TRA-35' format. For 'blocks': the blocker. For 'parent': the parent issue. For 'duplicate': the duplicate issue. Optional so REST can inject it from the path parameter. |
| `target_issue` | string | Yes | Target issue identifier in 'TRA-35' format. For 'blocks': the blocked issue. For 'parent': the child issue. For 'duplicate': the original issue. |

### Response

Returns `201 Created` on success with the created resource as JSON.

---

## `DELETE /relations/{id}`

**Operation:** `remove_relation`

Remove a relation between two issues by its relation ID.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `id` | path | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

