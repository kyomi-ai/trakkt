import { test, expect } from '@playwright/test';
import { gotoAuthenticated } from '../../helpers/test-helpers';

// In personal mode the ProfileInfoCard (name + email fields) is hidden behind
// `<Show when=move || !is_personal>`. The Appearance card and MCP Connection
// card are always visible. These tests verify what IS available on the profile
// page in personal mode.

test.describe('TC-009: Profile Settings', () => {
  test('profile page loads and shows heading', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const heading = page.locator('h2', { hasText: 'Profile Settings' });
    await expect(heading).toBeVisible();
  });

  test('Appearance card is visible on profile page', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const appearanceTitle = page.locator('text=Appearance');
    await expect(appearanceTitle).toBeVisible();

    const description = page.locator('text=Choose how Tane looks to you.');
    await expect(description).toBeVisible();
  });

  test('MCP Connection card is visible on profile page', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const mcpTitle = page.locator('text=MCP Connection');
    await expect(mcpTitle).toBeVisible();
  });

  test('MCP Connection card shows server URL', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const serverUrlHeading = page.locator('h4', { hasText: 'Server URL' });
    await expect(serverUrlHeading).toBeVisible();

    const urlPre = page.locator('pre').filter({ hasText: '/mcp' });
    await expect(urlPre.first()).toBeVisible();
  });

  test('MCP Connection card shows Claude Code section in personal mode', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const claudeCodeHeading = page.locator('h4', { hasText: 'Claude Code' });
    await expect(claudeCodeHeading).toBeVisible();

    const command = page.locator('pre').filter({ hasText: 'claude mcp add' });
    await expect(command).toBeVisible();
  });

  test('MCP Connection card shows Claude Desktop section in personal mode', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const claudeDesktopHeading = page.locator('h4', { hasText: 'Claude Desktop' });
    await expect(claudeDesktopHeading).toBeVisible();

    const config = page.locator('pre').filter({ hasText: 'mcpServers' });
    await expect(config).toBeVisible();
  });

  test('MCP Connection card shows Cursor section', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const cursorHeading = page.locator('h4', { hasText: 'Cursor' });
    await expect(cursorHeading).toBeVisible();

    const connectLink = page.locator('a', { hasText: 'Connect with Cursor' });
    await expect(connectLink).toBeVisible();
  });

  test('Profile Info card is hidden in personal mode', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    // ProfileInfoCard shows "Profile Information" title — should NOT be visible
    const profileInfoTitle = page.locator('text=Profile Information');
    await expect(profileInfoTitle).toHaveCount(0);
  });

  test('profile page does not show error state', async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const errorText = page.locator('text=Failed to load profile');
    await expect(errorText).toHaveCount(0);
  });
});
