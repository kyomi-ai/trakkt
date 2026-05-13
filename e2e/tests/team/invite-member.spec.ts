import { test, expect } from '@playwright/test';
import { gotoAuthenticated, db } from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

test.describe('TC-016: Team - Invite Member', () => {
  const testEmail = `test-invite-${Date.now()}@example.com`;

  test.afterAll(() => {
    db.exec(
      `DELETE FROM workspace_invitations WHERE email = '${testEmail}'`
    );
  });

  test('should invite a member and show them in the pending invitations list', async ({
    page,
  }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await expect(inviteButton).toBeVisible();
    await inviteButton.click();

    const modal = page.getByRole('heading', { name: 'Invite Team Member' });
    await expect(modal).toBeVisible({ timeout: 5_000 });

    const emailInput = page.locator('input[type="email"][placeholder="colleague@example.com"]');
    await expect(emailInput).toBeVisible();
    await emailInput.fill(testEmail);

    const sendButton = page.getByRole('button', { name: 'Send Invitation' });
    await expect(sendButton).toBeEnabled();
    await sendButton.click();

    // Modal should close after successful invitation
    await expect(modal).not.toBeVisible({ timeout: 10_000 });

    // Invited email should appear in the pending invitations section
    await expect(page.getByText(testEmail)).toBeVisible({ timeout: 10_000 });
  });
});
