# MCP Server

Trakkt includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server that lets AI agents interact with your issue tracker programmatically. The MCP server exposes the same 36 operations as the [REST API](../api-reference/README.md), so agents have full access to create issues, update statuses, search, and more.

## Quick Start

Get an AI agent connected to Trakkt in three steps:

1. **Create an API token** in Settings > Security > API Tokens. Select the scopes your agent needs and copy the token (it is shown only once).

2. **Add Trakkt to your MCP client** configuration:

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

3. **Verify the connection** by asking your agent to call `list_teams`. If it returns your teams, you are connected.

See the [Setup](setup.md) page for detailed configuration instructions.

## Protocol Details

The MCP server implements the [Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports#streamable-http) defined in the MCP specification (version 2025-03-26):

| Property | Value |
|----------|-------|
| Protocol version | `2025-03-26` |
| Server name | `trakkt-mcp` |
| Server version | `0.1.0` |
| Transport | JSON-RPC 2.0 over HTTP (Streamable HTTP) |
| POST endpoint | `/mcp` (JSON-RPC request/response) |
| GET endpoint | `/mcp` (SSE stream for server notifications) |
| DELETE endpoint | `/mcp` (session termination) |

### Capabilities

| Capability | Supported | Details |
|------------|-----------|---------|
| Tools | Yes | `listChanged: true` -- the server can notify clients when the tool list changes |
| Resources | Yes | `subscribe: false`, `listChanged: false` |

### Sessions

Each connection is assigned a session ID (UUIDv4) returned in the `mcp-session-id` response header. Sessions have a 24-hour TTL. After expiry, the client must reconnect to establish a new session.

## Guide Contents

- **[Setup](setup.md)** -- Configure Claude Code and other MCP clients to connect to Trakkt
- **[Authentication](authentication.md)** -- API tokens, scopes, OAuth, and personal mode
- **[Tools Reference](tools-reference.md)** -- All 36 tools organized by domain with required scopes
- **[Agent Workflows](workflows.md)** -- Practical patterns for common agent tasks
- **[Troubleshooting](troubleshooting.md)** -- Diagnose connection, auth, and parameter errors
