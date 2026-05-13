import { test, expect } from '@playwright/test';
import {
  waitForWasm,
  generateEmail,
  seedInvitation,
  getDefaultUserId,
  getDefaultWorkspaceId,
  walCheckpoint,
  fetchAuthConfig,
  isPersonalMode,
  isSelfHostedNoSmtp,
} from '../../helpers/test-helpers';

// Register rate limit: 5 requests per IP within a 3600s window.
const REGISTER_IP_CAPACITY = 5;

test.describe('TC-027: Rate Limiting (Signup)', () => {
  test.setTimeout(120_000);

  test('repeated signups trigger rate limit error', async ({ page }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No signup form in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    // Clear cookies so we hit /signup as unauthenticated
    await page.context().clearCookies();
    await page.goto('/signup');
    await waitForWasm(page);
    await page.waitForSelector('#signup-email', { timeout: 15000 });
    await page.waitForTimeout(1000);

    let rateLimited = false;

    for (let i = 0; i < REGISTER_IP_CAPACITY + 2; i++) {
      const email = generateEmail();

      // Seed an invitation so the signup attempt is not blocked by "Registration is closed"
      seedInvitation({
        workspaceId: getDefaultWorkspaceId(),
        email,
        invitedBy: getDefaultUserId(),
      });
      walCheckpoint();

      await page.locator('#signup-email').click();
      await page.locator('#signup-email').fill('');
      await page.locator('#signup-email').type(email, { delay: 5 });
      await page.locator('#signup-name').click();
      await page.locator('#signup-name').fill('');
      await page.locator('#signup-name').type(`Signup Tester ${i}`, { delay: 5 });
      await page.locator('#signup-password').click();
      await page.locator('#signup-password').fill('');
      await page.locator('#signup-password').type('SignupTest123!', { delay: 5 });
      await page.click('button[type="submit"]');

      await page.waitForTimeout(2000);

      // Successful signups redirect away from /signup
      if (!page.url().includes('/signup') && !page.url().includes('/login')) {
        await page.context().clearCookies();
        await page.goto('/signup');
        await waitForWasm(page);
        await page.waitForSelector('#signup-email', { timeout: 15000 });
        await page.waitForTimeout(1000);
        continue;
      }

      const errorText = await page.locator('.text-error-foreground').textContent().catch(() => null);
      if (errorText && errorText.includes('Too many signup attempts')) {
        rateLimited = true;
        break;
      }

      // May have switched to check-email or login view
      if (page.url().includes('/login') || page.url().includes('/signup')) {
        await page.context().clearCookies();
        await page.goto('/signup');
        await waitForWasm(page);
        await page.waitForSelector('#signup-email', { timeout: 15000 });
        await page.waitForTimeout(1000);
      }
    }

    expect(rateLimited).toBe(true);

    const errorElement = page.locator('.text-error-foreground');
    await expect(errorElement).toContainText('Too many signup attempts');
  });

  test('rate-limited IP cannot create new accounts', async ({ page }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No signup form in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    // The previous test exhausted the register rate limit; the 3600s window is still active.
    await page.context().clearCookies();
    await page.goto('/signup');
    await waitForWasm(page);
    await page.waitForSelector('#signup-email', { timeout: 15000 });
    await page.waitForTimeout(1000);

    const email = generateEmail();
    seedInvitation({
      workspaceId: getDefaultWorkspaceId(),
      email,
      invitedBy: getDefaultUserId(),
    });
    walCheckpoint();

    await page.locator('#signup-email').click();
    await page.locator('#signup-email').type(email, { delay: 5 });
    await page.locator('#signup-name').click();
    await page.locator('#signup-name').type('Blocked Signup', { delay: 5 });
    await page.locator('#signup-password').click();
    await page.locator('#signup-password').type('BlockedTest123!', { delay: 5 });
    await page.click('button[type="submit"]');

    await page.waitForTimeout(2000);

    const errorElement = page.locator('.text-error-foreground');
    const errorVisible = await errorElement.isVisible().catch(() => false);
    if (errorVisible) {
      await expect(errorElement).toContainText('Too many signup attempts');
    }

    const url = page.url();
    expect(url.includes('/signup') || url.includes('/login')).toBe(true);
  });
});
