import { test, expect, type Page } from '@playwright/test';
import { gotoAuthenticated, getDefaultUserId, db } from '../../helpers/test-helpers';

const SECURITY_URL = '/settings/security';
const NEW_PASSWORD = 'TestPassword1234';
const CHANGED_PASSWORD = 'ChangedPassword456!';

async function navigateToSecurity(page: Page) {
  await gotoAuthenticated(page, SECURITY_URL);
}

async function clickButtonByText(page: Page, text: string) {
  await page.getByRole('button', { name: text }).click();
}

test.describe('TC-011: Security - Password Management', () => {
  test.beforeAll(() => {
    try {
      db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${getDefaultUserId()}' AND auth_type = 'password'`);
      db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    } catch {}
  });

  // Restore the default password so subsequent tests can log in
  test.afterAll(async () => {
    try {
      const argon2 = require('argon2');
      const hash = await argon2.hash('TestPassword1234', { type: argon2.argon2id });
      const authData = JSON.stringify({ hash }).replace(/'/g, "''");
      db.exec(`DELETE FROM user_auth_methods WHERE user_id = '${getDefaultUserId()}' AND auth_type = 'password'`);
      db.exec(`INSERT INTO user_auth_methods (user_id, auth_type, auth_data, active) VALUES ('${getDefaultUserId()}', 'password', '${authData}', 1)`);
      db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    } catch {}
  });

  test('should show the Password card on the security page', async ({ page }) => {
    await navigateToSecurity(page);

    const passwordCard = page.locator('text=Password').first();
    await expect(passwordCard).toBeVisible();
  });

  test('should set a password when user has none', async ({ page }) => {
    await navigateToSecurity(page);

    // In personal mode, the auto-provisioned user has no password.
    // The card should show "Set Password" button.
    const setPasswordBtn = page.getByRole('button', { name: 'Set Password' });

    // If "Change Password" is shown instead, the user already has a password
    // and we skip set-password testing.
    const changePasswordBtn = page.getByRole('button', { name: 'Change Password' });
    const hasPassword = await changePasswordBtn.isVisible({ timeout: 3000 }).catch(() => false);

    if (hasPassword) {
      test.skip(true, 'User already has a password — skipping set-password test');
      return;
    }

    await expect(setPasswordBtn).toBeVisible();
    await setPasswordBtn.click();

    // Fill in the set-password form (no current password field)
    const currentPasswordField = page.locator('#currentPassword');
    await expect(currentPasswordField).not.toBeVisible();

    await page.locator('#newPassword').fill(NEW_PASSWORD);
    await page.locator('#confirmPassword').fill(NEW_PASSWORD);

    // Submit
    await clickButtonByText(page, 'Set Password');

    // Wait for success feedback
    await expect(page.getByText('Password set successfully')).toBeVisible({ timeout: 10000 });
  });

  test('should reject password shorter than 8 characters', async ({ page }) => {
    await navigateToSecurity(page);

    // Open the password form
    const setBtn = page.getByRole('button', { name: /Set Password|Change Password/ });
    await setBtn.click();

    // If we're changing, fill current password too
    const currentPasswordField = page.locator('#currentPassword');
    if (await currentPasswordField.isVisible({ timeout: 2000 }).catch(() => false)) {
      await currentPasswordField.fill(NEW_PASSWORD);
    }

    await page.locator('#newPassword').fill('short');
    await page.locator('#confirmPassword').fill('short');

    await page.getByRole('button', { name: /Set Password|Change Password/ }).first().click();

    await expect(page.getByText('at least 8 characters')).toBeVisible({ timeout: 5000 });
  });

  test('should reject mismatched passwords', async ({ page }) => {
    await navigateToSecurity(page);

    const setBtn = page.getByRole('button', { name: /Set Password|Change Password/ });
    await setBtn.click();

    const currentPasswordField = page.locator('#currentPassword');
    if (await currentPasswordField.isVisible({ timeout: 2000 }).catch(() => false)) {
      await currentPasswordField.fill(NEW_PASSWORD);
    }

    await page.locator('#newPassword').fill('ValidPassword1!');
    await page.locator('#confirmPassword').fill('DifferentPassword2!');

    await page.getByRole('button', { name: /Set Password|Change Password/ }).first().click();

    await expect(page.getByText('do not match')).toBeVisible({ timeout: 5000 });
  });

  test('should change an existing password', async ({ page }) => {
    await navigateToSecurity(page);

    // Ensure user has a password first (set it if not)
    const changeBtn = page.getByRole('button', { name: 'Change Password' });
    const hasPassword = await changeBtn.isVisible({ timeout: 3000 }).catch(() => false);

    if (!hasPassword) {
      // Set a password first
      await page.getByRole('button', { name: 'Set Password' }).click();
      await page.locator('#newPassword').fill(NEW_PASSWORD);
      await page.locator('#confirmPassword').fill(NEW_PASSWORD);
      await clickButtonByText(page, 'Set Password');
      await expect(page.getByText('Password set successfully')).toBeVisible({ timeout: 10000 });

      // Reload to get fresh state
      await navigateToSecurity(page);
    }

    // Now change the password
    await page.getByRole('button', { name: 'Change Password' }).click();

    await expect(page.locator('#currentPassword')).toBeVisible();
    await page.locator('#currentPassword').fill(NEW_PASSWORD);
    await page.locator('#newPassword').fill(CHANGED_PASSWORD);
    await page.locator('#confirmPassword').fill(CHANGED_PASSWORD);

    await clickButtonByText(page, 'Change Password');

    await expect(page.getByText('Password changed successfully')).toBeVisible({ timeout: 10000 });
  });

  test('should reject wrong current password on change', async ({ page }) => {
    await navigateToSecurity(page);

    const changeBtn = page.getByRole('button', { name: 'Change Password' });
    const hasPassword = await changeBtn.isVisible({ timeout: 3000 }).catch(() => false);

    if (!hasPassword) {
      test.skip(true, 'User has no password — cannot test wrong-current-password');
      return;
    }

    await changeBtn.click();

    await page.locator('#currentPassword').fill('WrongPassword999!');
    await page.locator('#newPassword').fill('AnotherPassword1!');
    await page.locator('#confirmPassword').fill('AnotherPassword1!');

    await clickButtonByText(page, 'Change Password');

    // Server should return an error about incorrect current password
    const errorAlert = page.locator('[data-variant="error"], .text-error-foreground').first();
    await expect(errorAlert).toBeVisible({ timeout: 10000 });
  });

  test('should toggle password visibility', async ({ page }) => {
    await navigateToSecurity(page);

    const setBtn = page.getByRole('button', { name: /Set Password|Change Password/ });
    await setBtn.click();

    const newPasswordInput = page.locator('#newPassword');
    await expect(newPasswordInput).toHaveAttribute('type', 'password');

    // Click the eye toggle (sibling button of the input)
    const toggleBtn = newPasswordInput.locator('..').locator('button');
    await toggleBtn.click();

    await expect(newPasswordInput).toHaveAttribute('type', 'text');

    // Toggle back
    await toggleBtn.click();
    await expect(newPasswordInput).toHaveAttribute('type', 'password');
  });

  test('should cancel password form and reset fields', async ({ page }) => {
    await navigateToSecurity(page);

    const setBtn = page.getByRole('button', { name: /Set Password|Change Password/ });
    await setBtn.click();

    await page.locator('#newPassword').fill('SomePassword123!');
    await page.locator('#confirmPassword').fill('SomePassword123!');

    await clickButtonByText(page, 'Cancel');

    // Form should be hidden, summary card should be back
    await expect(page.getByRole('button', { name: /Set Password|Change Password/ })).toBeVisible();
    await expect(page.locator('#newPassword')).not.toBeVisible();
  });
});
