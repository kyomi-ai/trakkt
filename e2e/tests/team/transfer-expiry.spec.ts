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

test.describe('TC-035: Team - Ownership Transfer Expiry', () => {
  const recipientEmail = generateEmail();
  const recipientName = 'Expiry Recipient';
  const recipientPassword = 'ExpiryPass123!';
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
      expired: true,
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

  test('expired transfer has expires_at in the past', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();

    const rows = db.query(
      `SELECT expires_at FROM ownership_transfers WHERE transfer_id = '${transferId}'`
    );
    expect(rows.length).toBe(1);

    const expiresAt = new Date(rows[0]);
    expect(expiresAt.getTime()).toBeLessThan(Date.now());
  });

  test('expired transfer retains pending status until processed', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();

    const status = db.getTransferStatus(transferId!);
    expect(status).toBe('pending');
  });

  test('recipient cannot accept expired transfer', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await loginUser(page, recipientEmail, recipientPassword);
      await page.goto(`/accept-ownership?transfer_id=${transferId}`);
      await waitForWasm(page);

      // Should show an error/expired state rather than the accept form
      const errorText = page.getByText(/expired|invalid|no longer available/i);
      const acceptBtn = page.getByRole('button', { name: /accept/i });

      const hasError = await errorText.isVisible({ timeout: 10_000 }).catch(() => false);
      const hasAcceptBtn = await acceptBtn.isVisible({ timeout: 2_000 }).catch(() => false);

      // Either an error message shows, or the accept button is missing/disabled
      expect(hasError || !hasAcceptBtn).toBe(true);
    } finally {
      await context.close();
    }

    const owner = db.getWorkspaceOwner(getDefaultWorkspaceId());
    expect(owner).toBe(getDefaultUserId());
  });

  test('original owner retains ownership despite expired transfer', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    const owner = db.getWorkspaceOwner(getDefaultWorkspaceId());
    expect(owner).toBe(getDefaultUserId());
  });

  test('marking expired transfer preserves ownership', () => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');
    expect(transferId).not.toBeNull();

    db.exec(`UPDATE ownership_transfers SET status = 'expired' WHERE transfer_id = '${transferId}'`);

    const status = db.getTransferStatus(transferId!);
    expect(status).toBe('expired');

    const owner = db.getWorkspaceOwner(getDefaultWorkspaceId());
    expect(owner).toBe(getDefaultUserId());
  });
});
