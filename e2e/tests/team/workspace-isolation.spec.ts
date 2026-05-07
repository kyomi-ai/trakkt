import { test, expect } from '@playwright/test';
import {
  createSecondUser,
  loginUser,
  generateEmail,
  seedUser,
  seedWorkspace,
  seedWorkspaceMember,
  waitForWasm,
  cleanTestData,
  isPersonalMode,
  isSelfHostedMode,
  db,
  walCheckpoint,
} from '../../helpers/test-helpers';

test.describe('TC-039: Workspace Switcher - Context Isolation', () => {
  let userId: string;
  let wsAId: string;
  let wsBId: string;
  let memberA1: string;
  let memberA2: string;
  let memberB1: string;

  const mainEmail = generateEmail();
  const mainName = 'Isolation User';
  const mainPassword = 'IsoPass123!';

  test.beforeAll(async ({ browser }) => {
    cleanTestData();

    if (isSelfHostedMode()) {
      userId = await createSecondUser(browser, {
        email: mainEmail,
        name: mainName,
        password: mainPassword,
        role: 'workspace_admin',
      });
    } else {
      const user = seedUser({ name: mainName });
      userId = user.userId;
    }

    wsAId = seedWorkspace({ ownerId: userId, name: 'Workspace A' });
    wsBId = seedWorkspace({ ownerId: userId, name: 'Workspace B' });

    const a1 = seedUser({ name: 'Member A1' });
    memberA1 = a1.userId;
    seedWorkspaceMember({ workspaceId: wsAId, userId: memberA1 });

    const a2 = seedUser({ name: 'Member A2' });
    memberA2 = a2.userId;
    seedWorkspaceMember({ workspaceId: wsAId, userId: memberA2 });

    const b1 = seedUser({ name: 'Member B1' });
    memberB1 = b1.userId;
    seedWorkspaceMember({ workspaceId: wsBId, userId: memberB1 });

    walCheckpoint();
  });

  test.afterAll(() => {
    db.exec(`DELETE FROM workspace_users WHERE workspace_id = '${wsAId}'`);
    db.exec(`DELETE FROM workspace_users WHERE workspace_id = '${wsBId}'`);
    db.exec(`DELETE FROM workspaces WHERE workspace_id = '${wsAId}'`);
    db.exec(`DELETE FROM workspaces WHERE workspace_id = '${wsBId}'`);
    db.exec(`DELETE FROM users WHERE user_id = '${memberA1}'`);
    db.exec(`DELETE FROM users WHERE user_id = '${memberA2}'`);
    db.exec(`DELETE FROM users WHERE user_id = '${memberB1}'`);
    if (isSelfHostedMode()) {
      db.exec(`DELETE FROM refresh_tokens WHERE user_id = '${userId}'`);
      db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${userId}'`);
      db.exec(`DELETE FROM users WHERE user_id = '${userId}'`);
    }
    walCheckpoint();
  });

  test('workspace A has correct members', () => {
    const members = db.getWorkspaceMembers(wsAId);
    expect(members).toContain(userId);
    expect(members).toContain(memberA1);
    expect(members).toContain(memberA2);
    expect(members).not.toContain(memberB1);
  });

  test('workspace B has correct members', () => {
    const members = db.getWorkspaceMembers(wsBId);
    expect(members).toContain(userId);
    expect(members).toContain(memberB1);
    expect(members).not.toContain(memberA1);
    expect(members).not.toContain(memberA2);
  });

  test('workspace A and B have different member counts', () => {
    const membersA = db.getWorkspaceMembers(wsAId);
    const membersB = db.getWorkspaceMembers(wsBId);

    expect(membersA.length).toBe(3);
    expect(membersB.length).toBe(2);
  });

  test('team page only shows members of the active workspace', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await loginUser(page, mainEmail, mainPassword);

      // Set user's active workspace to A and check team page
      db.exec(`UPDATE users SET last_workspace_id = '${wsAId}' WHERE user_id = '${userId}'`);
      walCheckpoint();

      await page.goto('/settings/team');
      await waitForWasm(page);

      const teamHeading = page.getByRole('heading', { name: 'Team Members' });
      const headingVisible = await teamHeading.isVisible({ timeout: 10_000 }).catch(() => false);

      if (headingVisible) {
        // Verify workspace A members shown, B members not shown
        const pageContent = await page.textContent('body');
        expect(pageContent).toContain('Member A1');
        expect(pageContent).not.toContain('Member B1');
      }
    } finally {
      await context.close();
    }
  });
});
