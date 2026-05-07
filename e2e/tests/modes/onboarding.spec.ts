import { test, expect } from '@playwright/test';
import { waitForWasm, generateEmail, isSelfHostedMode } from '../../helpers/test-helpers';

test.describe('TC-025: Onboarding Flow', () => {
  test.describe.configure({ mode: 'serial' });

  const isSelfHosted = isSelfHostedMode();

  test.skip(!isSelfHosted, 'Server must be running in self_hosted mode');

  const testEmail = generateEmail();
  const testName = 'E2E Onboarding User';
  const testPassword = 'OnboardTest123!';

  test('new signup redirects to /onboarding', async ({ page }) => {
    await page.context().clearCookies();
    await page.goto('/signup');
    await waitForWasm(page);
    await page.waitForSelector('#signup-email', { timeout: 15000 });
    await page.waitForTimeout(1000);

    await page.locator('#signup-email').click();
    await page.locator('#signup-email').type(testEmail, { delay: 10 });
    await page.locator('#signup-name').click();
    await page.locator('#signup-name').type(testName, { delay: 10 });
    await page.locator('#signup-password').click();
    await page.locator('#signup-password').type(testPassword, { delay: 10 });

    await page.click('button[type="submit"]');

    // AccountCreated redirects to "/" which redirects to /settings/profile,
    // or directly to /onboarding if the server routes new users there.
    await page.waitForURL(/\/(onboarding|settings\/profile)/, { timeout: 15_000 });
  });

  test('onboarding page renders welcome content', async ({ page }) => {
    await page.goto('/onboarding');
    await waitForWasm(page);

    const welcomeHeading = page.locator('text=Welcome aboard!');
    await expect(welcomeHeading).toBeVisible();

    const description = page.locator('text=Your account is ready');
    await expect(description).toBeVisible();

    const placeholderNotice = page.locator('text=This onboarding flow is a placeholder');
    await expect(placeholderNotice).toBeVisible();
  });

  test('"Go to Settings" button navigates to /settings/profile', async ({ page }) => {
    await page.goto('/onboarding');
    await waitForWasm(page);

    const goToSettingsButton = page.locator('button', { hasText: 'Go to Settings' });
    await expect(goToSettingsButton).toBeVisible();

    await goToSettingsButton.click();

    await page.waitForURL(/\/settings\/profile/, { timeout: 10_000 });
    await expect(page.locator('h2', { hasText: 'Profile Settings' })).toBeVisible();
  });

  test('re-login does not show onboarding again', async ({ page }) => {
    // Log out by clearing cookies
    await page.context().clearCookies();

    await page.goto('/login');
    await waitForWasm(page);
    await page.waitForSelector('#login-email', { timeout: 15000 });
    await page.waitForTimeout(500);

    await page.locator('#login-email').click();
    await page.locator('#login-email').type(testEmail, { delay: 10 });
    await page.locator('#login-password').click();
    await page.locator('#login-password').type(testPassword, { delay: 10 });
    await page.click('button[type="submit"]');

    await page.waitForURL(/\/(settings|$)/, { timeout: 15_000 });

    const url = page.url();
    expect(url).not.toContain('/onboarding');
  });
});
