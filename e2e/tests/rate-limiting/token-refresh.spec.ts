import { test, expect } from '@playwright/test';
import {
  waitForWasm,
  DEFAULT_TEST_USER,
  loginUser,
  hasCookie,
  isAuthenticated,
  fetchAuthConfig,
  isPersonalMode,
  isSelfHostedNoSmtp,
} from '../../helpers/test-helpers';

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:8099';

test.describe('TC-028: Token Refresh (Silent Re-authentication)', () => {
  test('POST /api/v1/auth/refresh returns new tokens for authenticated user', async ({
    page,
    context,
  }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No token auth in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    await loginUser(page, DEFAULT_TEST_USER.email, DEFAULT_TEST_USER.password);

    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );
    expect(await isAuthenticated(page)).toBe(true);

    expect(await hasCookie(context, 'access_token')).toBe(true);
    expect(await hasCookie(context, 'refresh_token')).toBe(true);

    const response = await page.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });

    expect(response.status()).toBe(200);

    const body = await response.json();
    expect(body).toHaveProperty('access_token');
    expect(body).toHaveProperty('token_type', 'bearer');
    expect(body).toHaveProperty('expires_in');
    expect(body).toHaveProperty('user');
    expect(body.user).toHaveProperty('user_id');
    expect(body.user).toHaveProperty('email', DEFAULT_TEST_USER.email);
    expect(typeof body.access_token).toBe('string');
    expect(body.access_token.length).toBeGreaterThan(0);
    expect(body.expires_in).toBeGreaterThan(0);
  });

  test('user remains authenticated after token refresh', async ({ page, context }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No token auth in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    await loginUser(page, DEFAULT_TEST_USER.email, DEFAULT_TEST_USER.password);

    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );

    const response = await page.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });
    expect(response.status()).toBe(200);

    expect(await hasCookie(context, 'access_token')).toBe(true);
    expect(await hasCookie(context, 'refresh_token')).toBe(true);

    await page.goto('/settings/profile');
    await waitForWasm(page);

    await expect(page).toHaveURL(/\/settings\/profile/);
    expect(await isAuthenticated(page)).toBe(true);
  });

  test('refresh without valid cookies returns 401', async ({ browser }) => {
    test.skip(isPersonalMode(), 'No token auth in personal mode');

    const freshContext = await browser.newContext();
    const freshPage = await freshContext.newPage();

    try {
      const response = await freshPage.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      });

      expect([400, 401, 403]).toContain(response.status());
    } finally {
      await freshContext.close();
    }
  });
});
