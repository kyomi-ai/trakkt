import { test, expect, type Page, type Locator } from '@playwright/test';
import { createIssueViaUi, expectNoPanics, launchTwoClients, type TwoClients } from './realtime-harness';

let clients: TwoClients;
test.setTimeout(180_000);
test.beforeAll(async () => { clients = await launchTwoClients(process.env.AUTH_STORAGE_STATE); });
test.afterEach(() => expectNoPanics(clients));
test.afterAll(async () => { await clients?.browser.close(); });

async function rememberSelection(editor: Locator) {
  await editor.evaluate(element => {
    if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
      (window as any).__focusSmoke = {
        element,
        start: element.selectionStart,
        end: element.selectionEnd,
        direction: element.selectionDirection,
      };
      return;
    }
    const selection = getSelection();
    (window as any).__focusSmoke = {
      element,
      anchor: selection?.anchorNode,
      anchorOffset: selection?.anchorOffset,
      focus: selection?.focusNode,
      focusOffset: selection?.focusOffset,
    };
  });
}

async function expectSameFocusedSelection(editor: Locator) {
  const result = await editor.evaluate(element => {
    const before = (window as any).__focusSmoke;
    if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
      return {
        sameElement: element === before.element,
        focused: document.activeElement === element,
        sameSelection: element.selectionStart === before.start
          && element.selectionEnd === before.end
          && element.selectionDirection === before.direction,
      };
    }
    const selection = getSelection();
    return {
      sameElement: element === before.element,
      focused: document.activeElement === element,
      sameSelection: selection?.anchorNode === before.anchor
        && selection?.anchorOffset === before.anchorOffset
        && selection?.focusNode === before.focus
        && selection?.focusOffset === before.focusOffset,
    };
  });
  expect(result).toEqual({ sameElement: true, focused: true, sameSelection: true });
}

async function typeAndRemember(editor: Locator, value: string) {
  await editor.click();
  await editor.pressSequentially(value);
  await editor.evaluate(element => {
    if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
      const caret = Math.max(0, (element.selectionStart ?? 0) - 2);
      element.setSelectionRange(caret, caret);
      return;
    }
    const selection = getSelection();
    selection?.collapse(selection.anchorNode, Math.max(0, (selection.anchorOffset ?? 0) - 2));
  });
  await rememberSelection(editor);
}

test('project creation description stays focused and editable during idle', async () => {
  const { pageA } = clients;
  await pageA.goto('/projects');
  await pageA.getByRole('button', { name: 'New Project' }).first().click();
  const description = pageA.getByPlaceholder('What is this project about?');
  await expect(description).toBeVisible();
  await typeAndRemember(description, 'Project draft survives idle');
  await pageA.waitForTimeout(1200);
  await expectSameFocusedSelection(description);
  await pageA.keyboard.type(' typing');
  await expect(description).toHaveValue('Project draft survives id typingle');
});

test('feedback description stays focused and editable during idle', async () => {
  const { pageA } = clients;
  await pageA.goto('/workspace');
  await pageA.getByText('Sync Tester', { exact: true }).locator('xpath=ancestor::button').click();
  await pageA.getByRole('button', { name: 'Send Feedback' }).click();
  const description = pageA.locator('textarea[placeholder="What happened? What did you expect to happen?"]');
  await expect(description).toBeVisible();
  await typeAndRemember(description, 'Feedback draft survives idle');
  await pageA.waitForTimeout(1200);
  await expectSameFocusedSelection(description);
  await pageA.keyboard.type(' typing');
  await expect(description).toHaveValue('Feedback draft survives id typingle');
});

test('inline issue title preserves focus, selection, and typing across unrelated live update', async () => {
  const { pageA, pageB } = clients;
  const href = await createIssueViaUi(pageA, `Title focus ${Date.now()}`);
  await pageA.goto(href);
  await pageB.goto(href);
  await pageA.locator('h1[title="Click to edit title"]').click();
  const title = pageA.locator('input:focus');
  await expect(title).toBeVisible();
  await title.pressSequentially(' changed');
  await title.evaluate(element => {
    const input = element as HTMLInputElement;
    input.setSelectionRange(input.value.length - 3, input.value.length - 3);
  });
  await rememberSelection(title);
  const titleBeforeLiveUpdate = await title.evaluate(element => {
    const input = element as HTMLInputElement;
    return { value: input.value, start: input.selectionStart, end: input.selectionEnd, direction: input.selectionDirection };
  });

  // A comment is an unrelated issue update delivered over the existing live-sync path.
  const composer = pageB.locator('[contenteditable="true"]').last();
  await expect(composer).toBeVisible();
  await composer.click();
  await pageB.keyboard.type(`unrelated update ${Date.now()}`);
  await pageB.getByRole('button', { name: 'Comment', exact: true }).click();
  await expect(pageA.getByText(/unrelated update/)).toBeVisible({ timeout: 30_000 });

  // Title commits on blur, so verify the active edit before committing it.
  const titleAfterLiveUpdate = await title.evaluate(element => {
    const input = element as HTMLInputElement;
    return { value: input.value, start: input.selectionStart, end: input.selectionEnd, direction: input.selectionDirection };
  });
  console.log('inline title selection across live update', JSON.stringify({ before: titleBeforeLiveUpdate, after: titleAfterLiveUpdate }));
  await expectSameFocusedSelection(title);
  await pageA.keyboard.type('X');
  const expectedAfterTyping = titleBeforeLiveUpdate.value.slice(0, titleBeforeLiveUpdate.start ?? 0)
    + 'X' + titleBeforeLiveUpdate.value.slice(titleBeforeLiveUpdate.start ?? 0);
  await expect(title).toHaveValue(expectedAfterTyping);
  await title.press('Enter');
});
