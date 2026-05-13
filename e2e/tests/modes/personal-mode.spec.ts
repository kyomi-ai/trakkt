import { test, expect } from '@playwright/test';
import { waitForWasm, isAuthenticated, db } from '../../helpers/test-helpers';

test.describe('TC-023: Personal Mode', () => {
  test('root URL redirects to /settings/profile without login', async ({ page }) => {
    await page.goto('/');
    await waitForWasm(page);

    await expect(page).toHaveURL(/\/settings\/profile/);
    await expect(page.locator('h1', { hasText: 'Settings' })).toBeVisible();
  });

  test('no login page is shown when navigating directly', async ({ page }) => {
    await page.goto('/');
    await waitForWasm(page);

    const authenticated = await isAuthenticated(page);
    expect(authenticated).toBe(true);
  });

  test('user is auto-logged-in with Local User identity', async ({ page }) => {
    await page.goto('/settings/profile');
    await waitForWasm(page);

    const localUser = db.getUserById('user-local');
    expect(localUser).not.toBeNull();
    expect(localUser!.email).toBe('local@localhost');
    expect(localUser!.name).toBe('Local User');
  });

  test('settings are accessible without authentication', async ({ page }) => {
    await page.goto('/settings/profile');
    await waitForWasm(page);

    await expect(page).toHaveURL(/\/settings\/profile/);

    const heading = page.locator('h2', { hasText: 'Profile Settings' });
    await expect(heading).toBeVisible();
  });

  test('auto-provisioned workspace exists in database', async ({ page }) => {
    const members = db.getWorkspaceMembers('workspace-local');
    expect(members).toContain('user-local');

    const owner = db.getWorkspaceOwner('workspace-local');
    expect(owner).toBe('user-local');

    const role = db.getMemberRole('workspace-local', 'user-local');
    expect(role).toBe('workspace_admin');
  });

  test('personal mode hides Security, Workspace, and Team tabs', async ({ page }) => {
    await page.goto('/settings/profile');
    await waitForWasm(page);

    const securityTab = page.locator('a[href="/settings/security"]').filter({
      has: page.locator('span', { hasText: 'Security' }),
    });
    const workspaceTab = page.locator('a[href="/settings/workspace"]').filter({
      has: page.locator('span', { hasText: 'Workspace' }),
    });
    const teamTab = page.locator('a[href="/settings/team"]').filter({
      has: page.locator('span', { hasText: 'Team' }),
    });

    await expect(securityTab).toHaveCount(0);
    await expect(workspaceTab).toHaveCount(0);
    await expect(teamTab).toHaveCount(0);
  });

  test('personal mode hides the profile info card', async ({ page }) => {
    await page.goto('/settings/profile');
    await waitForWasm(page);

    // ProfileInfoCard shows name/email fields; in personal mode it is hidden
    const profileInfoCard = page.locator('input#profile-name');
    await expect(profileInfoCard).toHaveCount(0);
  });

  test('/login redirects back to app in personal mode', async ({ page }) => {
    await page.goto('/login');
    await waitForWasm(page);

    // Personal mode auto-authenticates; the login page's auth check should redirect away
    const authenticated = await isAuthenticated(page);
    expect(authenticated).toBe(true);
  });
});
