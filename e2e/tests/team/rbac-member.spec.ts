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

test.describe('TC-037: RBAC - Member Cannot Access Admin Operations', () => {
  const memberEmail = generateEmail();
  const memberName = 'RBAC Member';
  const memberPassword = 'MemberPass123!';

  test.beforeAll(async ({ browser }) => {
    if (!isSelfHostedMode()) return;
    cleanTestData();

    await createSecondUser(browser, {
      email: memberEmail,
      name: memberName,
      password: memberPassword,
      role: 'workspace_user',
    });
  });

  test.afterAll(() => {
    const memberId = db.getUserByEmail(memberEmail);
    if (memberId) {
      db.exec(`DELETE FROM workspace_users WHERE user_id = '${memberId}' AND workspace_id = '${getDefaultWorkspaceId()}'`);
      db.exec(`DELETE FROM refresh_tokens WHERE user_id = '${memberId}'`);
      db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${memberId}'`);
      db.exec(`DELETE FROM users WHERE user_id = '${memberId}'`);
      walCheckpoint();
    }
  });

  test('member has workspace_user role in DB', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    const memberId = db.getUserByEmail(memberEmail);
    expect(memberId).not.toBeNull();
    const role = db.getMemberRole(getDefaultWorkspaceId(), memberId!);
    expect(role).toBe('workspace_user');
  });

  test('team page denies access for non-admin member', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await loginUser(page, memberEmail, memberPassword);
      await page.goto('/settings/team');
      await waitForWasm(page);

      // Member should not see the Team Members heading or should see an access denied state
      const teamHeading = page.getByRole('heading', { name: 'Team Members' });
      const accessDenied = page.getByText(/access denied|not authorized|forbidden/i);
      const redirectedToLogin = page.url().includes('/login');
      const redirectedAway = !page.url().includes('/settings/team');

      const denied = await accessDenied.isVisible({ timeout: 5_000 }).catch(() => false);
      const teamVisible = await teamHeading.isVisible({ timeout: 2_000 }).catch(() => false);

      expect(denied || redirectedToLogin || redirectedAway || !teamVisible).toBe(true);
    } finally {
      await context.close();
    }
  });

  test('workspace settings page renders for admin user', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/workspace');

    const heading = page.locator('h2', { hasText: 'Workspace Settings' });
    await expect(heading).toBeVisible();
  });

  test('team page renders for admin user', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/team');

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });
  });

  test('invite button is visible for admin user', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/team');

    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await expect(inviteButton).toBeVisible({ timeout: 10_000 });
  });
});
