import { test, expect, type Page } from '@playwright/test';
import { gotoAuthenticated, isPersonalMode } from '../../helpers/test-helpers';

const SECURITY_URL = '/settings/security';

async function navigateToSecurity(page: Page) {
  await gotoAuthenticated(page, SECURITY_URL);
}

test.describe('TC-060: Security - API Key Management', () => {
  test('should show the API Keys card', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    await expect(page.getByText('API Keys', { exact: true })).toBeVisible();
    await expect(page.getByText('Create and manage API keys for REST API and MCP access.')).toBeVisible();
  });

  test('should show empty state when no API keys exist', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    await expect(page.getByText('No API keys')).toBeVisible({ timeout: 10000 });
    await expect(page.getByText('Create an API key to access the REST API or connect MCP clients.')).toBeVisible();
  });

  test('should open create API key modal', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    await page.getByRole('button', { name: 'Create API Key' }).click();

    // Modal should be visible with form elements
    await expect(page.getByText('Name')).toBeVisible();
    await expect(page.getByText('Permissions')).toBeVisible();
    await expect(page.getByText('Expiration')).toBeVisible();
    await expect(page.getByPlaceholder('e.g. CI/CD Pipeline, MCP Client')).toBeVisible();
  });

  test('should create an API key and show token once', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    // Open create modal
    await page.getByRole('button', { name: 'Create API Key' }).click();

    // Fill in name
    await page.getByPlaceholder('e.g. CI/CD Pipeline, MCP Client').fill('Test CI Key');

    // Select a scope
    await page.getByText('Read issues').click();

    // Click create
    await page.getByRole('button', { name: 'Create Key' }).click();

    // Should show the token warning and copy button
    await expect(page.getByText("Make sure to copy your API key now. You won't be able to see it again!")).toBeVisible({ timeout: 10000 });
    await expect(page.getByRole('button', { name: 'Copy' })).toBeVisible();

    // Token should start with "trakkt-"
    const tokenElement = page.locator('code');
    await expect(tokenElement).toBeVisible();
    const tokenText = await tokenElement.textContent();
    expect(tokenText).toMatch(/^trakkt-/);

    // Close the modal
    await page.getByRole('button', { name: 'Done' }).click();

    // Key should now appear in the list
    await expect(page.getByText('Test CI Key')).toBeVisible();
  });

  test('should revoke an API key with confirmation', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    // First create a key to revoke
    await page.getByRole('button', { name: 'Create API Key' }).click();
    await page.getByPlaceholder('e.g. CI/CD Pipeline, MCP Client').fill('Key to Revoke');
    await page.getByRole('button', { name: 'Create Key' }).click();
    await expect(page.getByText("Make sure to copy your API key now.")).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'Done' }).click();

    // Find the revoke button for the key
    const keyRow = page.locator('tr').filter({ hasText: 'Key to Revoke' });
    await expect(keyRow).toBeVisible();
    await keyRow.getByRole('button', { name: 'Revoke' }).click();

    // Confirm dialog should appear
    await expect(page.getByText('Revoke API Key?')).toBeVisible();
    await expect(page.getByText('Are you sure you want to revoke "Key to Revoke"?')).toBeVisible();

    // Confirm revocation
    await page.getByRole('button', { name: 'Revoke' }).last().click();

    // Key should show as revoked
    await expect(page.getByText('Revoked')).toBeVisible({ timeout: 10000 });
  });

  test('should not allow creating a key with empty name', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    await page.getByRole('button', { name: 'Create API Key' }).click();

    // Create button should be disabled when name is empty
    const createButton = page.getByRole('button', { name: 'Create Key' });
    await expect(createButton).toBeDisabled();
  });

  test('should show scope badges in the key list', async ({ page }) => {
    test.skip(isPersonalMode(), 'Security tab hidden in personal mode');

    await navigateToSecurity(page);

    // Create a key with specific scopes
    await page.getByRole('button', { name: 'Create API Key' }).click();
    await page.getByPlaceholder('e.g. CI/CD Pipeline, MCP Client').fill('Scoped Key');
    await page.getByText('Read issues').click();
    await page.getByText('Create/update issues').click();
    await page.getByRole('button', { name: 'Create Key' }).click();
    await expect(page.getByText("Make sure to copy your API key now.")).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'Done' }).click();

    // Check scope badges appear in the list
    const keyRow = page.locator('tr').filter({ hasText: 'Scoped Key' });
    await expect(keyRow).toBeVisible();
    await expect(keyRow.getByText('issues:read')).toBeVisible();
    await expect(keyRow.getByText('issues:write')).toBeVisible();
  });
});
