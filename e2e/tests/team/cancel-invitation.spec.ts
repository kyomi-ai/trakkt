import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  generateEmail,
  db,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

test.describe('TC-032: Team - Cancel Invitation', () => {
  const testEmail = `test-cancel-${Date.now()}@example.com`;

  test.afterAll(() => {
    db.exec(`DELETE FROM workspace_invitations WHERE email = '${testEmail}'`);
  });

  test('create invitation, cancel it, verify removed from pending list', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    // Step 1: Create an invitation via UI
    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await expect(inviteButton).toBeVisible();
    await inviteButton.click();

    const modal = page.getByRole('heading', { name: 'Invite Team Member' });
    await expect(modal).toBeVisible({ timeout: 5_000 });

    const emailInput = page.locator('input[type="email"][placeholder="colleague@example.com"]');
    await emailInput.fill(testEmail);

    const sendButton = page.getByRole('button', { name: 'Send Invitation' });
    await expect(sendButton).toBeEnabled();
    await sendButton.click();

    await expect(modal).not.toBeVisible({ timeout: 10_000 });

    // Verify invitation appears in pending list
    await expect(page.getByText(testEmail)).toBeVisible({ timeout: 10_000 });

    // Verify invitation is pending in DB
    const invRows = db.query(
      `SELECT invitation_id, status FROM workspace_invitations WHERE email = '${testEmail}'`
    );
    expect(invRows.length).toBe(1);
    expect(invRows[0]).toContain('pending');

    // Step 2: Cancel the invitation via the trash button on the invitation row
    // Use the email text to find its parent row, then locate the cancel button
    const emailSpan = page.locator('span', { hasText: testEmail });
    const invitationCard = emailSpan.locator('xpath=ancestor::div[contains(@class, "border-border") and contains(@class, "rounded-lg")]').first();
    const cancelButton = invitationCard.locator('button[title="Cancel invitation"]');
    await expect(cancelButton).toBeVisible();
    await cancelButton.click();

    // Confirm dialog appears (the alertdialog with Cancel/Confirm buttons)
    const dialog = page.locator('[role="alertdialog"]');
    await expect(dialog).toBeVisible({ timeout: 5_000 });

    const confirmButton = dialog.locator('button', { hasText: 'Confirm' });
    await confirmButton.click();

    // Step 3: Verify invitation removed from pending list
    await expect(page.getByText(testEmail)).not.toBeVisible({ timeout: 10_000 });

    // Verify DB status is cancelled
    const statusRows = db.query(
      `SELECT status FROM workspace_invitations WHERE email = '${testEmail}'`
    );
    expect(statusRows[0]).toBe('cancelled');
  });
});
