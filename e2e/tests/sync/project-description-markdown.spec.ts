import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { randomBytes } from 'node:crypto';
import {
  attachSyncProbe,
  expectNoPanics,
  launchTwoClients,
  waitForSyncHandshake,
  type TwoClients,
} from './realtime-harness';

// TRA-10075: normal batch QA, including stored Markdown from the creation
// textarea, shared kode rendering, explicit persistence and editor lifecycle.
const markdown = readFileSync(resolve(__dirname,
  '../../../crates/trakkt-ui/tests/fixtures/project-description.md'), 'utf8');
let clients: TwoClients;
let projectHref: string;
const description = (page: Page) => page.getByRole('region', { name: 'Project description' });

test.setTimeout(180_000);
test.describe.configure({ mode: 'serial' });
test.beforeAll(async () => {
  clients = await launchTwoClients();
  const { pageA } = clients;
  await pageA.goto('/projects');
  await pageA.getByRole('button', { name: 'New Project' }).first().click();
  await pageA.getByPlaceholder('e.g. Q3 Launch').fill(`Markdown ${randomBytes(4).toString('hex')}`);
  await pageA.getByPlaceholder('What is this project about?').fill(markdown);
  await pageA.getByRole('button', { name: 'Create Project' }).click();
  await pageA.waitForURL(/\/projects\/[^/]+$/);
  projectHref = new URL(pageA.url()).pathname;
});
test.afterEach(() => expectNoPanics(clients));
test.afterAll(async () => { await clients?.browser.close(); });

async function expectFormatting(page: Page) {
  const section = description(page);
  await expect(section.getByRole('heading', { name: 'State beyond RAM' })).toBeVisible();
  await expect(section.locator('strong')).toHaveText('durable state');
  await expect(section.locator('em')).toHaveText('bounded memory');
  await expect(section.locator('ul li')).toHaveCount(2);
  await expect(section.locator('ol li')).toHaveCount(2);
  await expect(section.locator('table')).toContainText('Retention');
  await expect(section.locator('pre code')).toContainText('restore(checkpoint);');
  await expect(section.getByRole('link', { name: 'Storage guide' })).toHaveAttribute('href', 'https://example.com/storage');
}

test('stored Markdown renders, edits save across clients and reload, and cancel discards changes', async () => {
  const { pageA, pageB } = clients;
  const probeA = attachSyncProbe(pageA);
  const probeB = attachSyncProbe(pageB);
  await pageA.goto(projectHref);
  await pageB.goto(projectHref);
  await expectFormatting(pageA);
  await expectFormatting(pageB);
  await waitForSyncHandshake(probeA, 'A');
  await waitForSyncHandshake(probeB, 'B');

  const sectionA = description(pageA);
  // Link activation must not enter edit mode, even when links open a new tab.
  await clients.ctxA.route('https://example.com/storage', route => route.fulfill({ body: 'Storage guide' }));
  await sectionA.getByRole('link', { name: 'Storage guide' }).click();
  if (new URL(pageA.url()).pathname !== projectHref) await pageA.goBack();
  await expect(sectionA.locator('[contenteditable="true"]')).toHaveCount(0);

  await sectionA.getByRole('button', { name: 'Edit description' }).click();
  const editor = sectionA.locator('[contenteditable="true"]');
  await expect(editor).toBeFocused();
  await editor.press('ControlOrMeta+End');
  await editor.press('Enter');
  await editor.pressSequentially('Saved project note');
  const saved = pageA.waitForResponse(response => new URL(response.url()).pathname.startsWith('/leptos-api/update_project'));
  await sectionA.getByRole('button', { name: 'Save', exact: true }).click();
  expect((await saved).ok()).toBe(true);
  await expect(description(pageB)).toContainText('Saved project note');
  await pageA.reload();
  await expectFormatting(pageA);
  await expect(sectionA).toContainText('Saved project note');
  await sectionA.getByRole('button', { name: 'Edit description' }).click();
  await editor.press('ControlOrMeta+End');
  await editor.pressSequentially(' discard me');
  await sectionA.getByRole('button', { name: 'Cancel' }).click();
  await expect(sectionA).not.toContainText('discard me');
  await pageA.reload();
  await expect(sectionA).not.toContainText('discard me');

  // Wide tables/code must stay readable without widening the page; both themes
  // use the same token based renderer.
  await pageA.setViewportSize({ width: 375, height: 800 });
  for (const dark of [false, true]) {
    await pageA.evaluate(dark => document.documentElement.classList.toggle('dark', dark), dark);
    await expectFormatting(pageA);
    const bounds = await pageA.evaluate(() => ({ width: innerWidth, scroll: document.documentElement.scrollWidth }));
    expect(bounds.scroll).toBeLessThanOrEqual(bounds.width + 1);
  }
});

test('unrelated milestone live updates preserve the active draft DOM and caret', async () => {
  const { pageA, pageB } = clients;
  const probeA = attachSyncProbe(pageA);
  const probeB = attachSyncProbe(pageB);
  await pageA.goto(projectHref);
  await pageB.goto(projectHref);
  await waitForSyncHandshake(probeA, 'A');
  await waitForSyncHandshake(probeB, 'B');
  const sectionB = description(pageB);
  await sectionB.getByRole('button', { name: 'Edit description' }).click();
  const editor = sectionB.locator('[contenteditable="true"]');
  await editor.press('ControlOrMeta+End');
  await editor.pressSequentially(' active unsaved draft');
  await editor.evaluate(element => {
    (window as any).__descriptionEditor = element;
    const selection = getSelection();
    (window as any).__descriptionCaret = { node: selection?.anchorNode, offset: selection?.anchorOffset };
  });
  const milestone = `Keep draft ${randomBytes(4).toString('hex')}`;
  await pageA.getByRole('button', { name: 'Add milestone' }).click();
  await pageA.getByPlaceholder('Milestone name').fill(milestone);
  await pageA.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(pageB.getByText(milestone, { exact: true })).toBeVisible({ timeout: 30_000 });
  await expect(editor).toBeFocused();
  await expect(editor).toContainText('active unsaved draft');
  expect(await editor.evaluate(element => {
    const selection = getSelection();
    const state = window as any;
    return element === state.__descriptionEditor
      && selection?.anchorNode === state.__descriptionCaret.node
      && selection?.anchorOffset === state.__descriptionCaret.offset;
  })).toBe(true);
  await sectionB.getByRole('button', { name: 'Cancel' }).click();
});
