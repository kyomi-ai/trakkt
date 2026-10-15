import { expect, type Locator, type Page } from '@playwright/test';

async function rememberSelection(editor: Locator) {
  await editor.evaluate(element => {
    const selection = getSelection();
    (window as any).__descriptionProbe = {
      element, anchor: selection?.anchorNode, offset: selection?.anchorOffset,
      focus: selection?.focusNode, focusOffset: selection?.focusOffset,
    };
  });
}

async function expectSameSelection(editor: Locator) {
  const probe = await editor.evaluate(element => {
    const selection = getSelection();
    const before = (window as any).__descriptionProbe;
    return {
      sameElement: element === before.element,
      focused: document.activeElement === element,
      sameSelection: selection?.anchorNode === before.anchor && selection?.anchorOffset === before.offset
        && selection?.focusNode === before.focus && selection?.focusOffset === before.focusOffset,
      text: element.textContent,
      activeElement: document.activeElement?.tagName,
      selection: { anchor: selection?.anchorOffset, focus: selection?.focusOffset },
    };
  });
  console.log('autosave probe', JSON.stringify(probe));
  expect(probe.sameElement).toBe(true);
  expect(probe.focused).toBe(true);
  expect(probe.sameSelection).toBe(true);
}

async function settleResponse(page: Page, response: import('@playwright/test').Response) {
  expect(response.ok()).toBe(true);
  await response.finished();
  // Run the response continuation and resulting DOM/selection effects.
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

/** Starts on a blank issue description; every continuation uses keyboard input without refocusing. */
export async function assertDescriptionAutosavePreservesCaret(page: Page) {
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.click();
  const saved = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue'));
  await page.keyboard.type('Before TRA-123 after');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft');
  await rememberSelection(editor);
  await settleResponse(page, await saved);
  await expectSameSelection(editor);
  const inserted = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue'));
  await page.keyboard.type('X');
  await expect(editor).toContainText('Before TRA-123 aftXer');
  await settleResponse(page, await inserted);

  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  const secondLine = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue')
    && response.request().postData()?.includes('Second'));
  await page.keyboard.type('Second line');
  await rememberSelection(editor);
  await settleResponse(page, await secondLine);
  await expectSameSelection(editor);
  const continued = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue')
    && response.request().postData()?.includes('continued'));
  await page.keyboard.type(' continued');
  await settleResponse(page, await continued);
  await expect(editor).toContainText('Second line continued');
}
