# GitHub

## `GET /github/lookup/branch`

**Operation:** `lookup_branch`

Look up which issues are linked to a given branch name. Returns issue details including identifier, title, status, and description.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `branch` | query | string | Yes | Branch name (exact match) |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/github/lookup/branch?branch=feature/my-branch" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /github/lookup/commit`

**Operation:** `lookup_commit`

Look up which issues are linked to a given commit SHA. Uses prefix matching so abbreviated SHAs (7+ characters) work. Returns issue details including identifier, title, status, and description.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `sha` | query | string | Yes | Commit SHA (full or abbreviated, minimum 7 characters). Prefix match is used. |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/github/lookup/commit?sha=abc1234" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}/github-links`

**Operation:** `list_issue_github_links`

List all GitHub links (PRs, branches, commits) associated with an issue.

**Scope:** `issues:read`

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `identifier` | path | string | Yes |  |
| `issue_number` | query | integer (int64) (nullable) | No | Issue number within the team. Required if issue_identifier is not provided |
| `team_key` | query | string (nullable) | No | Team key (e.g. 'TRA'). Required if issue_identifier is not provided |

### Example

```bash
curl "https://your-trakkt-instance.com/api/v1/issues/TRA-35/github-links" \
  -H "Authorization: Bearer <token>"
```

### Response

Returns `200 OK` on success with the result as JSON.

---

