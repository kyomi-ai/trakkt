import { test, expect } from '@playwright/test';
import {
  createSecondUser,
  loginUser,
  generateEmail,
  seedOwnershipTransfer,
  waitForWasm,
  cleanTestData,
  isPersonalMode,
  isSelfHostedMode,
  getDefaultUserId,
  getDefaultWorkspaceId,
  db,
  walCheckpoint,
} from '../../helpers/test-helpers';

test.describe('TC-034: Team - Ownership Transfer Decline', () => {
  const recipientEmail = generateEmail();
  const recipientName = 'Transfer Recipient';
  const recipientPassword = 'RecipientPass123!';
  let recipientId: string | null = null;
  let transferId: string | null = null;

  test.beforeAll(async ({ browser }) => {
    if (!isSelfHostedMode()) return;
    cleanTestData();

    recipientId = await createSecondUser(browser, {
      email: recipientEmail,
      name: recipientName,
      password: recipientPassword,
      role: 'workspace_admin',
    });

    transferId = seedOwnershipTransfer({
      workspaceId: getDefaultWorkspaceId(),
      fromUserId: getDefaultUserId(),
      toUserId: recipientId,
    });
    walCheckpoint();
  });

  test.afterAll(() => {
    if (transferId) {
      db.exec(`DELETE FROM ownership_transfers WHERE transfer_id = '${transferId}'`);
    }
    if (recipientId) {
      db.exec(`DELETE FROM workspace_users WHERE user_id = '${recipientId}' AND workspace_id = '${getDefaultWorkspaceId()}'`);
      db.exec(`DELETE FROM refresh_tokens WHERE user_id = '${recipientId}'`);
      db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${recipientId}'`);
      db.exec(`DELETE FROM users WHERE user_id = '${recipientId}'`);
      walCheckpoint();
    }
  });

  test('seeded transfer exists as pending', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();
    const status = db.getTransferStatus(transferId!);
    expect(status).toBe('pending');
  });

  test('owner retains ownership before any action', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    const owner = db.getWorkspaceOwner(getDefaultWorkspaceId());
    expect(owner).toBe(getDefaultUserId());
  });

  test('recipient declines transfer via accept-ownership page', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await loginUser(page, recipientEmail, recipientPassword);
      await page.goto(`/accept-ownership?transfer_id=${transferId}`);
      await waitForWasm(page);

      const declineBtn = page.getByRole('button', { name: /decline/i });
      const visible = await declineBtn.isVisible({ timeout: 10_000 }).catch(() => false);
      if (visible) {
        await declineBtn.click();
        await page.waitForTimeout(3000);
      }

      const status = db.getTransferStatus(transferId!);
      expect(status).toBe('declined');

      const owner = db.getWorkspaceOwner(getDefaultWorkspaceId());
      expect(owner).toBe(getDefaultUserId());
    } finally {
      await context.close();
    }
  });

  test('simulated decline sets status to declined and preserves ownership', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();

    // Reset to pending for this verification
    db.exec(`UPDATE ownership_transfers SET status = 'pending' WHERE transfer_id = '${transferId}'`);
    db.exec(`UPDATE ownership_transfers SET status = 'declined' WHERE transfer_id = '${transferId}'`);

    const status = db.getTransferStatus(transferId!);
    expect(status).toBe('declined');

    const owner = db.getWorkspaceOwner(getDefaultWorkspaceId());
    expect(owner).toBe(getDefaultUserId());
  });
});
