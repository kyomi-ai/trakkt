import { test, expect, type Page, type Browser } from '@playwright/test';
import { gotoAuthenticated, waitForWasm, loginUser, isPersonalMode, isSelfHostedMode } from '../../helpers/test-helpers';

const SECURITY_URL = '/settings/security';

async function navigateToSecurity(page: Page) {
  await gotoAuthenticated(page, SECURITY_URL);
}

async function createSecondSession(browser: Browser): Promise<void> {
  const secondContext = await browser.newContext();
  const secondPage = await secondContext.newPage();
  try {
    if (isSelfHostedMode()) {
      await loginUser(secondPage, 'local@localhost', 'TestPassword1234');
    } else {
      await secondPage.goto('/');
      await waitForWasm(secondPage);
      await secondPage.waitForTimeout(3000);
    }
  } finally {
    await secondContext.close();
  }
}

test.describe('TC-014: Security - Session Management', () => {
  test('should show the Active Sessions card', async ({ page }) => {
    await navigateToSecurity(page);

    // Use exact match to avoid matching "No active sessions found" and similar text
    await expect(page.getByText('Active Sessions', { exact: true })).toBeVisible();
    await expect(page.getByText('Manage your active login sessions across different devices.')).toBeVisible();
  });

  test('should display the current session with "Current" badge', async ({ page }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await navigateToSecurity(page);

    // The sessions table should load and show at least one session (the current one)
    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    await expect(page.getByText('Current')).toBeVisible({ timeout: 10000 });
  });

  test('should not show revoke button on the current session', async ({ page }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await navigateToSecurity(page);

    // Wait for sessions to load
    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    // Find the row with "Current" badge
    const currentRow = page.locator('tr').filter({ hasText: 'Current' });
    await expect(currentRow).toBeVisible();

    // The current session row should NOT have a disconnect button
    const disconnectBtn = currentRow.locator('button[title="Disconnect this session"]');
    await expect(disconnectBtn).not.toBeVisible();
  });

  test('should refresh the sessions list', async ({ page }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await navigateToSecurity(page);

    await expect(page.locator('table').last()).toBeVisible({ timeout: 10000 });

    // Click the refresh button (the one near "Active Sessions")
    const refreshBtn = page.locator('button[title="Refresh sessions"]');
    await expect(refreshBtn).toBeVisible();
    await refreshBtn.click();

    // Should still show the sessions after refresh
    await expect(page.getByText('Current')).toBeVisible({ timeout: 10000 });
  });

  test('should show security tip when sessions exist', async ({ page }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await navigateToSecurity(page);

    await expect(page.locator('table').last()).toBeVisible({ timeout: 10000 });

    await expect(page.getByText(/Security tip/i)).toBeVisible();
    await expect(page.getByText(/unfamiliar sessions/i)).toBeVisible();
  });

  test('should show session details in table columns', async ({ page }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await navigateToSecurity(page);

    // Wait for sessions table to load
    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    // Verify table headers exist
    await expect(page.getByText('Device', { exact: false }).first()).toBeVisible();
    await expect(page.getByText('Location')).toBeVisible();
    await expect(page.getByText('Last Active')).toBeVisible();
    await expect(page.getByText('Created')).toBeVisible();
    await expect(page.getByText('Actions')).toBeVisible();
  });

  test('should revoke a secondary session', async ({ page, browser }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await createSecondSession(browser);

    await navigateToSecurity(page);

    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    const sessionRows = sessionsTable.locator('tbody tr');
    const rowCount = await sessionRows.count();
    expect(rowCount).toBeGreaterThan(1);

    const nonCurrentRow = sessionRows.filter({ hasNot: page.locator('text=Current') }).first();
    const disconnectBtn = nonCurrentRow.locator('button[title="Disconnect this session"]');
    await expect(disconnectBtn).toBeVisible();
    await disconnectBtn.click();

    await expect(page.getByText('Disconnect Session?')).toBeVisible({ timeout: 5000 });
    await page.getByRole('button', { name: 'Disconnect' }).click();

    await page.waitForTimeout(2000);
    const newRowCount = await sessionRows.count();
    expect(newRowCount).toBeLessThan(rowCount);
  });

  test('should show "Log Out from All Devices" when multiple sessions exist', async ({
    page,
    browser,
  }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await createSecondSession(browser);

    await navigateToSecurity(page);

    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    const sessionRows = sessionsTable.locator('tbody tr');
    const rowCount = await sessionRows.count();
    expect(rowCount).toBeGreaterThan(1);

    await expect(page.getByText('Sign Out All Devices')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Log Out from All Devices' })).toBeVisible();
  });

  test('should confirm before logging out all devices', async ({ page, browser }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await createSecondSession(browser);

    await navigateToSecurity(page);

    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    const sessionRows = sessionsTable.locator('tbody tr');
    const rowCount = await sessionRows.count();
    expect(rowCount).toBeGreaterThan(1);

    await page.getByRole('button', { name: 'Log Out from All Devices' }).click();

    await expect(page.getByText('Log Out From All Devices?')).toBeVisible({ timeout: 5000 });
    await expect(page.getByText(/log out from all devices/i)).toBeVisible();

    const cancelBtn = page.getByRole('button', { name: 'Cancel' });
    await cancelBtn.click();

    await expect(page.getByText('Active Sessions')).toBeVisible();
  });

  test('should redirect to login after logging out all sessions', async ({ page, browser }) => {
    test.skip(isPersonalMode(), 'Personal mode auto-authenticates without creating sessions');

    await createSecondSession(browser);

    await navigateToSecurity(page);

    const sessionsTable = page.locator('table').last();
    await expect(sessionsTable).toBeVisible({ timeout: 10000 });

    const sessionRows = sessionsTable.locator('tbody tr');
    const rowCount = await sessionRows.count();
    expect(rowCount).toBeGreaterThan(1);

    await page.getByRole('button', { name: 'Log Out from All Devices' }).click();
    await expect(page.getByText('Log Out From All Devices?')).toBeVisible({ timeout: 5000 });

    await page.getByRole('button', { name: 'Log Out All' }).click();

    await page.waitForURL('**/login**', { timeout: 15000 });
    await expect(page.locator('#login-email')).toBeVisible({ timeout: 10000 });
  });
});
