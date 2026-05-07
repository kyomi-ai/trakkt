import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  seedInvitation,
  generateEmail,
  getDefaultUserId,
  getDefaultWorkspaceId,
  db,
  pastISO,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';
const WORKSPACE_ID = getDefaultWorkspaceId();
const OWNER_ID = getDefaultUserId();

test.describe('TC-033: Team - Invitation Expiry', () => {
  let expiredEmail: string;
  let activeEmail: string;
  let expiredInvitationId: string;
  let activeInvitationId: string;

  test.beforeAll(() => {
    expiredEmail = generateEmail();
    activeEmail = generateEmail();

    expiredInvitationId = seedInvitation({
      workspaceId: WORKSPACE_ID,
      email: expiredEmail,
      invitedBy: OWNER_ID,
      role: 'workspace_user',
      expired: true,
    });

    activeInvitationId = seedInvitation({
      workspaceId: WORKSPACE_ID,
      email: activeEmail,
      invitedBy: OWNER_ID,
      role: 'workspace_user',
      expired: false,
    });
  });

  test.afterAll(() => {
    db.exec(`DELETE FROM workspace_invitations WHERE invitation_id = '${expiredInvitationId}'`);
    db.exec(`DELETE FROM workspace_invitations WHERE invitation_id = '${activeInvitationId}'`);
  });

  test('expired invitation has expires_at in the past', () => {
    const rows = db.query(
      `SELECT expires_at FROM workspace_invitations WHERE invitation_id = '${expiredInvitationId}'`
    );
    expect(rows.length).toBe(1);

    const expiresAt = new Date(rows[0]);
    expect(expiresAt.getTime()).toBeLessThan(Date.now());
  });

  test('active invitation has expires_at in the future', () => {
    const rows = db.query(
      `SELECT expires_at FROM workspace_invitations WHERE invitation_id = '${activeInvitationId}'`
    );
    expect(rows.length).toBe(1);

    const expiresAt = new Date(rows[0]);
    expect(expiresAt.getTime()).toBeGreaterThan(Date.now());
  });

  test('both invitations remain status=pending in DB regardless of expiry', () => {
    expect(db.getInvitationStatus(expiredInvitationId)).toBe('pending');
    expect(db.getInvitationStatus(activeInvitationId)).toBe('pending');
  });

  test('active invitation is visible on the team page', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(activeEmail)).toBeVisible({ timeout: 10_000 });
  });
});
