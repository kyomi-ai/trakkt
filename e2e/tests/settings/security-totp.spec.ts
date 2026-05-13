import { test, expect, type Page } from '@playwright/test';
import { authenticator } from 'otplib';
import { gotoAuthenticated, waitForWasm, db, isPersonalMode } from '../../helpers/test-helpers';

const SECURITY_URL = '/settings/security';
const USER_PASSWORD = 'TotpTestPassword1!';

async function navigateToSecurity(page: Page) {
  await gotoAuthenticated(page, SECURITY_URL);
}

async function ensureUserHasPassword(page: Page) {
  await navigateToSecurity(page);

  const changeBtn = page.getByRole('button', { name: 'Change Password' });
  const hasPassword = await changeBtn.isVisible({ timeout: 3000 }).catch(() => false);

  if (!hasPassword) {
    await page.getByRole('button', { name: 'Set Password' }).click();
    await page.locator('#newPassword').fill(USER_PASSWORD);
    await page.locator('#confirmPassword').fill(USER_PASSWORD);

    await page.getByRole('button', { name: 'Set Password' }).click();
    await expect(page.getByText('Password set successfully')).toBeVisible({ timeout: 10000 });

    await navigateToSecurity(page);
  }
}

async function extractTotpSecret(page: Page): Promise<string> {
  const secretInput = page.locator('input[readonly]');
  await expect(secretInput).toBeVisible({ timeout: 10000 });
  const secret = await secretInput.inputValue();
  if (!secret || secret.length < 16) {
    throw new Error(`Invalid TOTP secret extracted: "${secret}"`);
  }
  return secret;
}

async function disable2FAIfEnabled(page: Page): Promise<void> {
  await navigateToSecurity(page);
  const disableBtn = page.getByRole('button', { name: 'Disable 2FA' });
  const isEnabled = await disableBtn.isVisible({ timeout: 5000 }).catch(() => false);
  if (isEnabled) {
    await disableBtn.click();
    await expect(page.getByText('2FA has been successfully disabled')).toBeVisible({ timeout: 10000 });
  }
}

test.describe('TC-012: Security - TOTP Two-Factor Authentication', () => {
  test.beforeEach(async ({ page }) => {
    await disable2FAIfEnabled(page);
  });

  test('should show 2FA status card as disabled initially', async ({ page }) => {
    await navigateToSecurity(page);

    await expect(page.getByText('Two-Factor Authentication')).toBeVisible();

    await expect(page.locator('button:has-text("Setup 2FA")')).toBeVisible();
  });

  test('should complete the full 2FA enable flow', async ({ page }) => {
    await ensureUserHasPassword(page);
    await navigateToSecurity(page);

    await page.locator('button:has-text("Setup 2FA")').click();

    await expect(page.getByText('Setup Two-Factor Authentication')).toBeVisible({ timeout: 10000 });

    const qrImage = page.locator('img[alt="2FA QR Code"]');
    await expect(qrImage).toBeVisible();

    const secret = await extractTotpSecret(page);

    const totpCode = authenticator.generate(secret);

    await page.locator('#verification-code').fill(totpCode);
    await page.getByRole('button', { name: 'Enable 2FA' }).click();

    await expect(page.getByText('2FA has been successfully enabled')).toBeVisible({ timeout: 10000 });

    await expect(page.getByText('Enabled', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Disable 2FA' })).toBeVisible();
  });

  test('should reject an invalid TOTP code during setup', async ({ page }) => {
    await navigateToSecurity(page);

    await page.locator('button:has-text("Setup 2FA")').click();
    await expect(page.getByText('Setup Two-Factor Authentication')).toBeVisible({ timeout: 10000 });

    await page.locator('#verification-code').fill('000000');
    await page.getByRole('button', { name: 'Enable 2FA' }).click();

    const errorVisible = await page.getByText(/invalid|incorrect|failed|wrong/i)
      .isVisible({ timeout: 10000 })
      .catch(() => false);
    expect(errorVisible).toBe(true);
  });

  test('should cancel 2FA setup and return to status view', async ({ page }) => {
    await navigateToSecurity(page);

    await page.locator('button:has-text("Setup 2FA")').click();
    await expect(page.getByText('Setup Two-Factor Authentication')).toBeVisible({ timeout: 10000 });

    await page.getByRole('button', { name: 'Cancel' }).click();

    await expect(page.locator('button:has-text("Setup 2FA")')).toBeVisible();
    await expect(page.locator('#verification-code')).not.toBeVisible();
  });

  test('should disable 2FA after it has been enabled', async ({ page }) => {
    await ensureUserHasPassword(page);
    await navigateToSecurity(page);

    // Enable 2FA first
    await page.locator('button:has-text("Setup 2FA")').click();
    await expect(page.getByText('Setup Two-Factor Authentication')).toBeVisible({ timeout: 10000 });

    const secret = await extractTotpSecret(page);
    const totpCode = authenticator.generate(secret);

    await page.locator('#verification-code').fill(totpCode);
    await page.getByRole('button', { name: 'Enable 2FA' }).click();
    await expect(page.getByText('2FA has been successfully enabled')).toBeVisible({ timeout: 10000 });

    // Now disable
    await page.getByRole('button', { name: 'Disable 2FA' }).click();

    await expect(page.getByText('2FA has been successfully disabled')).toBeVisible({ timeout: 10000 });

    await expect(page.locator('button:has-text("Setup 2FA")')).toBeVisible();
  });

  test('should require TOTP code at login when 2FA is enabled', async ({ page }) => {
    test.skip(isPersonalMode(), 'Personal mode has no login page — cannot test TOTP at login');

    await ensureUserHasPassword(page);
    await navigateToSecurity(page);

    // Enable 2FA
    await page.locator('button:has-text("Setup 2FA")').click();
    await expect(page.getByText('Setup Two-Factor Authentication')).toBeVisible({ timeout: 10000 });
    const totpSecret = await extractTotpSecret(page);
    const code = authenticator.generate(totpSecret);
    await page.locator('#verification-code').fill(code);
    await page.getByRole('button', { name: 'Enable 2FA' }).click();
    await expect(page.getByText('2FA has been successfully enabled')).toBeVisible({ timeout: 10000 });

    const userEmail = db.query("SELECT email FROM users LIMIT 1")[0];
    expect(userEmail).toBeTruthy();

    const newContext = await page.context().browser()!.newContext();
    const loginPage = await newContext.newPage();

    await loginPage.goto('/login');
    await waitForWasm(loginPage);

    await loginPage.locator('#login-email').fill(userEmail);
    await loginPage.locator('#login-password').fill(USER_PASSWORD);
    await loginPage.getByRole('button', { name: 'Sign In' }).click();

    await expect(loginPage.locator('#totp-code')).toBeVisible({ timeout: 10000 });
    await expect(loginPage.getByText('Two-Factor Authentication')).toBeVisible();

    const loginTotpCode = authenticator.generate(totpSecret);
    await loginPage.locator('#totp-code').fill(loginTotpCode);
    await loginPage.getByRole('button', { name: /Verify|Sign In/ }).click();

    await loginPage.waitForURL(url => !url.toString().includes('/login'), { timeout: 15000 });

    await newContext.close();
  });
});
