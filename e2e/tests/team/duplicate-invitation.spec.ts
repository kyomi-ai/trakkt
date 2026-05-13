import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  generateEmail,
  db,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

test.describe('TC-040: Duplicate Workspace Invitation', () => {
  const testEmail = generateEmail();

  test.afterAll(() => {
    db.exec(`DELETE FROM workspace_invitations WHERE email = '${testEmail}'`);
  });

  test('first invitation succeeds, duplicate shows error', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    // Step 1: First invitation
    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await inviteButton.click();

    const modal = page.getByRole('heading', { name: 'Invite Team Member' });
    await expect(modal).toBeVisible({ timeout: 5_000 });

    const emailInput = page.locator('input[type="email"][placeholder="colleague@example.com"]');
    await emailInput.fill(testEmail);

    const sendButton = page.getByRole('button', { name: 'Send Invitation' });
    await sendButton.click();

    await expect(modal).not.toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(testEmail)).toBeVisible({ timeout: 10_000 });

    // Verify 1 invitation in DB
    const rows1 = db.query(
      `SELECT invitation_id FROM workspace_invitations WHERE email = '${testEmail}' AND status = 'pending'`
    );
    expect(rows1.length).toBe(1);

    // Step 2: Attempt duplicate invitation
    await inviteButton.click();
    await expect(modal).toBeVisible({ timeout: 5_000 });

    await emailInput.fill(testEmail);
    await sendButton.click();

    // Should see an error in the modal
    const errorAlert = page.locator('[data-variant="error"], .text-error-foreground, [role="alert"]');
    await expect(errorAlert.first()).toBeVisible({ timeout: 10_000 });

    // Verify still only 1 pending invitation in DB
    const rows2 = db.query(
      `SELECT invitation_id FROM workspace_invitations WHERE email = '${testEmail}' AND status = 'pending'`
    );
    expect(rows2.length).toBe(1);
  });
});
