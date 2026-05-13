import { test, expect } from '@playwright/test';
import { gotoAuthenticated, waitForWasm, getDefaultWorkspaceId, db } from '../../helpers/test-helpers';

const WORKSPACE_URL = '/settings/workspace';

test.describe('TC-015: Workspace Settings', () => {
  let originalName: string;

  test.afterAll(() => {
    if (originalName) {
      db.exec(
        `UPDATE workspaces SET name = '${originalName}' WHERE workspace_id = '${getDefaultWorkspaceId()}'`
      );
    }
  });

  test('should update workspace name, show save confirmation, and persist across refresh', async ({
    page,
  }) => {
    await gotoAuthenticated(page, WORKSPACE_URL);

    const nameInput = page.locator('input[type="text"][placeholder="My Workspace"]');
    await expect(nameInput).toBeVisible({ timeout: 10_000 });

    originalName = await nameInput.inputValue();
    expect(originalName).toBeTruthy();

    const newName = `Test Workspace ${Date.now()}`;
    await nameInput.clear();
    await nameInput.fill(newName);

    // Workspace name auto-saves on blur
    await nameInput.blur();

    await expect(page.getByText('Saved')).toBeVisible({ timeout: 5_000 });

    await page.reload();
    await waitForWasm(page);

    const refreshedInput = page.locator('input[type="text"][placeholder="My Workspace"]');
    await expect(refreshedInput).toBeVisible({ timeout: 10_000 });
    await expect(refreshedInput).toHaveValue(newName);
  });
});
