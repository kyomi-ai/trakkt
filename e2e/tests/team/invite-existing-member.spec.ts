import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  getDefaultWorkspaceId,
  db,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

// Use the auto-provisioned user (local@localhost) as the existing member.
// This avoids WAL visibility issues from cross-process SQLite seeding.
const EXISTING_MEMBER_EMAIL = 'local@localhost';

test.describe('TC-041: Invite Existing Workspace Member', () => {
  test.afterAll(() => {
    db.exec(`DELETE FROM workspace_invitations WHERE email = '${EXISTING_MEMBER_EMAIL}'`);
  });

  test('auto-provisioned user is already a member', () => {
    const rows = db.query(
      `SELECT wu.user_id FROM workspace_users wu
       JOIN users u ON wu.user_id = u.user_id
       WHERE wu.workspace_id = '${getDefaultWorkspaceId()}' AND u.email = '${EXISTING_MEMBER_EMAIL}' AND wu.active = 1`
    );
    expect(rows.length).toBe(1);
  });

  test('inviting an existing member shows error in the modal', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    const inviteButton = page.getByRole('button', { name: /Invite Member/i });
    await inviteButton.click();

    const modal = page.getByRole('heading', { name: 'Invite Team Member' });
    await expect(modal).toBeVisible({ timeout: 5_000 });

    const emailInput = page.locator('input[type="email"][placeholder="colleague@example.com"]');
    await emailInput.fill(EXISTING_MEMBER_EMAIL);

    const sendButton = page.getByRole('button', { name: 'Send Invitation' });
    await sendButton.click();

    // Should see an error alert in the modal about already being a member
    const errorAlert = page.locator('[data-variant="error"], .text-error-foreground, [role="alert"]');
    await expect(errorAlert.first()).toBeVisible({ timeout: 10_000 });
  });

  test('no invitation was created for the existing member', () => {
    const rows = db.query(
      `SELECT invitation_id FROM workspace_invitations WHERE email = '${EXISTING_MEMBER_EMAIL}' AND status = 'pending'`
    );
    expect(rows.length).toBe(0);
  });
});
