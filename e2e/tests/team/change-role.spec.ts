import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  waitForWasm,
  seedUser,
  seedWorkspaceMember,
  cleanTestData,
  getDefaultWorkspaceId,
  walCheckpoint,
  db,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

test.describe('TC-018: Team - Change Role', () => {
  let memberId: string;
  let memberEmail: string;

  test.beforeAll(() => {
    cleanTestData();
    const member = seedUser({ name: 'Role Target' });
    memberId = member.userId;
    memberEmail = member.email;
    seedWorkspaceMember({
      workspaceId: getDefaultWorkspaceId(),
      userId: memberId,
      role: 'workspace_user',
    });
    walCheckpoint();
  });

  test.afterAll(() => {
    cleanTestData();
  });

  test('should promote member to admin, persist across refresh, then demote back', async ({
    page,
  }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Workspace Members' })).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(memberEmail)).toBeVisible({ timeout: 10_000 });

    const memberRow = page
      .locator('div.border.border-border.rounded-lg.bg-background')
      .filter({ hasText: memberEmail });

    const roleTrigger = memberRow.locator('button[aria-haspopup="listbox"]');
    await expect(roleTrigger).toBeVisible();
    await expect(roleTrigger).toContainText('User');

    // Promote to admin
    await roleTrigger.click();
    const adminOption = page.getByRole('option', { name: 'Admin' });
    await expect(adminOption).toBeVisible({ timeout: 5_000 });
    await adminOption.click();

    // Wait for the server function to complete by watching the role trigger update
    await expect(roleTrigger).toContainText('Admin', { timeout: 10_000 });

    // Verify persistence: reload and check role is still Admin
    await page.reload();
    await waitForWasm(page);

    await expect(page.getByText(memberEmail)).toBeVisible({ timeout: 10_000 });
    const refreshedRow = page
      .locator('div.border.border-border.rounded-lg.bg-background')
      .filter({ hasText: memberEmail });
    const refreshedTrigger = refreshedRow.locator('button[aria-haspopup="listbox"]');
    await expect(refreshedTrigger).toContainText('Admin');

    // Demote back to user
    await refreshedTrigger.click();
    const userOption = page.getByRole('option', { name: 'User' });
    await expect(userOption).toBeVisible({ timeout: 5_000 });
    await userOption.click();

    // Verify the demotion took effect via UI
    await expect(refreshedTrigger).toContainText('User', { timeout: 10_000 });
  });
});
