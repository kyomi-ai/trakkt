# Authentication

The MCP server supports two authentication methods, tried in order. In personal mode, authentication is bypassed entirely.

## API Tokens

API tokens are the simplest way to authenticate MCP clients. They are scoped, long-lived, and do not require an OAuth flow.

### Creating a Token

1. Open **Settings > Security > API Tokens** in the Trakkt UI
2. Click **Create Token**
3. Enter a descriptive name (e.g., "Claude Code - project-name")
4. Select the scopes your agent needs (see the table below)
5. Optionally set an expiry in days, or leave it as "Never" for a non-expiring token
6. Click **Create**
7. Copy the token immediately -- it is shown only once and cannot be retrieved later

The token has the format `trakkt-<64-hex-characters>`. Only a SHA-256 hash is stored on the server; the plaintext is never persisted.

### Using the Token

Pass the token in the `Authorization` header:

```
Authorization: Bearer trakkt-abc123...
```

### Scope Reference

Each API operation requires a specific scope. When creating an API token, select only the scopes your agent needs.

| Scope | Grants access to |
|-------|-----------------|
| `issues:read` | List issues, search issues, get issue details, list relations, list activities, list GitHub links, lookup commits/branches |
| `issues:write` | Create, update, and delete issues; add/remove relations |
| `comments:write` | Add comments to issues |
| `labels:read` | List labels |
| `labels:write` | Create labels |
| `teams:read` | List teams |
| `teams:write` | Update team settings |
| `projects:read` | List/get projects and milestones |
| `projects:write` | Create, update, and delete projects and milestones |
| `attachments:read` | List and download attachments |
| `attachments:write` | Upload, delete, attach, and detach attachments |

For a read-only agent that browses issues and projects, select: `issues:read`, `labels:read`, `teams:read`, `projects:read`.

For a full-access agent that can create and modify issues, select all scopes.

See the [Tools Reference](tools-reference.md) for which scope each tool requires.

## JWT (OAuth 2.0)

MCP clients that support OAuth 2.0 can authenticate via the standard authorization code flow with PKCE. This is the method Claude Code uses when it performs OAuth discovery automatically.

JWT-authenticated users have full access (equivalent to all scopes).

### OAuth Endpoints

| Endpoint | Purpose |
|----------|---------|
| `/.well-known/oauth-authorization-server` | OAuth 2.0 server metadata discovery |
| `/.well-known/oauth-protected-resource` | Protected resource metadata |
| `/api/v1/oauth/register` | Dynamic client registration |
| `/api/v1/oauth/authorize` | Authorization endpoint |
| `/api/v1/oauth/token` | Token exchange |

### Flow Details

- **Grant types:** `authorization_code`, `refresh_token`
- **PKCE:** Required, method `S256`
- **Token format:** JWT signed with HS256
- JWT access tokens include `workspace_id` and `user_id` claims
- OAuth agent tokens include a `client_name` claim, which Trakkt uses to attribute actions (shown as "via ClientName" in the activity log)

Most users do not need to implement the OAuth flow manually -- MCP clients handle it automatically when they detect the `.well-known` metadata.

## Personal Mode

When Trakkt runs in [personal mode](../getting-started/configuration.md) (`TRAKKT_MODE=personal`), authentication is bypassed entirely. The MCP server uses the auto-provisioned local user context, so no token or OAuth flow is needed.

This is the simplest setup for single-user local use:

```json
{
  "mcpServers": {
    "trakkt": {
      "type": "sse",
      "url": "http://localhost:8003/mcp"
    }
  }
}
```

## Further Reading

For general API authentication details (login endpoint, token expiry, security recommendations), see the [REST API Authentication](../api-reference/authentication.md) page.
