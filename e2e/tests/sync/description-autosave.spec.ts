import { test, expect, type Page } from '@playwright/test';
import { assertDescriptionAutosavePreservesCaret } from '../../helpers/description-editor';
import { chmodSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { attachSyncProbe, createIssueViaUi, expectNoPanics, launchTwoClients, waitForSyncHandshake, type TwoClients } from './realtime-harness';

let clients: TwoClients;
const authState = join(tmpdir(), `tra-10104-auth-${new URL(process.env.BASE_URL ?? 'http://localhost:3100').port}.json`);
test.beforeAll(async () => {
  clients = await launchTwoClients(existsSync(authState) ? authState : undefined);
  await clients.ctxA.storageState({ path: authState });
  chmodSync(authState, 0o600);
});
test.afterEach(() => expectNoPanics(clients));
test.afterAll(async () => { await clients?.browser.close(); });


async function holdFirstDescriptionSave(page: Page) {
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  let arrived = false;
  let first = true;
  await page.route('**/leptos-api/update_issue*', async route => {
    if (!first) {
      await route.continue();
      return;
    }
    first = false;
    const response = await route.fetch();
    arrived = true;
    await held;
    await route.fulfill({ response });
  });
  return {
    release,
    waitForArrival: () => expect.poll(() => arrived, { timeout: 10_000 }).toBe(true),
    async cleanup() {
      release();
      await page.unroute('**/leptos-api/update_issue*');
    },
  };
}

test('an autosave acknowledgement preserves document identity, focus and the caret after a new issue identifier', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description caret ${Date.now()}`);
  await page.goto(href);
  await assertDescriptionAutosavePreservesCaret(page);
});

test('typing during a held save survives the acknowledgement and persists the newest draft', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description ordering ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  const held = await holdFirstDescriptionSave(page);
  try {
    await editor.click();
    await page.keyboard.type('First draft');
    await held.waitForArrival();
    await page.keyboard.type(' newest draft');
    const finalSave = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue')
      && r.request().postData()?.includes('newest'));
    held.release();
    expect((await finalSave).ok()).toBe(true);
    await expect(editor).toBeFocused();
    await expect(editor).toContainText('First draft newest draft');
    await page.keyboard.type(' continued');
    const continued = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue')
      && r.request().postData()?.includes('continued'));
    expect((await continued).ok()).toBe(true);
    await page.reload();
    await expect(editor).toContainText('First draft newest draft continued');
  } finally {
    await held.cleanup();
  }
});

test('a live description refresh preserves the comment composer DOM and selected range', async () => {
  const { pageA, pageB } = clients;
  const href = await createIssueViaUi(pageA, `Comment selection ${Date.now()}`);
  await pageA.goto(href);
  await pageB.goto(href);
  const composer = pageB.locator('[contenteditable="true"]').last();
  await expect(composer).toBeVisible();
  await composer.click();
  await pageB.keyboard.type('Comment draft');
  await pageB.keyboard.press('Shift+ArrowLeft');
  await pageB.keyboard.press('Shift+ArrowLeft');
  await composer.evaluate(element => {
    const s = getSelection();
    (window as any).__composerProbe = { element, anchor: s?.anchorNode, offset: s?.anchorOffset,
      focus: s?.focusNode, focusOffset: s?.focusOffset };
  });
  const descriptionA = pageA.locator('.markdown-description [contenteditable="true"]').first();
  await descriptionA.click();
  await pageA.keyboard.type('Remote description');
  await expect(pageB.locator('.markdown-description')).toContainText('Remote description');
  console.log("composer active element", await pageB.evaluate(() => document.activeElement?.outerHTML.slice(0, 200)));
  await expect(composer).toBeFocused();
  expect(await composer.evaluate(element => {
    const s = getSelection();
    const p = (window as any).__composerProbe;
    return element === p.element && s?.anchorNode === p.anchor && s?.anchorOffset === p.offset
      && s?.focusNode === p.focus && s?.focusOffset === p.focusOffset;
  })).toBe(true);
  await pageB.keyboard.type('X');
  await expect(composer).toContainText('Comment draX');
});


test('a clean focused description defers a remote document replacement until blur', async () => {
  const { pageA, pageB } = clients;
  const href = await createIssueViaUi(pageA, `Remote description ${Date.now()}`);
  const probe = attachSyncProbe(pageB);
  await pageA.goto(href);
  await pageB.goto(href);
  await waitForSyncHandshake(probe, 'B');
  const editorB = pageB.locator('.markdown-description [contenteditable="true"]').first();
  const editorA = pageA.locator('.markdown-description [contenteditable="true"]').first();
  await editorB.click();
  await editorB.evaluate(element => {
    const s = getSelection();
    (window as any).__remoteProbe = { element, anchor: s?.anchorNode, offset: s?.anchorOffset };
  });
  await editorA.click();
  await pageA.keyboard.type('Remote clean update');
  await expect.poll(() => probe.frames.some(f => f.dir === 'in' && f.payload.includes('Remote clean update'))).toBe(true);
  // Timeline is a separate consumer: seeing the activity proves the frame's
  // reactive work ran, while the focused document remains owned by the editor.
  await expect(pageB.getByText('updated the description').first()).toBeVisible();
  console.log("remote active element", await pageB.evaluate(() => document.activeElement?.outerHTML.slice(0, 200)));
  await expect(editorB).toBeFocused();
  expect(await editorB.evaluate(element => {
    const p = (window as any).__remoteProbe;
    const s = getSelection();
    return element === p.element && s?.anchorNode === p.anchor && s?.anchorOffset === p.offset;
  })).toBe(true);
  await pageB.keyboard.press('Tab');
  await expect(editorB).toContainText('Remote clean update');
});

for (const subIssue of [false, true]) {
  test(`${subIssue ? 'sub-issue' : 'new issue'} description retains DOM and selection across a live issue frame`, async () => {
    const { pageA, pageB } = clients;
    const href = await createIssueViaUi(pageA, `Modal parent ${Date.now()}`);
    const probe = attachSyncProbe(pageB);
    await pageB.goto(subIssue ? href : '/workspace');
    await waitForSyncHandshake(probe, 'B');
    await pageB.getByRole('button', { name: subIssue ? 'New sub-issue' : 'New Issue', exact: true }).first().click();
    await expect(pageB.locator('#issue-title')).toBeVisible();
    const modal = pageB.locator('[tabindex="-1"]').filter({ has: pageB.locator('#issue-title') });
    const editor = modal.locator('[contenteditable="true"]');
    await expect(editor).toBeVisible();
    await editor.click();
    await pageB.keyboard.type('Modal draft');
    await pageB.keyboard.press('ArrowLeft');
    await editor.evaluate(element => {
      const s = getSelection();
      (window as any).__modalProbe = { element, anchor: s?.anchorNode, offset: s?.anchorOffset };
    });
    const title = `Live while modal ${Date.now()}`;
    await createIssueViaUi(pageA, title);
    await expect.poll(() => probe.frames.some(f => f.dir === 'in' && f.payload.includes(title))).toBe(true);
    await expect(editor).toBeFocused();
    expect(await editor.evaluate(element => {
      const s = getSelection();
      const p = (window as any).__modalProbe;
      return element === p.element && s?.anchorNode === p.anchor && s?.anchorOffset === p.offset;
    })).toBe(true);
    await pageB.keyboard.type('X');
    await expect(editor).toContainText('Modal drafXt');
    await pageB.keyboard.press('Escape');
  });
}

test('a failed save retains the draft and an explicit retry persists it', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description failure ${Date.now()}`);
  await page.goto(href);
  let fail = true;
  await page.route('**/leptos-api/update_issue*', async route => {
    if (fail) {
      fail = false;
      await route.fulfill({ status: 503, body: 'Injected unavailable response' });
    } else {
      await route.continue();
    }
  });
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  try {
    await editor.click();
    await page.keyboard.type('Retained failure draft');
    await expect(page.getByRole('alert').filter({ hasText: 'Description not saved' })).toBeVisible();
    await expect(editor).toContainText('Retained failure draft');
    const saved = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue'));
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    expect((await saved).ok()).toBe(true);
    await page.reload();
    await expect(editor).toContainText('Retained failure draft');
  } finally {
    await page.unroute('**/leptos-api/update_issue*');
  }
});

test('a selected range survives autosave, and replacing it remains undoable', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description range ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  const held = await holdFirstDescriptionSave(page);
  try {
    await editor.click();
    const saved = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue'));
    await page.keyboard.type('One TRA-123 two');
    for (let index = 0; index < 3; index++) await page.keyboard.press('Shift+ArrowLeft');
    const selected = await page.evaluate(() => getSelection()?.toString());
    expect(selected).toBe('two');
    await held.waitForArrival();
    held.release();
    await (await saved).finished();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(editor).toBeFocused();
    expect(await page.evaluate(() => getSelection()?.toString())).toBe('two');
    await page.keyboard.type('X');
    await expect(editor).toContainText('One TRA-123 X');
    await page.keyboard.press('ControlOrMeta+z');
    await expect(editor).toContainText('One TRA-123 two');
    await page.keyboard.press('ControlOrMeta+Shift+z');
    await expect(editor).toContainText('One TRA-123 X');
  } finally {
    await held.cleanup();
  }
});

async function captureNextAnimationFrame(page: Page) {
  await page.evaluate(() => {
    const state = window as any;
    const request = window.requestAnimationFrame.bind(window);
    state.__heldEditorFrames = [];
    state.__editorFrameStacks = [];
    state.__restoreEditorRaf = () => { window.requestAnimationFrame = request; };
    window.requestAnimationFrame = callback => {
      state.__heldEditorFrames.push(callback);
      state.__editorFrameStacks.push(new Error('scheduled frame').stack);
      return -state.__heldEditorFrames.length;
    };
  });
}

async function expectCapturedFrames(page: Page) {
  await expect.poll(() => page.evaluate(() => (window as any).__heldEditorFrames.length), { timeout: 10_000 }).toBeGreaterThan(0);
}

async function releaseAnimationFrame(page: Page) {
  await expectCapturedFrames(page);
  console.log('held animation frame scheduling stacks', await page.evaluate(() => (window as any).__editorFrameStacks));
  await page.evaluate(() => {
    const state = window as any;
    state.__restoreEditorRaf();
    const callbacks = state.__heldEditorFrames.splice(0);
    for (const callback of callbacks) callback(performance.now());
  });
}

test('a queued editor restoration cannot reset a native input focused before its frame runs', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Queued restore ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  await editor.click();
  await page.keyboard.type('Document');
  await captureNextAnimationFrame(page);
  await page.keyboard.press('Enter');
  await expectCapturedFrames(page);
  await page.getByTitle('Click to edit title').click();
  const title = page.locator('input.font-display');
  await expect(title).toBeFocused();
  await title.press('End');
  await title.press('ArrowLeft');
  const selection = await title.evaluate((element: HTMLInputElement) => [element.selectionStart, element.selectionEnd]);
  await releaseAnimationFrame(page);
  await expect(title).toBeFocused();
  expect(await title.evaluate((element: HTMLInputElement) => [element.selectionStart, element.selectionEnd])).toEqual(selection);
  const value = await title.inputValue();
  await page.keyboard.type('Z');
  expect(await title.inputValue()).toBe(value.slice(0, selection[0]!) + 'Z' + value.slice(selection[1]!));
  await title.press('Escape');
});

test('disposing a modal editor before its queued restoration does not panic', async () => {
  const page = clients.pageA;
  await page.goto('/workspace');
  await page.getByRole('button', { name: 'New Issue', exact: true }).first().click();
  await expect(page.locator('#issue-title')).toBeVisible();
  const editor = page.locator('[contenteditable="true"]').last();
  await editor.click();
  await page.keyboard.type('Draft');
  await captureNextAnimationFrame(page);
  await page.keyboard.press('Enter');
  await expectCapturedFrames(page);
  await page.keyboard.press('Escape');
  await expect(page.locator('#issue-title')).toHaveCount(0);
  await releaseAnimationFrame(page);
});

test('composition starting after scheduling prevents an old selection restoration', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Composition restore ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  await editor.click();
  await page.keyboard.type('IME ');
  await captureNextAnimationFrame(page);
  await page.keyboard.press('Enter');
  await expectCapturedFrames(page);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Input.imeSetComposition', { text: 'に', selectionStart: 1, selectionEnd: 1 });
  await editor.evaluate(element => {
    const selection = getSelection();
    (window as any).__imeProbe = { element, node: selection?.anchorNode, offset: selection?.anchorOffset };
  });
  await releaseAnimationFrame(page);
  expect(await editor.evaluate(element => {
    const probe = (window as any).__imeProbe;
    const selection = getSelection();
    return document.activeElement === element && element === probe.element
      && selection?.anchorNode === probe.node && selection?.anchorOffset === probe.offset;
  })).toBe(true);
  await cdp.send('Input.insertText', { text: '日本語' });
  await expect(editor).toContainText('日本語');
  await cdp.detach();
});


test('an identifier blurred during an in-flight save is linked without stealing focus', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description blur links ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  const held = await holdFirstDescriptionSave(page);
  try {
    await editor.click();
    const saved = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue'));
    await page.keyboard.type('See TRA-123');
    const composer = page.locator('[contenteditable="true"]').last();
    await composer.click();
    await page.keyboard.type('Comment keeps focus');
    await held.waitForArrival();
    held.release();
    await (await saved).finished();
    await expect(editor.getByRole('link', { name: 'TRA-123' })).toHaveAttribute('href', '/issues/TRA-123');
    await expect(composer).toBeFocused();
    await page.keyboard.type(' continued');
    await expect(composer).toContainText('Comment keeps focus continued');
  } finally {
    await held.cleanup();
  }
});

test('empty blocks and list items retain the active caret across autosave', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description blocks ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  await editor.click();
  await page.keyboard.type('First paragraph');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await page.keyboard.type('- First item');
  await page.keyboard.press('Enter');
  const saved = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue')
    && r.request().postData()?.includes('Second'));
  await page.keyboard.type('Second item');
  await (await saved).finished();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect(editor).toBeFocused();
  await page.keyboard.type(' continued');
  await expect(editor).toContainText('Second item continued');
  await expect(editor.locator('li')).toHaveCount(2);
});

test('attachment completion preserves newer draft text and its next autosave preserves typing', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description attachment ${Date.now()}`);
  await page.goto(href);
  const editor = page.locator('.markdown-description [contenteditable="true"]').first();
  let release!: () => void;
  let intercepted!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  const arrived = new Promise<void>(resolve => { intercepted = resolve; });
  await page.route('**/api/v1/attachments?*', async route => {
    const response = await route.fetch();
    intercepted();
    await held;
    await route.fulfill({ response });
  });
  try {
    await editor.click();
    await page.keyboard.type('Attachment draft');
    await page.locator('input[type="file"]').first().setInputFiles({
      name: 'draft-note.txt', mimeType: 'text/plain', buffer: Buffer.from('Attachment regression fixture'),
    });
    await arrived;
    await page.keyboard.type(' newest text');
    release();
    await expect(editor).toContainText('draft-note.txt');
    await expect(editor).toContainText('Attachment draft newest text');
    await editor.click();
    await page.keyboard.press('ControlOrMeta+Home');
    const saved = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue')
      && r.request().postData()?.includes('Checked'));
    await page.keyboard.type('Checked ');
    await (await saved).finished();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(editor).toBeFocused();
    await page.keyboard.type('continued ');
    await expect(editor).toContainText('Checked continued Attachment draft newest text');
    const finalSave = page.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue')
      && r.request().postData()?.includes('continued'));
    await (await finalSave).finished();
    await page.reload();
    await expect(editor).toContainText('Checked continued Attachment draft newest text');
    await expect(editor).toContainText('draft-note.txt');
  } finally {
    release();
    await page.unroute('**/api/v1/attachments?*');
  }
});

test('an older IndexedDB description read cannot replace a newer completed read', async () => {
  const { pageA, pageB } = clients;
  const href = await createIssueViaUi(pageA, `Description read order ${Date.now()}`);
  await pageA.goto(href);
  const editorA = pageA.locator('.markdown-description [contenteditable="true"]').first();
  async function replaceDescription(text: string) {
    const saved = pageA.waitForResponse(r => new URL(r.url()).pathname.startsWith('/leptos-api/update_issue'));
    await editorA.click();
    await pageA.keyboard.press('ControlOrMeta+a');
    await pageA.keyboard.type(text);
    await (await saved).finished();
  }
  await replaceDescription('Read one');
  await pageB.addInitScript(() => {
    const state = window as any;
    const get = IDBObjectStore.prototype.get;
    const descriptor = Object.getOwnPropertyDescriptor(IDBRequest.prototype, 'onsuccess')!;
    IDBObjectStore.prototype.get = function (query) {
      const request = get.call(this, query);
      const key = query instanceof IDBKeyRange ? query.lower : query;
      (request as any).__descriptionRead = typeof key === 'string' && key.startsWith('issue_content\0');
      return request;
    };
    Object.defineProperty(IDBRequest.prototype, 'onsuccess', {
      ...descriptor,
      set(callback) {
        descriptor.set!.call(this, typeof callback !== 'function' ? callback : function (this: IDBRequest, event: Event) {
          if (state.__holdNextDescriptionRead && (this as any).__descriptionRead) {
            state.__holdNextDescriptionRead = false;
            const request = this;
            state.__releaseDescriptionRead = () => callback.call(request, event);
          } else {
            callback.call(this, event);
          }
        });
      },
    });
    state.__restoreDescriptionReadHook = () => {
      IDBObjectStore.prototype.get = get;
      Object.defineProperty(IDBRequest.prototype, 'onsuccess', descriptor);
    };
  });
  await pageB.goto(href);
  const editorB = pageB.locator('.markdown-description [contenteditable="true"]').first();
  await expect(editorB).toContainText('Read one');
  try {
    await pageB.evaluate(() => { (window as any).__holdNextDescriptionRead = true; });
    await replaceDescription('Read two');
    await expect.poll(() => pageB.evaluate(() => typeof (window as any).__releaseDescriptionRead), { timeout: 10_000 }).toBe('function');
    await replaceDescription('Read three');
    await expect(editorB).toContainText('Read three');
    await pageB.evaluate(() => {
      const state = window as any;
      state.__releaseDescriptionRead();
      state.__releaseDescriptionRead = undefined;
    });
    await pageB.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(editorB).toContainText('Read three');
  } finally {
    await pageB.evaluate(() => {
      const state = window as any;
      state.__releaseDescriptionRead?.();
      state.__releaseDescriptionRead = undefined;
      state.__restoreDescriptionReadHook?.();
    });
  }
});

test('floating toolbar formatting keeps its selection and continued typing across autosave', async () => {
  const page = clients.pageA;
  const href = await createIssueViaUi(page, `Description toolbar ${Date.now()}`);
  await page.goto(href);
  const section = page.locator('.markdown-description').first();
  const editor = section.locator('[contenteditable="true"]');
  await editor.click();
  const plainSaved = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue'));
  await page.keyboard.type('Toolbar words');
  await (await plainSaved).finished();
  await page.keyboard.press('ControlOrMeta+a');
  const formatted = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue'));
  await section.getByTitle('Bold (Ctrl+B)', { exact: true }).click();
  await expect(editor.locator('strong')).toHaveText('Toolbar words');
  await (await formatted).finished();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect(editor).toBeFocused();
  expect(await page.evaluate(() => getSelection()?.toString())).toBe('Toolbar words');
  // Replace the preserved range directly; end-of-document ArrowRight navigation is tracked in TRA-10105.
  const continued = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_issue')
    && response.request().postData()?.includes('Replacement'));
  await page.keyboard.type('Replacement words');
  await (await continued).finished();
  await expect(editor.locator('strong')).toHaveText('Replacement words');
  await page.reload();
  await expect(editor.locator('strong')).toHaveText('Replacement words');
});
