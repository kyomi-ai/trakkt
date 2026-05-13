import { test, expect } from '@playwright/test';
import { gotoAuthenticated, getDefaultWorkspaceId, db } from '../../helpers/test-helpers';

const WORKSPACE_URL = '/settings/workspace';
const WORKSPACE_ID = getDefaultWorkspaceId();

test.describe('TC-030: Workspace Settings - Model Selection', () => {
  let originalSettings: string;

  test.beforeAll(() => {
    const rows = db.query(`SELECT settings FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`);
    originalSettings = rows[0] ?? '';
  });

  test.afterAll(() => {
    if (originalSettings) {
      db.exec(
        `UPDATE workspaces SET settings = '${originalSettings.replace(/'/g, "''")}' WHERE workspace_id = '${WORKSPACE_ID}'`
      );
    } else {
      db.exec(`UPDATE workspaces SET settings = NULL WHERE workspace_id = '${WORKSPACE_ID}'`);
    }
  });

  test('workspace settings page loads and shows heading', async ({ page }) => {
    await gotoAuthenticated(page, WORKSPACE_URL);

    const heading = page.locator('h2', { hasText: 'Workspace Settings' });
    await expect(heading).toBeVisible();
  });

  test('workspace name card is visible and editable', async ({ page }) => {
    await gotoAuthenticated(page, WORKSPACE_URL);

    const nameInput = page.locator('input[placeholder="My Workspace"]');
    await expect(nameInput).toBeVisible();

    const currentValue = await nameInput.inputValue();
    expect(currentValue.length).toBeGreaterThan(0);
  });

  test('setting model via DB updates the workspace settings JSON', () => {
    const testModel = 'claude-sonnet-4-5-20250929';

    db.exec(
      `UPDATE workspaces SET settings = json_set(
        COALESCE(settings, '{}'),
        '$.custom_settings.default_model',
        '${testModel}'
      ) WHERE workspace_id = '${WORKSPACE_ID}'`
    );

    const rows = db.query(
      `SELECT json_extract(settings, '$.custom_settings.default_model') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
    );
    expect(rows[0]).toBe(testModel);
  });

  test('model persists after workspace name update via UI', async ({ page }) => {
    const testModel = 'claude-haiku-3-5-20241022';

    db.exec(
      `UPDATE workspaces SET settings = json_set(
        COALESCE(settings, '{}'),
        '$.custom_settings.default_model',
        '${testModel}'
      ) WHERE workspace_id = '${WORKSPACE_ID}'`
    );

    await gotoAuthenticated(page, WORKSPACE_URL);

    const nameInput = page.locator('input[placeholder="My Workspace"]');
    await expect(nameInput).toBeVisible({ timeout: 10_000 });

    const currentName = await nameInput.inputValue();
    await nameInput.clear();
    await nameInput.fill(currentName + ' X');
    await nameInput.blur();
    await page.waitForTimeout(2000);

    // Model setting should still be intact after name save
    const rows = db.query(
      `SELECT json_extract(settings, '$.custom_settings.default_model') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
    );
    expect(rows[0]).toBe(testModel);

    // Restore name
    await nameInput.clear();
    await nameInput.fill(currentName);
    await nameInput.blur();
    await page.waitForTimeout(2000);
  });

  test('different models can be set and read back', () => {
    const models = [
      'claude-sonnet-4-5-20250929',
      'claude-haiku-3-5-20241022',
      'gpt-4o-2024-11-20',
    ];

    for (const model of models) {
      db.exec(
        `UPDATE workspaces SET settings = json_set(
          COALESCE(settings, '{}'),
          '$.custom_settings.default_model',
          '${model}'
        ) WHERE workspace_id = '${WORKSPACE_ID}'`
      );

      const rows = db.query(
        `SELECT json_extract(settings, '$.custom_settings.default_model') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
      );
      expect(rows[0]).toBe(model);
    }
  });
});
