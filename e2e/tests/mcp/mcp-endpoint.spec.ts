// SPDX-License-Identifier: AGPL-3.0-or-later

import { test, expect } from '@playwright/test';
import { apiRequest } from '../../helpers/test-helpers';

const JSONRPC = '2.0';

function rpcRequest(method: string, id: number, params?: unknown) {
  return { jsonrpc: JSONRPC, id, method, ...(params !== undefined && { params }) };
}

test.describe('TC-022: MCP Endpoint — JSON-RPC over HTTP', () => {

  test('initialize returns server info, capabilities, and session ID header', async ({ page }) => {
    const { status, body, headers } = await apiRequest(page, 'POST', '/mcp', rpcRequest('initialize', 1, {
      protocolVersion: '2025-03-26',
      clientInfo: { name: 'playwright-test', version: '1.0.0' },
      capabilities: {},
    }));

    expect(status).toBe(200);

    const result = (body as any).result;
    expect(result.protocolVersion).toBe('2025-03-26');
    expect(result.serverInfo.name).toBe('tane-mcp');
    expect(result.serverInfo.version).toBe('0.1.0');
    expect(result.capabilities.tools).toBeDefined();
    expect(result.capabilities.tools.listChanged).toBe(true);

    const sessionId = headers['mcp-session-id'];
    expect(sessionId).toBeTruthy();
  });

  test('tools/list returns the hello tool', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('tools/list', 2));

    expect(status).toBe(200);

    const tools = (body as any).result.tools;
    expect(Array.isArray(tools)).toBe(true);
    expect(tools.length).toBeGreaterThanOrEqual(1);

    const hello = tools.find((t: any) => t.name === 'hello');
    expect(hello).toBeDefined();
    expect(hello.description).toBeTruthy();
    expect(hello.inputSchema).toBeDefined();
    expect(hello.inputSchema.type).toBe('object');
  });

  test('tools/call with hello tool returns greeting', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('tools/call', 3, {
      name: 'hello',
      arguments: { name: 'Playwright' },
    }));

    expect(status).toBe(200);

    const content = (body as any).result.content;
    expect(Array.isArray(content)).toBe(true);
    expect(content[0].type).toBe('text');
    expect(content[0].text).toContain('Hello, Playwright!');
    expect(content[0].text).toContain('Tane MCP server is running');
  });

  test('tools/call with hello tool defaults name to "world"', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('tools/call', 4, {
      name: 'hello',
      arguments: {},
    }));

    expect(status).toBe(200);
    expect((body as any).result.content[0].text).toContain('Hello, world!');
  });

  test('tools/call with unknown tool returns error', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('tools/call', 5, {
      name: 'nonexistent',
      arguments: {},
    }));

    expect(status).toBe(200);

    const error = (body as any).error;
    expect(error).toBeDefined();
    expect(error.code).toBe(-32602);
    expect(error.message).toContain('Unknown tool');
  });

  test('unknown method returns -32601 Method not found', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('bogus/method', 6));

    expect(status).toBe(200);

    const error = (body as any).error;
    expect(error.code).toBe(-32601);
    expect(error.message).toContain('Method not found');
  });

  test('ping returns empty object', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('ping', 7));

    expect(status).toBe(200);
    expect((body as any).result).toEqual({});
  });

  test('notifications/initialized returns 202 Accepted', async ({ page }) => {
    const { status } = await apiRequest(page, 'POST', '/mcp', rpcRequest('notifications/initialized', 8));

    expect(status).toBe(202);
  });

  test('resources/list returns empty array', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'POST', '/mcp', rpcRequest('resources/list', 9));

    expect(status).toBe(200);
    expect((body as any).result.resources).toEqual([]);
  });

  test('GET /mcp returns SSE ping', async ({ page }) => {
    const { status, body } = await apiRequest(page, 'GET', '/mcp');

    expect(status).toBe(200);
    expect(body).toContain('event: ping');
  });

  test('DELETE /mcp returns 204 No Content', async ({ page }) => {
    // First initialize to get a session ID
    const init = await apiRequest(page, 'POST', '/mcp', rpcRequest('initialize', 10, {
      protocolVersion: '2025-03-26',
      clientInfo: { name: 'playwright-test', version: '1.0.0' },
      capabilities: {},
    }));
    const sessionId = init.headers['mcp-session-id'];

    const { status } = await apiRequest(page, 'DELETE', '/mcp', undefined, {
      'mcp-session-id': sessionId,
    });

    expect(status).toBe(204);
  });

  test('full MCP lifecycle: initialize → tools/list → tools/call → delete', async ({ page }) => {
    // Step 1: Initialize
    const init = await apiRequest(page, 'POST', '/mcp', rpcRequest('initialize', 100, {
      protocolVersion: '2025-03-26',
      clientInfo: { name: 'playwright-lifecycle', version: '1.0.0' },
      capabilities: {},
    }));
    expect(init.status).toBe(200);
    const sessionId = init.headers['mcp-session-id'];
    expect(sessionId).toBeTruthy();

    // Step 2: Send initialized notification
    const notif = await apiRequest(page, 'POST', '/mcp', rpcRequest('notifications/initialized', 101), {
      'mcp-session-id': sessionId,
    });
    expect(notif.status).toBe(202);

    // Step 3: List tools
    const list = await apiRequest(page, 'POST', '/mcp', rpcRequest('tools/list', 102), {
      'mcp-session-id': sessionId,
    });
    expect(list.status).toBe(200);
    const tools = (list.body as any).result.tools;
    expect(tools.some((t: any) => t.name === 'hello')).toBe(true);

    // Step 4: Call the hello tool
    const call = await apiRequest(page, 'POST', '/mcp', rpcRequest('tools/call', 103, {
      name: 'hello',
      arguments: { name: 'E2E' },
    }), {
      'mcp-session-id': sessionId,
    });
    expect(call.status).toBe(200);
    expect((call.body as any).result.content[0].text).toContain('Hello, E2E!');

    // Step 5: Terminate session
    const del = await apiRequest(page, 'DELETE', '/mcp', undefined, {
      'mcp-session-id': sessionId,
    });
    expect(del.status).toBe(204);
  });
});
