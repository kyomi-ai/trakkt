import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  seedUser,
  seedWorkspace,
  seedWorkspaceMember,
  cleanTestData,
  getDefaultUserId,
  getDefaultWorkspaceId,
  db,
} from '../../helpers/test-helpers';

test.describe('TC-020: Workspace Switcher', () => {
  test.afterAll(() => {
    cleanTestData();
  });

  test('should show the current workspace name in settings', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/workspace');

    const nameInput = page.locator('input[type="text"][placeholder="My Workspace"]');
    await expect(nameInput).toBeVisible({ timeout: 10_000 });

    const currentName = await nameInput.inputValue();
    expect(currentName).toBeTruthy();

    await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
  });

  test('should seed a second workspace for the auto-provisioned user', async () => {
    // Verify the auto-provisioned user is in the default workspace
    const defaultMembers = db.getWorkspaceMembers(getDefaultWorkspaceId());
    expect(defaultMembers).toContain(getDefaultUserId());

    // Seed a second workspace owned by the auto-provisioned user.
    // In personal mode there is no workspace switcher UI yet,
    // so this test validates the DB seeding is correct and the
    // user has membership in both workspaces.
    const secondWsId = seedWorkspace({
      ownerId: getDefaultUserId(),
      name: 'Second Workspace',
    });

    const secondMembers = db.getWorkspaceMembers(secondWsId);
    expect(secondMembers).toContain(getDefaultUserId());

    const firstMembers = db.getWorkspaceMembers(getDefaultWorkspaceId());
    expect(firstMembers).toContain(getDefaultUserId());

    // Verify the user is admin in both
    const role1 = db.getMemberRole(getDefaultWorkspaceId(), getDefaultUserId());
    expect(role1).toBe('workspace_admin');

    const role2 = db.getMemberRole(secondWsId, getDefaultUserId());
    expect(role2).toBe('workspace_admin');
  });
});
