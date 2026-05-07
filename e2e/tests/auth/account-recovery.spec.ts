// SPDX-License-Identifier: AGPL-3.0-or-later

import { test, expect } from '@playwright/test';
import {
  waitForWasm,
  generateEmail,
  isPersonalMode,
  db,
  walCheckpoint,
} from '../../helpers/test-helpers';

test.describe('TC-021: Account Recovery — Start Page', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/account/recover');
    await waitForWasm(page);
  });

  test('renders recovery form with email input and submit button', async ({ page }) => {
    await expect(page.locator('h1')).toContainText('Recover Your Account');
    await expect(page.locator('#recovery-email')).toBeVisible();
    await expect(page.locator('button[type="submit"]')).toBeVisible();
    await expect(page.locator('button[type="submit"]')).toContainText('Send Recovery Link');
    await expect(page.locator('a[href="/login"]')).toContainText('Back to login');
  });

  test('submit button is disabled when email is empty', async ({ page }) => {
    await expect(page.locator('button[type="submit"]')).toBeDisabled();
  });

  test('submit button enables after entering an email', async ({ page }) => {
    await page.fill('#recovery-email', 'user@example.com');
    await expect(page.locator('button[type="submit"]')).toBeEnabled();
  });

  test('submitting an email transitions to "Check Your Email" view', async ({ page }) => {
    const email = generateEmail();
    await page.fill('#recovery-email', email);
    await page.click('button[type="submit"]');

    await expect(page.locator('h1')).toContainText('Check Your Email', { timeout: 10_000 });
    await expect(page.locator('text=recovery link expires in 15 minutes')).toBeVisible();
    await expect(page.locator('text=Back to Login')).toBeVisible();
    await expect(page.locator('text=Try a different email')).toBeVisible();
  });

  test('"Try a different email" returns to the recovery form', async ({ page }) => {
    await page.fill('#recovery-email', generateEmail());
    await page.click('button[type="submit"]');
    await expect(page.locator('h1')).toContainText('Check Your Email', { timeout: 10_000 });

    await page.click('text=Try a different email');
    await expect(page.locator('h1')).toContainText('Recover Your Account', { timeout: 5_000 });
    await expect(page.locator('#recovery-email')).toBeVisible();
    await expect(page.locator('#recovery-email')).toHaveValue('');
  });

  test('login page links to recovery page', async ({ browser }) => {
    // Use a fresh context without storageState to avoid auto-auth redirect
    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await page.goto('/login');
      await waitForWasm(page);

      const emailField = page.locator('#login-email');
      const isLoginPage = await emailField.isVisible({ timeout: 5_000 }).catch(() => false);
      if (!isLoginPage) {
        test.skip(true, 'login page redirected (no unauthenticated state available)');
        return;
      }

      const recoverLink = page.locator('a[href="/account/recover"]');
      await expect(recoverLink).toBeVisible();
    } finally {
      await context.close();
    }
  });
});

test.describe('TC-021: Account Recovery — Completion Page', () => {
  test('renders error state with invalid token', async ({ page }) => {
    await page.goto('/account/recover/complete?token=invalid-token-abc123');
    await waitForWasm(page);

    await expect(page.locator('h1')).toContainText('Recovery Failed', { timeout: 10_000 });
    await expect(page.locator('a[href="/account/recover"]')).toContainText('Request New Recovery Link');
    await expect(page.locator('a[href="/login"]')).toContainText('Back to Login');
  });

  test('renders error state with missing token', async ({ page }) => {
    await page.goto('/account/recover/complete');
    await waitForWasm(page);

    await expect(page.locator('h1')).toContainText('Recovery Failed', { timeout: 10_000 });
  });

  test('recovery token from DB allows password reset', async ({ browser }) => {
    test.skip(isPersonalMode(), 'Requires self_hosted mode');

    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      // Trigger recovery for the local user to generate a token in the DB
      await page.goto('/account/recover');
      await waitForWasm(page);
      await page.fill('#recovery-email', 'local@localhost');
      await page.click('button[type="submit"]');
      await expect(page.locator('h1')).toContainText('Check Your Email', { timeout: 10_000 });

      // Read the token directly from SQLite
      const rows = db.query(
        `SELECT token_id FROM verification_tokens WHERE email = 'local@localhost' AND token_type = 'recovery' AND used = 0 ORDER BY created_at DESC LIMIT 1`
      );
      expect(rows.length).toBeGreaterThan(0);
      const tokenId = rows[0];

      // Navigate to the completion page with the real token
      await page.goto(`/account/recover/complete?token=${tokenId}`);
      await waitForWasm(page);

      // Should render the password reset form (not error state)
      const passwordField = page.locator('#new-password');
      const errorHeading = page.locator('h1:has-text("Recovery Failed")');

      const hasPasswordField = await passwordField.isVisible({ timeout: 10_000 }).catch(() => false);
      const hasError = await errorHeading.isVisible({ timeout: 2_000 }).catch(() => false);

      expect(hasPasswordField && !hasError).toBe(true);

      if (hasPasswordField) {
        await page.fill('#new-password', 'NewPassword456!');
        const confirmField = page.locator('#confirm-password');
        if (await confirmField.isVisible({ timeout: 2_000 }).catch(() => false)) {
          await confirmField.fill('NewPassword456!');
        }
        await page.click('button[type="submit"]');
        await page.waitForTimeout(3000);

        // Should show success or redirect to login
        const success = page.getByText(/password.*reset|password.*changed|success/i);
        const redirectedToLogin = page.url().includes('/login');
        const hasSuccess = await success.isVisible({ timeout: 5_000 }).catch(() => false);

        expect(hasSuccess || redirectedToLogin).toBe(true);
      }
    } finally {
      await context.close();
      // Clean up the recovery token
      db.exec(`DELETE FROM verification_tokens WHERE email = 'local@localhost' AND token_type = 'recovery'`);
      walCheckpoint();
    }
  });
});
