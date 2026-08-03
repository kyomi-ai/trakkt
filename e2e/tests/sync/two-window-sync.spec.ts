import { test, expect, type Page } from '@playwright/test';
import * as crypto from 'crypto';
import { expectNoPanics, launchTwoClients, type TwoClients } from './realtime-harness';

// Standalone two-window sync verification.
//
// The scenario is the one originally reported: open a second window, and the
// two fall out of step. Every assertion below is about what the SECOND window
// shows without being reloaded.
//
// Setup — the Postgres-safe signup/login and the panic watcher, with the
// reasoning for both — lives in `./realtime-harness`, shared with the
// milestone realtime suite.

let clients: TwoClients;
let pageA: Page;
let pageB: Page;

test.describe.configure({ mode: 'serial' });

// Panics are drained per test, so one is attributed to the test that provoked
// it rather than to whichever one happens to run last.
test.afterEach(async () => {
  expectNoPanics(clients);
});

// Signup plus a cold WASM boot regularly exceeds the 30s default.
test.setTimeout(120_000);

test.beforeAll(async () => {
  clients = await launchTwoClients();
  pageA = clients.pageA;
  pageB = clients.pageB;
});

test.afterAll(async () => {
  await clients?.browser?.close();
});

test('both windows load the app authenticated', async () => {
  await pageB.goto('/');
  await pageB.waitForLoadState('networkidle');

  // Neither window bounced back to an auth page.
  expect(new URL(pageA.url()).pathname).not.toMatch(/\/(signup|login)/);
  expect(new URL(pageB.url()).pathname).not.toMatch(/\/(signup|login)/);
});

test('the app opens its own websocket in both windows', async () => {
  // The whole sync series is downstream of this. If the socket is not
  // connecting, every later assertion is meaningless and should be read as
  // such.
  //
  // This instruments the APP's socket rather than opening one of our own:
  // the server route is `/ws/{user_id}`, so a hand-rolled probe would only
  // prove the server accepts a URL we constructed, not that the client
  // connects. Wrapping the constructor records what the app actually does.
  for (const [label, page] of [['A', pageA], ['B', pageB]] as const) {
    await page.addInitScript(() => {
      (window as any).__wsLog = [];
      const Real = window.WebSocket;
      const Wrapped: any = function (url: string, protocols?: any) {
        const sock = protocols === undefined ? new Real(url) : new Real(url, protocols);
        const entry = { url: String(url), opened: false };
        (window as any).__wsLog.push(entry);
        sock.addEventListener('open', () => { entry.opened = true; });
        return sock;
      };
      Wrapped.prototype = Real.prototype;
      Wrapped.OPEN = Real.OPEN;
      Wrapped.CLOSED = Real.CLOSED;
      Wrapped.CONNECTING = Real.CONNECTING;
      Wrapped.CLOSING = Real.CLOSING;
      (window as any).WebSocket = Wrapped;
    });

    await page.goto('/');
    await page.waitForLoadState('networkidle');

    await expect
      .poll(
        async () => await page.evaluate(() => ((window as any).__wsLog ?? []).some((e: any) => e.opened)),
        { timeout: 25_000, message: `window ${label}: the app never opened a websocket` },
      )
      .toBe(true);

    const log = await page.evaluate(() => (window as any).__wsLog);
    console.log(`[window ${label}] sockets: ${JSON.stringify(log)}`);
  }
});

test('a workspace rename in window A reaches window B without a reload', async () => {
  const newName = `Renamed ${crypto.randomBytes(3).toString('hex')}`;

  await pageA.goto('/settings/workspace');
  await pageA.waitForLoadState('networkidle');

  const nameField = pageA.locator('input').first();
  await expect(nameField).toBeVisible({ timeout: 15_000 });

  await pageB.goto('/settings/workspace');
  await pageB.waitForLoadState('networkidle');
  const nameFieldB = pageB.locator('input').first();
  await expect(nameFieldB).toBeVisible({ timeout: 15_000 });

  await nameField.fill(newName);
  // The card has no save button: it commits on blur, via the `on_blur` handler
  // at workspace.rs:222, bound to the input at :252. Pressing Enter does
  // nothing, so the field must actually lose focus.
  await nameField.blur();

  // B must pick this up from the live frame, with no navigation.
  await expect(nameFieldB).toHaveValue(newName, { timeout: 20_000 });
});

test("an activity in window A reaches window B's issue timeline without a reload", async () => {
  // The reported bug: comment on an issue or change a field, and a colleague
  // with that issue open sees nothing until they reload. Both activity write
  // sites logged their sync entry with `None`, and `cache/apply.rs` drops a
  // data-less insert/update before it reaches the arm that bumps the timeline's
  // refetch counter — so every ACTIVITY frame was discarded on arrival.
  //
  // The assertion is deliberately on an ACTIVITY row rather than on the issue
  // field itself. `IssueTimeline` hands `(team_key, number, activities_version)`
  // to its `Resource`, and `IssueDetailContent` is keyed on `(team_key, number)`
  // by the `<For>` that renders it, so the `issue` frame this same edit emits
  // updates the header and the description without rebuilding the timeline. The
  // activity counter is the only path a new timeline row has.
  const issueTitle = `Sync activity ${crypto.randomBytes(3).toString('hex')}`;

  // `/workspace`, not `/issues`. This case was added with TRA-9992 and never
  // executed; its first run, under TRA-9964, showed `/issues` redirects to
  // `/my-issues` — a list of what you are assigned or watching, which has no
  // create button at all. `/workspace` is the workspace-wide issue list.
  await pageA.goto('/workspace');
  await pageA.waitForLoadState('networkidle');

  // A fresh workspace shows the empty state, which carries its own "New Issue"
  // button alongside the header's — take the first either way.
  await pageA.getByRole('button', { name: 'New Issue' }).first().click();
  await pageA.waitForSelector('#issue-title', { timeout: 15_000 });
  await pageA.fill('#issue-title', issueTitle);
  await pageA.getByRole('button', { name: 'Create Issue' }).click();

  const createdRow = pageA.locator('a[href*="/issues/"]').filter({ hasText: issueTitle }).first();
  await expect(createdRow).toBeVisible({ timeout: 20_000 });
  const issueHref = await createdRow.getAttribute('href');
  expect(issueHref, 'the new issue needs a detail link both windows can open').toBeTruthy();

  await pageA.goto(issueHref!);
  await pageA.waitForLoadState('networkidle');
  await pageB.goto(issueHref!);
  await pageB.waitForLoadState('networkidle');

  // B is on the issue and has the creation activity — so anything missing later
  // is a missing update, not a timeline that never rendered.
  await expect(pageB.getByText('created this issue').first()).toBeVisible({ timeout: 20_000 });
  await expect(pageB.getByText('updated the description')).toHaveCount(0);

  // A edits the description. The debounced auto-save records a
  // `description_changed` activity through `coalesce_or_insert_activity`.
  const editorA = pageA.locator('[contenteditable="true"]').first();
  await expect(editorA).toBeVisible({ timeout: 15_000 });
  await editorA.click();
  await pageA.waitForTimeout(200);
  await pageA.keyboard.type(`Edited by A ${crypto.randomBytes(3).toString('hex')}`, { delay: 30 });

  // B must pick this up from the live frame, with no navigation.
  await expect(pageB.getByText('updated the description').first()).toBeVisible({ timeout: 30_000 });
});

test('typing in window B is not destroyed by a frame arriving from window A', async () => {
  // This is the regression TRA-9977 introduced and then fixed. It is the one
  // assertion here that is about data loss rather than staleness.
  const typed = `Do not clobber ${crypto.randomBytes(3).toString('hex')}`;
  const fromA = `From A ${crypto.randomBytes(3).toString('hex')}`;

  await pageB.goto('/settings/workspace');
  await pageB.waitForLoadState('networkidle');
  const nameFieldB = pageB.locator('input').first();
  await expect(nameFieldB).toBeVisible({ timeout: 15_000 });

  // B starts editing and does NOT save.
  await nameFieldB.click();
  await nameFieldB.fill(typed);

  // Meanwhile A saves a different name, producing a frame B will receive.
  await pageA.goto('/settings/workspace');
  await pageA.waitForLoadState('networkidle');
  const nameFieldA = pageA.locator('input').first();
  await expect(nameFieldA).toBeVisible({ timeout: 15_000 });
  await nameFieldA.fill(fromA);
  // Commits on blur — see the note in the previous test.
  await nameFieldA.blur();

  // Give the frame time to land in B.
  await pageB.waitForTimeout(6000);

  // B's in-progress text must survive.
  await expect(nameFieldB).toHaveValue(typed);
});
