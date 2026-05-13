import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  waitForWasm,
  createSecondUser,
  loginUser,
  generateEmail,
  cleanTestData,
  isPersonalMode,
  isSelfHostedMode,
  getDefaultWorkspaceId,
  db,
  walCheckpoint,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

test.describe('TC-038: RBAC - Admin Permissions', () => {
  const adminEmail = generateEmail();
  const adminName = 'RBAC Admin NonOwner';
  const adminPassword = 'AdminPass123!';

  test.beforeAll(async ({ browser }) => {
    if (!isSelfHostedMode()) return;
    cleanTestData();

    await createSecondUser(browser, {
      email: adminEmail,
      name: adminName,
      password: adminPassword,
      role: 'workspace_admin',
    });
  });

  test.afterAll(() => {
    const adminId = db.getUserByEmail(adminEmail);
    if (adminId) {
      db.exec(`DELETE FROM workspace_invitations WHERE invited_by_user_id = '${adminId}'`);
      db.exec(`DELETE FROM workspace_users WHERE user_id = '${adminId}' AND workspace_id = '${getDefaultWorkspaceId()}'`);
      db.exec(`DELETE FROM refresh_tokens WHERE user_id = '${adminId}'`);
      db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${adminId}'`);
      db.exec(`DELETE FROM users WHERE user_id = '${adminId}'`);
      walCheckpoint();
    }
    cleanTestData();
  });

  test('admin can invite a member via the UI', async ({ page }) => {
    const email = generateEmail();

    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await inviteButton.click();

    const modal = page.getByRole('heading', { name: 'Invite Team Member' });
    await expect(modal).toBeVisible({ timeout: 5_000 });

    const emailInput = page.locator('input[type="email"][placeholder="colleague@example.com"]');
    await emailInput.fill(email);

    const sendButton = page.getByRole('button', { name: 'Send Invitation' });
    await sendButton.click();

    await expect(modal).not.toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(email)).toBeVisible({ timeout: 10_000 });

    db.exec(`DELETE FROM workspace_invitations WHERE email = '${email}'`);
  });

  test('admin sees workspace settings page', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/workspace');

    const heading = page.locator('h2', { hasText: 'Workspace Settings' });
    await expect(heading).toBeVisible();
  });

  test('admin sees team management page with invite and transfer buttons', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await expect(inviteButton).toBeVisible();
  });

  test('non-owner admin cannot see Transfer Ownership button', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await loginUser(page, adminEmail, adminPassword);
      await page.goto(TEAM_URL);
      await waitForWasm(page);

      await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

      const transferBtn = page.locator('button[title="Transfer Ownership"]');
      await expect(transferBtn).not.toBeVisible({ timeout: 5_000 });
    } finally {
      await context.close();
    }
  });

  test('Transfer Ownership button is visible for workspace owner', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    const transferBtn = page.locator('button[title="Transfer Ownership"]');
    await expect(transferBtn).toBeVisible({ timeout: 10_000 });
  });
});
