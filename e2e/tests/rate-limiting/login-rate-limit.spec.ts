import { test, expect } from '@playwright/test';
import {
  waitForWasm,
  DEFAULT_TEST_USER,
  fetchAuthConfig,
  isPersonalMode,
  isSelfHostedNoSmtp,
} from '../../helpers/test-helpers';

// Login rate limit: 10 requests per IP within a 300s window.
const LOGIN_IP_CAPACITY = 10;

test.describe('TC-026: Rate Limiting (Login)', () => {
  test.setTimeout(120_000);

  test('repeated wrong-password logins trigger rate limit error', async ({ page }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No login form in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    await page.goto('/login');
    await waitForWasm(page);
    await page.waitForSelector('#login-email', { timeout: 15000 });
    await page.waitForTimeout(500);

    let rateLimited = false;

    for (let i = 0; i < LOGIN_IP_CAPACITY + 2; i++) {
      await page.locator('#login-email').click();
      await page.locator('#login-email').fill('');
      await page.locator('#login-email').type(DEFAULT_TEST_USER.email, { delay: 5 });
      await page.locator('#login-password').click();
      await page.locator('#login-password').fill('');
      await page.locator('#login-password').type('WrongPassword999!', { delay: 5 });
      await page.click('button[type="submit"]');

      await page.waitForTimeout(1500);

      const errorText = await page.locator('.text-error-foreground').textContent().catch(() => null);
      if (errorText && errorText.includes('Too many login attempts')) {
        rateLimited = true;
        break;
      }
    }

    expect(rateLimited).toBe(true);

    const errorElement = page.locator('.text-error-foreground');
    await expect(errorElement).toContainText('Too many login attempts');
    await expect(errorElement).toContainText('seconds');
  });

  test('rate-limited user cannot log in even with correct password', async ({ page }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No login form in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    // The previous test exhausted the rate limit; the 300s window is still active.
    await page.goto('/login');
    await waitForWasm(page);
    await page.waitForSelector('#login-email', { timeout: 15000 });
    await page.waitForTimeout(500);

    await page.locator('#login-email').click();
    await page.locator('#login-email').type(DEFAULT_TEST_USER.email, { delay: 5 });
    await page.locator('#login-password').click();
    await page.locator('#login-password').type(DEFAULT_TEST_USER.password, { delay: 5 });
    await page.click('button[type="submit"]');

    await page.waitForTimeout(2000);

    expect(page.url()).toContain('/login');

    const errorElement = page.locator('.text-error-foreground');
    const errorVisible = await errorElement.isVisible().catch(() => false);
    if (errorVisible) {
      await expect(errorElement).toContainText('Too many login attempts');
    }
  });
});
