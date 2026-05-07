import { test, expect, type BrowserContext } from '@playwright/test';
import {
  waitForWasm,
  DEFAULT_TEST_USER,
  loginUser,
  getCookies,
  fetchAuthConfig,
  isPersonalMode,
  isSelfHostedNoSmtp,
} from '../../helpers/test-helpers';

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:8099';

async function getRefreshTokenValue(context: BrowserContext): Promise<string | null> {
  const cookies = await getCookies(context);
  const refreshCookie = cookies.find((c) => c.name === 'refresh_token');
  return refreshCookie?.value ?? null;
}

test.describe('TC-029: Token Refresh — Rotation Detection', () => {
  test('replaying a consumed refresh token is rejected and invalidates the family', async ({
    page,
    context,
  }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'No token auth in personal mode');
    test.skip(!isSelfHostedNoSmtp(config), 'Requires self_hosted mode without SMTP');

    // Step 1: Log in and capture the initial refresh token
    await loginUser(page, DEFAULT_TEST_USER.email, DEFAULT_TEST_USER.password);

    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );

    const originalRefreshToken = await getRefreshTokenValue(context);
    expect(originalRefreshToken).not.toBeNull();

    // Step 2: Legitimate refresh — consumes the original token, issues a new one
    const firstRefresh = await page.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });
    expect(firstRefresh.status()).toBe(200);

    const firstRefreshBody = await firstRefresh.json();
    expect(firstRefreshBody).toHaveProperty('access_token');

    const newRefreshToken = await getRefreshTokenValue(context);
    expect(newRefreshToken).not.toBeNull();
    expect(newRefreshToken).not.toBe(originalRefreshToken);

    // Step 3: Replay the OLD (already-consumed) refresh token to simulate theft
    await context.addCookies([
      {
        name: 'refresh_token',
        value: originalRefreshToken!,
        domain: new URL(BASE_URL).hostname,
        path: '/',
      },
    ]);

    const replayResponse = await page.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });

    expect([400, 401, 403]).toContain(replayResponse.status());

    // Step 4: The entire token family is now invalidated.
    // Even the legitimately-issued NEW token should be rejected.
    await context.addCookies([
      {
        name: 'refresh_token',
        value: newRefreshToken!,
        domain: new URL(BASE_URL).hostname,
        path: '/',
      },
    ]);

    const familyInvalidated = await page.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });

    expect([400, 401, 403]).toContain(familyInvalidated.status());

    // Step 5: User must re-authenticate — no valid session remains
    await context.clearCookies();
    await page.goto('/settings/profile');
    await waitForWasm(page);

    await page.waitForURL('**/login', { timeout: 15_000 });
    expect(page.url()).toContain('/login');
  });

  test('normal rotation issues distinct tokens on each refresh', async ({
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

    const tokenBefore = await getRefreshTokenValue(context);

    const response = await page.request.fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });
    expect(response.status()).toBe(200);

    const tokenAfter = await getRefreshTokenValue(context);

    expect(tokenBefore).not.toBeNull();
    expect(tokenAfter).not.toBeNull();
    expect(tokenAfter).not.toBe(tokenBefore);
  });
});
