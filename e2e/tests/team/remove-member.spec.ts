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

test.describe('TC-017: Team - Remove Member', () => {
  let memberId: string;
  let memberEmail: string;

  test.beforeAll(() => {
    cleanTestData();
    const member = seedUser({ name: 'Remove Target' });
    memberId = member.userId;
    memberEmail = member.email;
    seedWorkspaceMember({
      workspaceId: getDefaultWorkspaceId(),
      userId: member.userId,
      role: 'workspace_user',
    });
    walCheckpoint();
  });

  test.afterAll(() => {
    cleanTestData();
  });

  test('should remove a seeded member after confirmation', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Workspace Members' })).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(memberEmail)).toBeVisible({ timeout: 10_000 });

    // The remove button is in the same row as the member email.
    // Each member row is a border-bordered div. Find the row containing
    // the member email and click its trash/remove button.
    const memberRow = page
      .locator('div.border.border-border.rounded-lg.bg-background')
      .filter({ hasText: memberEmail });
    const removeButton = memberRow.getByRole('button', { name: /Remove member/i });
    await expect(removeButton).toBeVisible();
    await removeButton.click();

    // ConfirmDialog appears
    const confirmDialog = page.getByRole('alertdialog');
    await expect(confirmDialog).toBeVisible({ timeout: 5_000 });

    // Click the confirm button (second button in the dialog footer)
    const confirmButton = confirmDialog.getByRole('button').nth(1);
    await expect(confirmButton).toBeVisible();
    await confirmButton.click();

    // Wait for the dialog to close
    await expect(confirmDialog).not.toBeVisible({ timeout: 5_000 });

    // Verify removal completed at the DB level
    await expect(async () => {
      const members = db.getWorkspaceMembers(getDefaultWorkspaceId());
      expect(members).not.toContain(memberId);
    }).toPass({ timeout: 10_000 });

    // Reload to verify the member is gone from the UI
    await page.reload();
    await waitForWasm(page);
    await expect(page.getByRole('heading', { name: 'Workspace Members' })).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(memberEmail)).not.toBeVisible({ timeout: 5_000 });
  });
});
