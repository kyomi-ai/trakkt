# GitHub

## `GET /github/lookup/branch`

**Operation:** `lookup_branch`

Look up which issues are linked to a given branch name. Returns issue details including identifier, title, status, and description.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `branch` | query | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /github/lookup/commit`

**Operation:** `lookup_commit`

Look up which issues are linked to a given commit SHA. Uses prefix matching so abbreviated SHAs (7+ characters) work. Returns issue details including identifier, title, status, and description.

### Parameters

| Name | In | Type | Required | Description |
|------|----|------|----------|-------------|
| `sha` | query | string | Yes |  |

### Response

Returns `200 OK` on success with the result as JSON.

---

## `GET /issues/{identifier}/github-links`

**Operation:** `list_issue_github_links`

List all GitHub links (PRs, branches, commits) associated with an issue.

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

