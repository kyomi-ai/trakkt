# Setup

This page covers how to connect MCP clients to your Trakkt instance. Before you begin, you will need a Trakkt API token -- see [Authentication](authentication.md) for how to create one.

## Claude Code

Claude Code can connect to Trakkt's MCP server using the SSE transport. There are two levels of configuration:

### Project-level (recommended)

Add Trakkt to your project's `.claude/settings.json` so every team member on the project gets it automatically:

```json
{
  "mcpServers": {
    "trakkt": {
      "type": "sse",
      "url": "https://your-trakkt-instance.com/mcp",
      "headers": {
        "Authorization": "Bearer trakkt-abc123..."
      }
    }
  }
}
```

### User-level

To make Trakkt available across all projects, add it to your user-level config at `~/.claude.json`:

```json
{
  "mcpServers": {
    "trakkt": {
      "type": "sse",
      "url": "https://your-trakkt-instance.com/mcp",
      "headers": {
        "Authorization": "Bearer trakkt-abc123..."
      }
    }
  }
}
```

> **Note:** The `type` must be `"sse"`. Claude Code connects via SSE transport, and the Trakkt server handles the protocol negotiation.

## Other MCP Clients

Any MCP-compatible client can connect to Trakkt. Use these connection parameters:

| Parameter | Value |
|-----------|-------|
| Transport | SSE |
| URL | `https://your-trakkt-instance.com/mcp` |
| Authentication | `Authorization: Bearer <your-token>` header |

The server implements the full MCP tools protocol with typed JSON Schema parameters, so any compliant client will discover available tools automatically via the `tools/list` method.

## Local Development

When running Trakkt locally, use your local URL. The default port is `8003` unless you have configured a different `PORT`:

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

In [personal mode](../getting-started/configuration.md), authentication is not required -- the server automatically uses the local user context.

## Verifying Your Connection

After configuring your client, verify the connection is working by asking the agent to call `list_teams`. A successful response looks like this:

```
Agent: I found the following teams in your workspace:
- ENG (Engineering)
- DES (Design)
```

If the connection fails, see [Troubleshooting](troubleshooting.md) for common issues.
