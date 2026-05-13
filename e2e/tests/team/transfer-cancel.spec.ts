import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  seedUser,
  seedWorkspaceMember,
  seedOwnershipTransfer,
  getDefaultUserId,
  getDefaultWorkspaceId,
  db,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';
const WORKSPACE_ID = getDefaultWorkspaceId();
const OWNER_ID = getDefaultUserId();

test.describe('TC-036: Team - Ownership Transfer Cancellation', () => {
  let recipientId: string;
  let transferId: string;

  test.beforeAll(() => {
    db.exec(`DELETE FROM ownership_transfers WHERE workspace_id = '${WORKSPACE_ID}' AND from_user_id = '${OWNER_ID}'`);
    db.exec(`DELETE FROM workspace_users WHERE workspace_id = '${WORKSPACE_ID}' AND user_id LIKE 'user-test%'`);
    db.exec(`DELETE FROM users WHERE user_id LIKE 'user-test-cancel-rcpt%'`);

    const recipient = seedUser({ userId: `user-test-cancel-rcpt-${Date.now()}`, name: 'Cancel Recipient' });
    recipientId = recipient.userId;
    seedWorkspaceMember({ workspaceId: WORKSPACE_ID, userId: recipientId, role: 'workspace_admin' });

    transferId = seedOwnershipTransfer({
      workspaceId: WORKSPACE_ID,
      fromUserId: OWNER_ID,
      toUserId: recipientId,
    });
  });

  test.afterAll(() => {
    db.exec(`DELETE FROM ownership_transfers WHERE transfer_id = '${transferId}'`);
    db.exec(`DELETE FROM workspace_users WHERE workspace_id = '${WORKSPACE_ID}' AND user_id = '${recipientId}'`);
    db.exec(`DELETE FROM users WHERE user_id = '${recipientId}'`);
  });

  test('cancel pending transfer via UI and verify in DB', async ({ page }) => {
    const status = db.getTransferStatus(transferId);
    expect(status).toBe('pending');

    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    // The transfer should be visible as "Pending transfer to ..."
    const pendingText = page.getByText('Pending transfer to');
    await expect(pendingText).toBeVisible({ timeout: 10_000 });

    const cancelButton = page.locator('button[title="Cancel transfer"]');
    await expect(cancelButton).toBeVisible();
    await cancelButton.click();

    // Confirm dialog
    const dialog = page.locator('[role="alertdialog"]');
    await expect(dialog).toBeVisible({ timeout: 5_000 });

    const confirmButton = dialog.locator('button', { hasText: 'Confirm' });
    await confirmButton.click();

    // Verify DB status changed
    await expect(async () => {
      const cancelledStatus = db.getTransferStatus(transferId);
      expect(cancelledStatus).toBe('cancelled');
    }).toPass({ timeout: 10_000 });

    // Ownership unchanged
    const owner = db.getWorkspaceOwner(WORKSPACE_ID);
    expect(owner).toBe(OWNER_ID);
  });
});
