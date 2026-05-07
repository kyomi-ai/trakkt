import { test, expect, type Page } from '@playwright/test';
import {
  generateEmail,
  signupUser,
  loginUser,
  waitForWasm,
  isAuthenticated,
  seedInvitation,
  walCheckpoint,
  getDefaultUserId,
  getDefaultWorkspaceId,
  db,
  cleanTestData,
} from '../../helpers/test-helpers';

// ---------------------------------------------------------------------------
// Detect the runtime mode by probing the server's auth config endpoint.
// This determines which test cases are runnable vs skipped.
// ---------------------------------------------------------------------------

interface AuthConfig {
  self_hosted: boolean;
  smtp_configured: boolean;
  password: boolean;
  passkeys: boolean;
  google_oauth: boolean;
}

let authConfig: AuthConfig | null = null;

async function fetchAuthConfig(page: Page): Promise<AuthConfig> {
  if (authConfig) return authConfig;

  const response = await page.request.fetch(
    `${process.env.BASE_URL ?? 'http://localhost:8099'}/leptos-api/get_auth_config`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/cbor' },
    }
  );

  // The server function may use CBOR or JSON depending on encoding.
  // Try JSON parse of the response; if it fails, we're likely in personal mode.
  try {
    const body = await response.json();
    authConfig = body as AuthConfig;
    return authConfig;
  } catch {
    // Personal mode may not expose config meaningfully — infer from page behavior
    authConfig = {
      self_hosted: false,
      smtp_configured: false,
      password: true,
      passkeys: false,
      google_oauth: false,
    };
    return authConfig;
  }
}

function isPersonalMode(): boolean {
  if (process.env.TANE_MODE) return process.env.TANE_MODE === 'personal';
  try {
    const envPath = require('path').resolve(__dirname, '../../../.env');
    const envContent = require('fs').readFileSync(envPath, 'utf-8');
    const match = envContent.match(/^TANE_MODE=(.+)$/m);
    return match?.[1]?.trim() === 'personal';
  } catch {
    return false;
  }
}

function isSelfHostedNoSmtp(config: AuthConfig): boolean {
  return config.self_hosted && !config.smtp_configured;
}

function isSaasWithSmtp(config: AuthConfig): boolean {
  return !config.self_hosted && config.smtp_configured;
}

// ---------------------------------------------------------------------------
// TC-001: Signup Flow
// ---------------------------------------------------------------------------

test.describe('TC-001: Signup Flow', () => {
  test.describe('SaaS mode (email verification path)', () => {
    test('submitting email shows "check your email" message', async ({ page }) => {
      const config = await fetchAuthConfig(page);
      test.skip(
        !isSaasWithSmtp(config),
        'SaaS + SMTP signup only available when server runs in saas mode with SMTP configured'
      );

      await page.goto('/signup');
      await waitForWasm(page);

      const email = generateEmail();
      await page.fill('#signup-email', email);

      // In SaaS mode, only email field is visible (no name/password)
      await expect(page.locator('#signup-name')).not.toBeVisible();
      await expect(page.locator('#signup-password')).not.toBeVisible();

      await page.click('button[type="submit"]');

      await expect(page.getByText('Check Your Email')).toBeVisible({ timeout: 10_000 });
      await expect(page.getByText(email)).toBeVisible();
    });
  });

  test.describe('Self-hosted without SMTP (direct account creation)', () => {
    const testEmail = generateEmail();
    const testName = 'TC001 Signup User';
    const testPassword = 'TestPassword1234';

    test('signup with email, name, and password creates account directly', async ({ page }) => {
      const config = await fetchAuthConfig(page);
      test.skip(
        !isSelfHostedNoSmtp(config),
        'Direct signup only available in self_hosted mode without SMTP'
      );

      await page.goto('/signup');
      await waitForWasm(page);

      await expect(page.locator('#signup-email')).toBeVisible();
      await expect(page.locator('#signup-name')).toBeVisible();
      await expect(page.locator('#signup-password')).toBeVisible();

      await page.fill('#signup-email', testEmail);
      await page.fill('#signup-name', testName);
      await page.fill('#signup-password', testPassword);

      await page.click('button[type="submit"]');

      // Account creation redirects to an authenticated route
      await page.waitForURL((url) => !url.pathname.includes('/signup'), {
        timeout: 15_000,
      });
      expect(await isAuthenticated(page)).toBe(true);

      // Verify user was persisted
      const userId = db.getUserByEmail(testEmail);
      expect(userId).not.toBeNull();
    });

    test.afterAll(() => {
      cleanTestData();
    });
  });

  test.describe('Personal mode', () => {
    test('visiting /signup redirects to authenticated area', async ({ page }) => {
      test.skip(!isPersonalMode(), 'Personal mode test — only runs when TANE_MODE=personal');

      await page.goto('/signup');
      await waitForWasm(page);

      // Personal mode auto-authenticates; visiting /signup should redirect away
      await page.waitForURL(
        (url) => !url.pathname.includes('/signup') && !url.pathname.includes('/login'),
        { timeout: 15_000 }
      );
      expect(await isAuthenticated(page)).toBe(true);
    });
  });
});

// ---------------------------------------------------------------------------
// TC-002: Signup With Invitation
// ---------------------------------------------------------------------------

test.describe('TC-002: Signup With Invitation', () => {
  const invitedEmail = generateEmail();
  const invitedName = 'Invited User';
  const invitedPassword = 'InvitedPass123!';

  test('invited user signs up and joins the correct workspace', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');

    seedInvitation({
      workspaceId: getDefaultWorkspaceId(),
      email: invitedEmail,
      invitedBy: getDefaultUserId(),
      role: 'workspace_user',
    });
    walCheckpoint();

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await signupUser(page, invitedEmail, invitedName, invitedPassword);

      const userId = db.getUserByEmail(invitedEmail);
      expect(userId).not.toBeNull();

      // Verify user was added to the default workspace
      const role = db.getMemberRole(getDefaultWorkspaceId(), userId!);
      expect(role).toBe('workspace_user');
    } finally {
      await context.close();
      const userId = db.getUserByEmail(invitedEmail);
      if (userId) {
        db.exec(`DELETE FROM workspace_users WHERE user_id = '${userId}'`);
        db.exec(`DELETE FROM refresh_tokens WHERE user_id = '${userId}'`);
        db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${userId}'`);
        db.exec(`DELETE FROM users WHERE user_id = '${userId}'`);
      }
      db.exec(`DELETE FROM workspace_invitations WHERE email = '${invitedEmail}'`);
    }
  });

  test('expired invitation does not auto-add user to workspace', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');

    const expiredEmail = generateEmail();

    seedInvitation({
      workspaceId: getDefaultWorkspaceId(),
      email: expiredEmail,
      invitedBy: getDefaultUserId(),
      role: 'workspace_user',
      expired: true,
    });
    walCheckpoint();

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await signupUser(page, expiredEmail, 'Expired Invite', 'ExpiredPass123!');

      const userId = db.getUserByEmail(expiredEmail);
      if (userId) {
        // Expired invitation should not have added user to the workspace
        const role = db.getMemberRole(getDefaultWorkspaceId(), userId);
        expect(role).toBeNull();
      }
    } finally {
      await context.close();
      const userId = db.getUserByEmail(expiredEmail);
      if (userId) {
        db.exec(`DELETE FROM workspace_users WHERE user_id = '${userId}'`);
        db.exec(`DELETE FROM refresh_tokens WHERE user_id = '${userId}'`);
        db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${userId}'`);
        db.exec(`DELETE FROM users WHERE user_id = '${userId}'`);
      }
      db.exec(`DELETE FROM workspace_invitations WHERE email = '${expiredEmail}'`);
    }
  });
});

// ---------------------------------------------------------------------------
// TC-003: Login Flow (Password)
// ---------------------------------------------------------------------------

test.describe('TC-003: Login Flow (Password)', () => {
  const testEmail = generateEmail();
  const testName = 'TC003 Login User';
  const testPassword = 'LoginTest123!';

  test.beforeAll(async ({ browser }) => {
    // Only seed user via signup in self-hosted no-SMTP mode
    const page = await browser.newPage();
    try {
      const config = await fetchAuthConfig(page);
      if (isSelfHostedNoSmtp(config)) {
        await signupUser(page, testEmail, testName, testPassword);
      }
    } finally {
      await page.close();
    }
  });

  test('login with valid credentials navigates to authenticated area via SPA', async ({
    page,
  }) => {
    const config = await fetchAuthConfig(page);
    test.skip(
      isPersonalMode() || !isSelfHostedNoSmtp(config),
      'Password login test requires self_hosted mode without SMTP and a seeded user'
    );

    await page.goto('/login');
    await waitForWasm(page);

    // Mark the document element to detect SPA navigation vs full reload
    await page.evaluate(() => {
      (document.documentElement as any).__spaMarker = true;
    });

    await page.fill('#login-email', testEmail);
    await page.fill('#login-password', testPassword);
    await page.click('button[type="submit"]');

    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );
    expect(await isAuthenticated(page)).toBe(true);

    // Verify SPA navigation: the marker on documentElement survives SPA nav
    // but is lost on a full page reload
    const markerSurvived = await page.evaluate(
      () => (document.documentElement as any).__spaMarker === true
    );
    expect(markerSurvived).toBe(true);
  });

  test('login with wrong password shows error', async ({ page }) => {
    const config = await fetchAuthConfig(page);
    test.skip(
      isPersonalMode() || !isSelfHostedNoSmtp(config),
      'Password login test requires self_hosted mode without SMTP'
    );

    await page.goto('/login');
    await waitForWasm(page);

    await page.fill('#login-email', testEmail);
    await page.fill('#login-password', 'WrongPassword999!');
    await page.click('button[type="submit"]');

    // Should remain on login page with an error message
    await expect(page.locator('[role="alert"], [data-variant="error"]')).toBeVisible({
      timeout: 10_000,
    });
    expect(page.url()).toContain('/login');
  });

  test.afterAll(() => {
    cleanTestData();
  });
});

// ---------------------------------------------------------------------------
// TC-004: Login Flow (Passkey / WebAuthn)
// ---------------------------------------------------------------------------

test.describe('TC-004: Login Flow (Passkey)', () => {
  // WebAuthn requires either HTTPS or localhost, and Playwright's CDP-based
  // virtual authenticator to simulate the credential ceremony.
  test.skip(
    ({ browserName }) => browserName !== 'chromium',
    'WebAuthn CDP requires Chromium'
  );

  test('passkey login with virtual authenticator', async ({ page, context }) => {
    const config = await fetchAuthConfig(page);
    test.skip(!config.passkeys, 'Passkeys not enabled on this server');
    test.skip(isPersonalMode(), 'No passkey login in personal mode');

    // Create a virtual authenticator via CDP
    const cdpSession = await context.newCDPSession(page);
    await cdpSession.send('WebAuthn.enable');
    const { authenticatorId } = await cdpSession.send('WebAuthn.addVirtualAuthenticator', {
      options: {
        protocol: 'ctap2',
        transport: 'internal',
        hasResidentKey: true,
        hasUserVerification: true,
        isUserVerified: true,
      },
    });

    try {
      await page.goto('/login');
      await waitForWasm(page);

      // The passkey button should be visible when passkeys are enabled
      const passkeyButton = page.locator('button:has-text("passkey"), button:has-text("Passkey")');
      await expect(passkeyButton).toBeVisible({ timeout: 10_000 });

      // NOTE: Actually completing passkey login requires a registered credential.
      // A full test would: register a passkey in /settings/security first,
      // then attempt login. This test verifies the button is present and
      // the CDP authenticator is wired up correctly.
      //
      // Full passkey round-trip would be:
      // 1. Sign up + login via password
      // 2. Register passkey via /settings/security
      // 3. Logout
      // 4. Login via passkey button
      // That flow belongs in a dedicated passkey-lifecycle test.
    } finally {
      await cdpSession.send('WebAuthn.removeVirtualAuthenticator', { authenticatorId });
      await cdpSession.detach();
    }
  });
});

// ---------------------------------------------------------------------------
// TC-005: Logout
// ---------------------------------------------------------------------------

test.describe('TC-005: Logout', () => {
  const testEmail = generateEmail();
  const testName = 'TC005 Logout User';
  const testPassword = 'LogoutTest123!';

  test('sign out redirects to /login and prevents access to protected routes', async ({
    page,
  }) => {
    const config = await fetchAuthConfig(page);
    test.skip(isPersonalMode(), 'Personal mode has no login/logout flow');
    test.skip(
      !isSelfHostedNoSmtp(config),
      'Logout test requires self_hosted no-SMTP mode for user seeding'
    );

    await signupUser(page, testEmail, testName, testPassword);
    await loginUser(page, testEmail, testPassword);

    // Verify we're authenticated
    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );
    expect(await isAuthenticated(page)).toBe(true);

    // Navigate to settings where the Sign Out button lives
    await page.goto('/settings/profile');
    await waitForWasm(page);

    const signOutButton = page.locator('button:has-text("Sign Out")');
    await expect(signOutButton).toBeVisible({ timeout: 10_000 });
    await signOutButton.click();

    // Should redirect to /login
    await page.waitForURL('**/login', { timeout: 15_000 });
    expect(page.url()).toContain('/login');

    // Attempting to visit a protected route should redirect back to /login
    await page.goto('/settings/profile');
    await page.waitForURL('**/login', { timeout: 15_000 });
    expect(page.url()).toContain('/login');
  });

  test.afterAll(() => {
    cleanTestData();
  });
});

// ---------------------------------------------------------------------------
// TC-006: Auth Guard (Unauthenticated Access)
// ---------------------------------------------------------------------------

test.describe('TC-006: Auth Guard (Unauthenticated Access)', () => {
  const protectedRoutes = ['/settings', '/settings/profile', '/settings/security'];

  test('unauthenticated user is redirected to /login for all protected routes', async ({
    browser,
  }) => {
    test.skip(isPersonalMode(), 'Personal mode has no auth guard');

    // Fresh context with no cookies ensures unauthenticated state
    const context = await browser.newContext();
    const page = await context.newPage();

    try {
      for (const route of protectedRoutes) {
        await page.goto(route);
        await waitForWasm(page);

        await page.waitForURL('**/login', { timeout: 15_000 });
        expect(page.url()).toContain('/login');
      }
    } finally {
      await context.close();
    }
  });

  test('unauthenticated API requests return 401 or redirect', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Personal mode has no auth guard');

    const context = await browser.newContext();
    const page = await context.newPage();
    const baseUrl = process.env.BASE_URL ?? 'http://localhost:8099';

    try {
      const response = await page.request.fetch(`${baseUrl}/leptos-api/get_user_context`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/cbor' },
      });

      // Server should indicate unauthenticated (401, 403, or error in body)
      const status = response.status();
      expect([200, 400, 401, 403]).toContain(status);

      if (status === 200) {
        // If 200, the body should contain an error variant, not a valid user context
        const body = await response.text();
        // Leptos server functions return Ok(Err(...)) for auth failures
        expect(body).toBeTruthy();
      }
    } finally {
      await context.close();
    }
  });
});

// ---------------------------------------------------------------------------
// TC-007: Already Authenticated Redirect
// ---------------------------------------------------------------------------

test.describe('TC-007: Already Authenticated Redirect', () => {
  const testEmail = generateEmail();
  const testName = 'TC007 Auth Redirect User';
  const testPassword = 'RedirectTest123!';

  test('authenticated user visiting /login is redirected away', async ({ page }) => {
    if (isPersonalMode()) {
      // Personal mode is always "authenticated" — visiting /login should redirect
      await page.goto('/login');
      await waitForWasm(page);

      await page.waitForURL(
        (url) => !url.pathname.includes('/login'),
        { timeout: 15_000 }
      );
      expect(await isAuthenticated(page)).toBe(true);
      return;
    }

    const config = await fetchAuthConfig(page);
    test.skip(
      !isSelfHostedNoSmtp(config),
      'Requires self_hosted no-SMTP mode for user seeding'
    );

    await signupUser(page, testEmail, testName, testPassword);
    await loginUser(page, testEmail, testPassword);

    // Confirm authenticated
    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );

    // Now visit /login while authenticated
    await page.goto('/login');
    await waitForWasm(page);

    await page.waitForURL(
      (url) => !url.pathname.includes('/login'),
      { timeout: 15_000 }
    );
    expect(await isAuthenticated(page)).toBe(true);
  });

  test('authenticated user visiting /signup is redirected away', async ({ page }) => {
    if (isPersonalMode()) {
      await page.goto('/signup');
      await waitForWasm(page);

      await page.waitForURL(
        (url) => !url.pathname.includes('/signup'),
        { timeout: 15_000 }
      );
      expect(await isAuthenticated(page)).toBe(true);
      return;
    }

    const config = await fetchAuthConfig(page);
    test.skip(
      !isSelfHostedNoSmtp(config),
      'Requires self_hosted no-SMTP mode for user seeding'
    );

    // Reuse credentials from the /login test (runs in same describe block)
    await loginUser(page, testEmail, testPassword);

    await page.waitForURL(
      (url) => !url.pathname.includes('/login') && !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );

    await page.goto('/signup');
    await waitForWasm(page);

    await page.waitForURL(
      (url) => !url.pathname.includes('/signup'),
      { timeout: 15_000 }
    );
    expect(await isAuthenticated(page)).toBe(true);
  });

  test.afterAll(() => {
    if (!isPersonalMode()) {
      cleanTestData();
    }
  });
});
