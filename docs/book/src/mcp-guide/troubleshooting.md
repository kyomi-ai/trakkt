# Troubleshooting

This page covers common issues when connecting to and using the Trakkt MCP server.

## Connection Issues

### Cannot connect / connection refused

**Symptoms:** The MCP client fails to establish a connection, reports "connection refused", or times out.

**Causes and fixes:**

- **Wrong URL.** Verify your URL ends with `/mcp` (not `/mcp/sse` or `/api/v1/mcp`). The correct format is:
  ```
  https://your-trakkt-instance.com/mcp
  ```
- **Wrong port.** The default port is `8003` for local development. Check your Trakkt instance's `PORT` environment variable. See [Configuration](../getting-started/configuration.md) for details.
- **Server not running.** Confirm the Trakkt server is up and reachable. Try opening the base URL in a browser -- you should see the Trakkt UI.
- **Firewall or network.** If connecting to a remote instance, ensure the port is open and any firewalls, VPNs, or proxies allow the connection.
- **HTTPS required.** Production instances typically require HTTPS. Make sure your URL uses `https://` and that TLS is properly configured on the server or reverse proxy.

### SSE connection drops

**Symptoms:** The connection works initially but drops after some time.

**Causes and fixes:**

- **Session expired.** MCP sessions have a 24-hour TTL. After expiry, the client must reconnect. Most MCP clients handle this automatically.
- **Reverse proxy timeout.** If you run Trakkt behind nginx or another reverse proxy, ensure the proxy is configured for long-lived SSE connections. For nginx, set:
  ```nginx
  proxy_read_timeout 86400s;
  proxy_buffering off;
  ```

## Authentication Errors

### 401 Unauthorized

**Symptoms:** Every tool call returns an error, or the MCP client reports authentication failure.

**Causes and fixes:**

- **Missing token.** Ensure your configuration includes the `headers` section with the `Authorization` header:
  ```json
  "headers": {
    "Authorization": "Bearer trakkt-abc123..."
  }
  ```
- **Expired token.** If your API token has an expiry date, it may have expired. Create a new token in Settings > Security > API Tokens.
- **Revoked token.** Check Settings > Security > API Tokens to confirm your token is still active (not revoked).
- **Malformed header.** The header value must be exactly `Bearer <token>` with a single space between `Bearer` and the token. No trailing spaces or newlines.

### Personal mode -- no auth needed

If running in personal mode (`TRAKKT_MODE=personal`), you do not need any authentication. Remove the `headers` section from your configuration:

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

## Scope Errors

### Error code -32001: Token lacks required scope

**Symptoms:** A specific tool call fails with JSON-RPC error code `-32001` and a message about insufficient scope.

**Cause:** Your API token does not include the scope required by the tool you are calling.

**Fix:** Create a new API token with the needed scopes, or check the [Tools Reference](tools-reference.md) to see which scope each tool requires. For example:

- Calling `create_issue` requires `issues:write`
- Calling `add_comment` requires `comments:write`
- Calling `list_milestones` requires `projects:read`

If you are unsure which scopes your agent needs, create a token with all scopes and narrow it down later.

## JSON-RPC Errors

The MCP server returns standard JSON-RPC error codes:

| Code | Meaning | Common cause |
|------|---------|-------------|
| `-32600` | Invalid request | Malformed JSON-RPC message |
| `-32601` | Unknown method | Calling a method that does not exist (typo in method name) |
| `-32602` | Invalid params or domain error | Unknown tool name, missing/invalid parameters, or resource not found (e.g., issue identifier does not exist) |
| `-32001` | Auth or scope error | Missing/expired token, or token lacks required scope |
| `-32000` | Server unavailable | Rate limit exceeded or server temporarily unavailable |
| `-32603` | Internal error | Server-side error -- check server logs |

## Common Parameter Mistakes

### Forgetting `team_key`

`create_issue` assigns to the default team if you omit `team_key` and `team_id`. If you have multiple teams, always specify which team the issue belongs to:

```json
{
  "team_key": "ENG",
  "title": "My issue"
}
```

### Using UUIDs instead of identifiers

Tools like `get_issue`, `update_issue`, and `delete_issue` accept the human-readable issue identifier (e.g., `ENG-42`), not the internal UUID. Use the `issue_identifier` parameter:

```json
{
  "issue_identifier": "ENG-42"
}
```

### Hardcoding status IDs

Status IDs are UUIDs that vary between workspaces. Never hardcode them. Instead, call `list_statuses` first to discover the correct IDs for your workspace:

```
1. list_statuses → find the ID for "In Progress"
2. update_issue(issue_identifier: "ENG-42", status_id: "<discovered-id>")
```

### Priority values

Priority is a numeric value, not a string:

| Value | Priority |
|-------|----------|
| `0` | None |
| `1` | Urgent |
| `2` | High |
| `3` | Medium |
| `4` | Low |

### Clearing a field vs. omitting it

When calling `update_issue`:
- **Omit a field** to leave it unchanged
- **Set a field to `null`** to clear its value

For example, to remove an assignee: `{ "assignee": null }`. Simply omitting `assignee` leaves the current assignee in place.

## Getting Help

If you encounter an issue not covered here:

1. Check the Trakkt server logs (`RUST_LOG=debug` for verbose output)
2. Verify your setup against the [Setup](setup.md) page
3. Confirm your token scopes match the tools you are calling via the [Tools Reference](tools-reference.md)
