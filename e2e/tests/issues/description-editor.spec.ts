import { test, expect, type Page } from '@playwright/test';

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:3100';

test.describe('Issue Description Editor', () => {
  test('cursor position and content preserved after auto-save round-trip', async ({ page }) => {
    // Navigate to issues list and find or create an issue to test with
    await page.goto(`${BASE_URL}/issues`);
    await page.waitForLoadState('networkidle');

    // Find the first issue link and navigate to it
    const issueLink = page.locator('a[href*="/issues/"]').first();
    await expect(issueLink).toBeVisible({ timeout: 10_000 });
    await issueLink.click();
    await page.waitForLoadState('networkidle');

    // Find the description editor (contenteditable div inside the kode editor)
    const editor = page.locator('[contenteditable="true"]').first();
    await expect(editor).toBeVisible({ timeout: 10_000 });

    // Click into the editor to focus it
    await editor.click();
    await page.waitForTimeout(200);

    // Type a unique test string
    const testText = `cursor-test-${Date.now()}`;
    await page.keyboard.type(testText, { delay: 30 });

    // Record the content right after typing
    const contentAfterTyping = await editor.textContent();

    // Wait for the debounced auto-save (500ms) plus WebSocket round-trip
    await page.waitForTimeout(2000);

    // The content should still contain our test text (not reset by server round-trip)
    const contentAfterSave = await editor.textContent();
    expect(contentAfterSave).toContain(testText);

    // Now test that hitting Enter preserves the new line
    await editor.click();
    await page.keyboard.press('End'); // Go to end of current content

    await page.keyboard.press('Enter');
    const secondLine = `second-line-${Date.now()}`;
    await page.keyboard.type(secondLine, { delay: 30 });

    // Record content with both lines
    const contentWithNewLine = await editor.textContent();
    expect(contentWithNewLine).toContain(secondLine);

    // Wait for auto-save round-trip again
    await page.waitForTimeout(2000);

    // After save round-trip, both lines should still exist
    const finalContent = await editor.textContent();
    expect(finalContent).toContain(testText);
    expect(finalContent).toContain(secondLine);

    // Verify cursor is NOT at position 0,0 (top-left reset)
    // We do this by checking the editor still has focus and typing more
    const additionalText = '-still-focused';
    await page.keyboard.type(additionalText, { delay: 30 });
    const afterAdditionalTyping = await editor.textContent();
    // The additional text should appear after secondLine, not at the beginning
    expect(afterAdditionalTyping).toContain(secondLine + additionalText);
  });
});
