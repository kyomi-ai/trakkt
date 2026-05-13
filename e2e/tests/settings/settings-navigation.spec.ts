import { test, expect } from '@playwright/test';
import { gotoAuthenticated } from '../../helpers/test-helpers';

// In personal mode only the Profile tab is visible (Security, Workspace, Team
// are gated behind `!is_personal_mode`). These tests verify the settings shell
// navigation that IS available in personal mode, plus direct URL navigation to
// the hidden routes which still render via the router.

test.describe('TC-008: Settings Navigation', () => {
  test('navigates to /settings/profile and renders the settings shell', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    await expect(page).toHaveURL(/\/settings\/profile/);

    const heading = page.locator('h1', { hasText: 'Settings' });
    await expect(heading).toBeVisible();

    const subheading = page.locator('h2', { hasText: 'Profile Settings' });
    await expect(subheading).toBeVisible();
  });

  test('Profile tab is active when on /settings/profile', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const profileTab = page.locator('a[href="/settings/profile"]').filter({
      has: page.locator('span', { hasText: 'Profile' }),
    });
    await expect(profileTab).toBeVisible();
    await expect(profileTab).toHaveClass(/border-primary/);
  });

  test('/settings redirects to /settings/profile', async ({ page }) => {
    await gotoAuthenticated(page, '/settings');

    await expect(page).toHaveURL(/\/settings\/profile/);
  });

  test('direct URL navigation to /settings/security renders without error', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/security');

    await expect(page).toHaveURL(/\/settings\/security/);

    const shellHeading = page.locator('h1', { hasText: 'Settings' });
    await expect(shellHeading).toBeVisible();
  });

  test('direct URL navigation to /settings/workspace renders without error', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/workspace');

    await expect(page).toHaveURL(/\/settings\/workspace/);

    const shellHeading = page.locator('h1', { hasText: 'Settings' });
    await expect(shellHeading).toBeVisible();
  });

  test('direct URL navigation to /settings/team renders without error', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/team');

    await expect(page).toHaveURL(/\/settings\/team/);

    const shellHeading = page.locator('h1', { hasText: 'Settings' });
    await expect(shellHeading).toBeVisible();
  });

  test('SPA navigation between settings routes preserves shell DOM', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const shell = page.locator('h1', { hasText: 'Settings' });
    await expect(shell).toBeVisible();

    // Tag the shell heading with a marker attribute to detect full-page reloads
    await shell.evaluate((el) => el.setAttribute('data-spa-marker', 'alive'));

    // Navigate to /settings/security via direct URL bar simulation (client-side)
    await page.evaluate(() => {
      const link = document.querySelector('a[href="/settings/security"]') as HTMLAnchorElement | null;
      if (link) {
        link.click();
      } else {
        window.history.pushState({}, '', '/settings/security');
        window.dispatchEvent(new PopStateEvent('popstate'));
      }
    });
    await page.waitForTimeout(1500);

    // The shell heading should still have our marker — proves no full page reload
    const markerAfter = await shell.getAttribute('data-spa-marker');
    expect(markerAfter).toBe('alive');
  });

  test('close button navigates to home', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const closeButton = page.locator('a[aria-label="Close settings"]');
    await expect(closeButton).toBeVisible();

    const href = await closeButton.getAttribute('href');
    expect(href).toBe('/');
  });

  test('Sign Out button is present in settings', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const signOutButton = page.locator('button', { hasText: 'Sign Out' });
    await expect(signOutButton).toBeVisible();
  });
});
