import { test, expect, chromium, type Browser, type BrowserContext, type Page } from '@playwright/test';
import * as crypto from 'crypto';

// Standalone two-window sync verification.
//
// This deliberately does NOT use the shared global-setup / test-helpers: both
// read `data/trakkt.db` with the `sqlite3` CLI, and this run is against
// Postgres. It does its own signup through the real UI instead.
//
// The scenario is the one originally reported: open a second window, and the
// two fall out of step. Every assertion below is about what the SECOND window
// shows without being reloaded.

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:3100';

// Fixed, not random: in self_hosted mode only the FIRST user may sign up
// ("Registration is closed" for the rest), so a random address makes the suite
// pass once and fail on every rerun. A fixed one lets the second run log in.
const EMAIL = 'sync-verify@example.com';
const PASSWORD = 'TestPassword1234';

let browser: Browser;
let ctxA: BrowserContext;
let ctxB: BrowserContext;
let pageA: Page;
let pageB: Page;

// Uncaught wasm panics seen since the last test boundary, per window.
//
// This suite originally asserted only on field values, and that is how it passed
// while /settings/workspace was panicking on every frame: the rename it checked
// for still arrived, because the panic was in the *previous* view's disposed
// render effect, not in the one being asserted on. A browser test that ignores
// the console is not browser verification, so every test here now fails on a
// panic regardless of what else it proves.
//
// Registered once per page in `beforeAll` and drained in `afterEach`, so a panic
// is attributed to the test that provoked it rather than to whichever one
// happens to run last.
const panics: string[] = [];

/** Fail the current test on any uncaught wasm panic in `page`. */
function watchForPanics(label: string, page: Page) {
  page.on('pageerror', (e) => {
    console.log(`[${label} pageerror] ${e.message}`);
    panics.push(`[${label} pageerror] ${e.message.split('\n')[0]}`);
  });
  page.on('console', (m) => {
    const text = m.text();
    if (m.type() === 'error') console.log(`[${label} console.error] ${text}`);
    // `panicked at` is the Rust panic hook; `unreachable` is the tachys
    // class-rendering cascade the disposed-value panic turns into.
    if (text.includes('panicked at') || text.includes('unreachable')) {
      panics.push(`[${label} console] ${text.split('\n')[0]}`);
    }
  });
}

test.describe.configure({ mode: 'serial' });

test.afterEach(async () => {
  const seen = panics.splice(0, panics.length);
  expect(seen, 'the app raised an uncaught wasm panic during this test').toEqual([]);
});

// Signup plus a cold WASM boot regularly exceeds the 30s default.
test.setTimeout(120_000);

test.beforeAll(async () => {
  browser = await chromium.launch();

  ctxA = await browser.newContext({ baseURL: BASE_URL });
  pageA = await ctxA.newPage();
  watchForPanics('A', pageA);
  // Surface which request 404s rather than leaving a bare console line.
  pageA.on('response', (r) => {
    if (r.status() >= 400) console.log(`[A HTTP ${r.status()}] ${r.url()}`);
  });

  // Try signup; fall back to login if this database already has its one user.
  await pageA.goto('/signup');
  await pageA.waitForSelector('#signup-email', { timeout: 30_000 });
  await pageA.waitForTimeout(1000); // let hydration settle before typing
  await pageA.fill('#signup-email', EMAIL);

  const nameInput = pageA.locator('#signup-name');
  if (await nameInput.isVisible({ timeout: 5000 }).catch(() => false)) {
    await nameInput.fill('Sync Tester');
  }
  await pageA.fill('#signup-password', PASSWORD);
  await pageA.locator('button[type="submit"]').click();

  const signedUp = await pageA
    .waitForURL((u) => !/\/(signup|login)/.test(u.pathname), { timeout: 20_000 })
    .then(() => true)
    .catch(() => false);

  if (!signedUp) {
    // Signup can be refused for more than one reason — "registration is
    // closed" once this database has its first user, or "already registered"
    // on a rerun against the same database. Both mean the same thing here:
    // the account exists, so log in instead.
    const notice = await pageA
      .locator('p, div')
      .filter({ hasText: /closed|already|exists|invalid/i })
      .first()
      .textContent({ timeout: 2000 })
      .catch(() => null);
    console.log(`[signup refused] notice=${JSON.stringify(notice)} — falling back to login`);

    await pageA.goto('/login');
    await pageA.waitForSelector('#login-email', { timeout: 30_000 });
    await pageA.waitForTimeout(1000);
    await pageA.fill('#login-email', EMAIL);
    await pageA.fill('#login-password', PASSWORD);
    await pageA.locator('button[type="submit"]').click();
    await pageA.waitForURL((u) => !/\/(signup|login)/.test(u.pathname), { timeout: 30_000 });
  }

  // Window B reuses A's session — the same user with two windows open, which
  // is exactly the reported scenario.
  const state = await ctxA.storageState();
  ctxB = await browser.newContext({ baseURL: BASE_URL, storageState: state });
  pageB = await ctxB.newPage();
  watchForPanics('B', pageB);

  // Anything raised during signup/login belongs to setup, not to the first
  // test — drop it so the first `afterEach` reports only what it provoked.
  // Setup failures surface as the setup itself failing.
  panics.length = 0;
});

test.afterAll(async () => {
  await browser?.close();
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

  await pageA.goto('/issues');
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
