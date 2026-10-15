import { test } from '@playwright/test';
import { assertDescriptionAutosavePreservesCaret } from '../../helpers/description-editor';
import { createIssueViaUi } from '../sync/realtime-harness';

test.describe('Issue Description Editor', () => {
  test('cursor position and content preserved after auto-save round-trip', async ({ page }) => {
    const href = await createIssueViaUi(page, `Description caret ${Date.now()}`);
    await page.goto(href);
    await assertDescriptionAutosavePreservesCaret(page);
  });
});
