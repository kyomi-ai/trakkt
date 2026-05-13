import { test, expect } from '@playwright/test';
import { gotoAuthenticated, waitForWasm } from '../../helpers/test-helpers';

// Theme switching uses the AppearanceCard on /settings/profile.
// Three theme buttons: Light, Dark, System.
// Theme applies by toggling the "dark" class on <html>.
// - "dark" -> <html class="dark">
// - "light" -> <html> without "dark" class
// - "system" -> follows prefers-color-scheme media query

test.describe('TC-010: Appearance (Theme Switching)', () => {
  test.beforeEach(async ({ page }) => {
    await gotoAuthenticated(page, '/settings/profile');

    const appearanceTitle = page.locator('text=Appearance');
    await expect(appearanceTitle).toBeVisible();
  });

  test('three theme buttons are visible', async ({ page }) => {
    const lightButton = page.locator('button', { hasText: 'Light' });
    const darkButton = page.locator('button', { hasText: 'Dark' });
    const systemButton = page.locator('button', { hasText: 'System' });

    await expect(lightButton).toBeVisible();
    await expect(darkButton).toBeVisible();
    await expect(systemButton).toBeVisible();
  });

  test('selecting Dark theme adds "dark" class to html element', async ({ page }) => {
    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);

    const htmlClass = await page.locator('html').getAttribute('class');
    expect(htmlClass).toContain('dark');
  });

  test('selecting Light theme removes "dark" class from html element', async ({ page }) => {
    // First ensure dark is applied
    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);

    const htmlClassBefore = await page.locator('html').getAttribute('class');
    expect(htmlClassBefore).toContain('dark');

    // Switch to Light
    const lightButton = page.locator('button', { hasText: 'Light' });
    await lightButton.click();
    await page.waitForTimeout(500);

    const htmlClassAfter = await page.locator('html').getAttribute('class') ?? '';
    expect(htmlClassAfter).not.toContain('dark');
  });

  test('theme switch does not cause page reload', async ({ page }) => {
    // Tag the settings heading to detect full-page reloads
    const heading = page.locator('h1', { hasText: 'Settings' });
    await heading.evaluate((el) => el.setAttribute('data-spa-marker', 'alive'));

    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);

    const markerAfterDark = await heading.getAttribute('data-spa-marker');
    expect(markerAfterDark).toBe('alive');

    const lightButton = page.locator('button', { hasText: 'Light' });
    await lightButton.click();
    await page.waitForTimeout(500);

    const markerAfterLight = await heading.getAttribute('data-spa-marker');
    expect(markerAfterLight).toBe('alive');
  });

  test('selected theme button has active styling', async ({ page }) => {
    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);

    await expect(darkButton).toHaveClass(/border-primary/);

    const lightButton = page.locator('button', { hasText: 'Light' });
    await expect(lightButton).not.toHaveClass(/border-primary/);
  });

  test('switching from Dark to Light updates button active state', async ({ page }) => {
    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);
    await expect(darkButton).toHaveClass(/border-primary/);

    const lightButton = page.locator('button', { hasText: 'Light' });
    await lightButton.click();
    await page.waitForTimeout(500);

    await expect(lightButton).toHaveClass(/border-primary/);
    await expect(darkButton).not.toHaveClass(/border-primary/);
  });

  test('selecting System theme respects OS preference', async ({ page }) => {
    // Emulate dark color scheme preference
    await page.emulateMedia({ colorScheme: 'dark' });

    const systemButton = page.locator('button', { hasText: 'System' });
    await systemButton.click();
    await page.waitForTimeout(500);

    const htmlClassDark = await page.locator('html').getAttribute('class');
    expect(htmlClassDark).toContain('dark');

    // Emulate light color scheme preference
    await page.emulateMedia({ colorScheme: 'light' });
    await page.waitForTimeout(1000);

    const htmlClassLight = await page.locator('html').getAttribute('class') ?? '';
    expect(htmlClassLight).not.toContain('dark');
  });

  test('theme persists in localStorage', async ({ page }) => {
    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);

    const storedTheme = await page.evaluate(() => localStorage.getItem('trakkt-theme'));
    expect(storedTheme).toBe('dark');

    const lightButton = page.locator('button', { hasText: 'Light' });
    await lightButton.click();
    await page.waitForTimeout(500);

    const storedThemeAfter = await page.evaluate(() => localStorage.getItem('trakkt-theme'));
    expect(storedThemeAfter).toBe('light');
  });

  test('theme persists across page reload', async ({ page }) => {
    const darkButton = page.locator('button', { hasText: 'Dark' });
    await darkButton.click();
    await page.waitForTimeout(500);

    await page.reload();
    await waitForWasm(page);

    const htmlClass = await page.locator('html').getAttribute('class');
    expect(htmlClass).toContain('dark');

    // Dark button should be active after reload
    const darkButtonAfter = page.locator('button', { hasText: 'Dark' });
    await expect(darkButtonAfter).toHaveClass(/border-primary/);
  });
});
