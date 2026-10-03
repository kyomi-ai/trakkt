import { type Page, type Locator } from '@playwright/test';
import { test, expect } from '../../helpers/connect-fixture';

async function command(page: Page, terminal: Locator, text: string) {
  await terminal.focus();
  await page.keyboard.type(text);
  await page.keyboard.press('Enter');
}

test('browser controls a real shell PTY and restores sessions without an agent impersonation', async ({ page, context, connect }) => {
  const terminal = page.getByTestId('connect-terminal');
  const status = page.getByTestId('connect-status');
  const shell = page.getByRole('button', { name: 'New shell session', exact: true });
  const tabs = page.getByRole('tab');
  const frames: Array<Record<string, any>> = [];
  const browserErrors: string[] = [];
  let simulatingOutage = false;
  page.on('pageerror', error => browserErrors.push(error.message));
  page.on('console', message => {
    // Only Connect's own console errors; unrelated app logging is outside this
    // suite. Failed network attempts during the explicit outage are expected.
    if (message.type() === 'error' && message.text().includes('[trakkt-connect]') && !simulatingOutage) {
      browserErrors.push(message.text());
    }
  });
  await page.addInitScript(() => {
    const NativeWebSocket = window.WebSocket;
    const terminals: WebSocket[] = [];
    window.WebSocket = class extends NativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        if (String(url).includes('/ws/connect/terminal')) terminals.push(this);
      }
    };
    (window as any).__connectDropSocket = () => {
      const open = terminals.filter(socket => socket.readyState === NativeWebSocket.OPEN);
      for (const socket of open) socket.close();
      return open.length;
    };
  });
  page.on('websocket', socket => {
    if (!socket.url().includes('/ws/connect/terminal')) return;
    socket.on('framesent', event => {
      try { frames.push(JSON.parse(String(event.payload))); }
      catch { /* WebSocket control frames are not application messages. */ }
    });
  });

  await page.goto('/connect');
  await expect(status).toHaveText('Agent disconnected');
  await expect(shell).toBeDisabled();
  await expect(tabs).toHaveCount(0);
  await connect.startAgent();
  await expect(status).toHaveText('Agent connected');
  await expect(shell).toBeEnabled();

  // Deterministic production SpawnFailed: the real agent allows bash/sh only.
  await page.getByRole('button', { name: 'New Claude session', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('command not allowed');
  await expect(tabs).toHaveCount(1);
  const failedId = await tabs.first().getAttribute('data-session-id');
  expect(failedId).toBeTruthy();
  await page.getByRole('button', { name: `Close session ${failedId}`, exact: true }).click();
  await expect(tabs).toHaveCount(0);

  await shell.click();
  await expect(tabs).toHaveCount(1);
  const firstId = await tabs.first().getAttribute('data-session-id');
  expect(firstId).toBeTruthy();
  const first = page.locator(`[role="tab"][data-session-id="${firstId}"]`);
  await expect(first).toHaveAttribute('aria-selected', 'true');
  // Disable echo so the assertion cannot pass just by rendering our keystrokes.
  await command(page, terminal, "stty -echo; printf '\\n%s%s\\n' FIRST_ PTY_OUTPUT");
  await expect(terminal).toContainText('FIRST_PTY_OUTPUT');
  await command(page, terminal, "printf '\\n%s%s\\n' KEYBOARD_ ROUNDTRIP");
  await expect(terminal).toContainText('KEYBOARD_ROUNDTRIP');
  await terminal.focus();
  const inputMark = frames.length;
  await page.keyboard.press('Control+k');
  await expect.poll(() => frames.slice(inputMark).some(frame =>
    frame.type === 'session_input' && frame.session_id === firstId &&
    Buffer.from(frame.data, 'base64').equals(Buffer.from([0x0b])),
  )).toBe(true);
  await expect(page.getByPlaceholder('Type a command or search issues...')).toHaveCount(0);
  await expect(terminal).toBeFocused();
  // Clear any canonical input left by Ctrl+K when /bin/sh has no readline.
  await page.keyboard.press('Control+u');
  await command(page, terminal, "printf '\\n%s%s\\n' CONTROL_KEY_ ROUNDTRIP");
  await expect(terminal).toContainText('CONTROL_KEY_ROUNDTRIP');

  await terminal.evaluate(element => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', "printf '\\n%s%s\\n' PASTE_ ROUNDTRIP\n");
    element.dispatchEvent(new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true }));
  });
  await expect(terminal).toContainText('PASTE_ROUNDTRIP');

  const previousResizeCount = frames.filter(frame => frame.type === 'session_resize').length;
  await page.setViewportSize({ width: 980, height: 680 });
  await expect.poll(() => frames.filter(frame => frame.type === 'session_resize').length).toBeGreaterThan(previousResizeCount);
  const resize = frames.filter(frame => frame.type === 'session_resize' && frame.session_id === firstId).at(-1)!;
  // stty queries the kernel PTY size, verifying more than a browser frame send.
  await command(page, terminal, "printf '\\nPTY_SIZE='; stty size");
  await expect(terminal).toContainText(`PTY_SIZE=${resize.rows} ${resize.cols}`);

  await page.getByRole('button', { name: 'Enter fullscreen', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Exit fullscreen', exact: true })).toBeVisible();
  // Real CLI alternate-screen and SGR sequences through the PTY, relay, parser.
  await command(page, terminal, "printf '\\033[?1049h\\033[2J\\033[H\\033[31m%s%s\\033[0m' ALT_ SCREEN; sleep 2; printf '\\033[?1049l'");
  await expect(terminal).toContainText('ALT_SCREEN');
  await expect(terminal).toContainText('PASTE_ROUNDTRIP');
  await page.getByRole('button', { name: 'Exit fullscreen', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Enter fullscreen', exact: true })).toBeVisible();

  await shell.click();
  await expect(tabs).toHaveCount(2);
  const secondId = await tabs.nth(1).getAttribute('data-session-id');
  expect(secondId).toBeTruthy();
  const second = page.locator(`[role="tab"][data-session-id="${secondId}"]`);
  await command(page, terminal, "stty -echo; printf '\\n%s%s\\n' SECOND_ PTY_OUTPUT");
  await expect(terminal).toContainText('SECOND_PTY_OUTPUT');
  await first.click();
  await expect(terminal).toContainText('FIRST_PTY_OUTPUT');
  await expect(terminal).not.toContainText('SECOND_PTY_OUTPUT');
  await second.click();
  await expect(terminal).toContainText('SECOND_PTY_OUTPUT');

  // Chromium's offline emulation blocks new connections but leaves existing
  // WebSockets open (see the existing realtime harness). Close only this real
  // terminal socket while offline; the app performs its own normal reconnect.
  simulatingOutage = true;
  await context.setOffline(true);
  try {
    expect(await page.evaluate(() => (window as any).__connectDropSocket())).toBe(1);
    await expect(status).toHaveText('Agent disconnected');
    await expect(shell).toBeDisabled();
  } finally {
    await context.setOffline(false);
  }
  await expect(status).toHaveText('Agent connected');
  simulatingOutage = false;
  await expect(shell).toBeEnabled();
  await expect(tabs).toHaveCount(2);
  await first.click();
  await expect(terminal).toContainText('FIRST_PTY_OUTPUT');
  await second.click();
  await expect(terminal).toContainText('SECOND_PTY_OUTPUT');
  await command(page, terminal, "printf '\\n%s%s\\n' RECONNECT_ ROUNDTRIP");
  await expect(terminal).toContainText('RECONNECT_ROUNDTRIP');

  await page.reload();
  await expect(status).toHaveText('Agent connected');
  await expect(tabs).toHaveCount(2);
  await first.click();
  await expect(terminal).toContainText('FIRST_PTY_OUTPUT');
  await second.click();
  await expect(terminal).toContainText('SECOND_PTY_OUTPUT');
  await command(page, terminal, "trap '' TERM; printf '\\n%s%s\\n' TERM_ IGNORED");
  await expect(terminal).toContainText('TERM_IGNORED');
  await page.getByRole('button', { name: `Close session ${secondId}`, exact: true }).click();
  await expect.poll(() => frames.some(frame =>
    frame.type === 'session_kill' && frame.session_id === secondId && frame.force === true,
  )).toBe(true);
  await expect(second).toHaveCount(0);
  await page.reload();
  await expect(status).toHaveText('Agent connected');
  await expect(tabs).toHaveCount(1);
  await expect(second).toHaveCount(0);
  await expect(first).toHaveAttribute('aria-selected', 'true');
  await expect(terminal).toContainText('FIRST_PTY_OUTPUT');
  await command(page, terminal, 'exit 7');
  await expect(page.getByRole('alert')).toContainText('7');
  await expect(first).toBeVisible();
  await expect(terminal).toContainText('FIRST_PTY_OUTPUT');
  await page.getByRole('button', { name: `Close session ${firstId}`, exact: true }).click();
  await expect(tabs).toHaveCount(0);
  await connect.stopAgent();
  await expect(status).toHaveText('Agent disconnected');
  await expect(shell).toBeDisabled();
  expect(browserErrors, 'No uncaught browser errors or unexpected Connect console errors').toEqual([]);
});
