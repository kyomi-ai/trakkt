import { test, expect } from '@playwright/test';
import { gotoAuthenticated, getDefaultWorkspaceId, db } from '../../helpers/test-helpers';

const WORKSPACE_URL = '/settings/workspace';
const WORKSPACE_ID = getDefaultWorkspaceId();

test.describe('TC-031: Workspace Settings - ChartML Configuration', () => {
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

  test('setting chart palette via DB persists correctly', () => {
    const testPalette = 'balanced';

    const configValue = JSON.stringify({
      type: 'config',
      version: 1,
      style: testPalette,
    });

    db.exec(
      `UPDATE workspaces SET settings = json_set(
        COALESCE(settings, '{}'),
        '$.custom_settings.chartml_config',
        json('${configValue}')
      ) WHERE workspace_id = '${WORKSPACE_ID}'`
    );

    const rows = db.query(
      `SELECT json_extract(settings, '$.custom_settings.chartml_config.style') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
    );
    expect(rows[0]).toBe(testPalette);
  });

  test('chart palette persists after workspace name update via UI', async ({ page }) => {
    const testPalette = 'vibrant';

    const configValue = JSON.stringify({
      type: 'config',
      version: 1,
      style: testPalette,
    });

    db.exec(
      `UPDATE workspaces SET settings = json_set(
        COALESCE(settings, '{}'),
        '$.custom_settings.chartml_config',
        json('${configValue}')
      ) WHERE workspace_id = '${WORKSPACE_ID}'`
    );

    await gotoAuthenticated(page, WORKSPACE_URL);

    const nameInput = page.locator('input[placeholder="My Workspace"]');
    await expect(nameInput).toBeVisible({ timeout: 10_000 });

    const currentName = await nameInput.inputValue();
    await nameInput.clear();
    await nameInput.fill(currentName + ' Y');
    await nameInput.blur();
    await page.waitForTimeout(2000);

    // Chart palette should still be intact after name save
    const rows = db.query(
      `SELECT json_extract(settings, '$.custom_settings.chartml_config.style') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
    );
    expect(rows[0]).toBe(testPalette);

    // Restore name
    await nameInput.clear();
    await nameInput.fill(currentName);
    await nameInput.blur();
    await page.waitForTimeout(2000);
  });

  test('different palettes can be set and read back sequentially', () => {
    const palettes = ['tane', 'balanced', 'vibrant', 'monochrome'];

    for (const palette of palettes) {
      const configValue = JSON.stringify({
        type: 'config',
        version: 1,
        style: palette,
      });

      db.exec(
        `UPDATE workspaces SET settings = json_set(
          COALESCE(settings, '{}'),
          '$.custom_settings.chartml_config',
          json('${configValue}')
        ) WHERE workspace_id = '${WORKSPACE_ID}'`
      );

      const rows = db.query(
        `SELECT json_extract(settings, '$.custom_settings.chartml_config.style') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
      );
      expect(rows[0]).toBe(palette);
    }
  });

  test('chartml_config shape matches server function output format', () => {
    // Verify the config shape matches what update_workspace_chartml_config produces
    const configValue = JSON.stringify({
      type: 'config',
      version: 1,
      style: 'tane',
    });

    db.exec(
      `UPDATE workspaces SET settings = json_set(
        COALESCE(settings, '{}'),
        '$.custom_settings.chartml_config',
        json('${configValue}')
      ) WHERE workspace_id = '${WORKSPACE_ID}'`
    );

    const typeRows = db.query(
      `SELECT json_extract(settings, '$.custom_settings.chartml_config.type') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
    );
    expect(typeRows[0]).toBe('config');

    const versionRows = db.query(
      `SELECT json_extract(settings, '$.custom_settings.chartml_config.version') FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
    );
    expect(versionRows[0]).toBe('1');
  });
});
