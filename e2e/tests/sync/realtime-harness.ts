import { expect, chromium, type Browser, type BrowserContext, type Page } from '@playwright/test';

// Shared harness for the two-client realtime suites.
//
// Extracted from `two-window-sync.spec.ts` when TRA-9964 added a second spec
// that needs exactly the same setup; the signup/login and panic-watching code
// below is that file's, moved rather than rewritten.
//
// This deliberately does NOT use the shared global-setup / test-helpers: both
// read `data/trakkt.db` with the `sqlite3` CLI, and these runs are against
// Postgres. It does its own signup through the real UI instead.

export const BASE_URL = process.env.BASE_URL ?? 'http://localhost:3100';

// Fixed, not random: in self_hosted mode only the FIRST user may sign up
// ("Registration is closed" for the rest), so a random address makes the suite
// pass once and fail on every rerun. A fixed one lets the second run log in.
export const EMAIL = 'sync-verify@example.com';
export const PASSWORD = 'TestPassword1234';

/** Two browser contexts sharing one logged-in session, plus their panic log. */
export interface TwoClients {
  browser: Browser;
  ctxA: BrowserContext;
  ctxB: BrowserContext;
  pageA: Page;
  pageB: Page;
  /** Uncaught wasm panics seen since the last drain, across both windows. */
  panics: string[];
}

/** Fail the current test on any uncaught wasm panic in `page`. */
export function watchForPanics(label: string, page: Page, panics: string[]) {
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

/**
 * Assert no uncaught wasm panic since the last call, and reset the log.
 *
 * A suite that asserts only on field values passes while the app panics on
 * every frame — the panic lands in the *previous* view's disposed render
 * effect, not in the one being asserted on. A browser test that ignores the
 * console is not browser verification, so every test using this harness fails
 * on a panic regardless of what else it proves.
 */
export function expectNoPanics(clients: TwoClients) {
  const seen = clients.panics.splice(0, clients.panics.length);
  expect(seen, 'the app raised an uncaught wasm panic during this test').toEqual([]);
}

/**
 * Launch two browser contexts logged in as the same user.
 *
 * Window B reuses A's storage state — the same user with two windows open,
 * which is the scenario the whole sync epic is about. Separate contexts, not
 * two pages in one: `tab_leader.rs` elects a single socket-owning tab per
 * origin per profile, so two pages in one context would give one real
 * WebSocket and one BroadcastChannel follower.
 */
export async function launchTwoClients(): Promise<TwoClients> {
  const panics: string[] = [];
  const browser = await chromium.launch();

  const ctxA = await browser.newContext({ baseURL: BASE_URL });
  const pageA = await ctxA.newPage();
  watchForPanics('A', pageA, panics);
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

  const state = await ctxA.storageState();
  const ctxB = await browser.newContext({ baseURL: BASE_URL, storageState: state });
  const pageB = await ctxB.newPage();
  watchForPanics('B', pageB, panics);

  // Anything raised during signup/login belongs to setup, not to the first
  // test — drop it so the first `expectNoPanics` reports only what it
  // provoked. Setup failures surface as the setup itself failing.
  panics.length = 0;

  return { browser, ctxA, ctxB, pageA, pageB, panics };
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures created through the real UI
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Create an issue from `page` through the real UI and return its detail path.
 *
 * Extracted under TRA-9997, when a fourth copy of this block was about to be
 * written. It was already inline in three suites — `two-window-sync.spec.ts`,
 * `milestone-realtime.spec.ts` and, wrapped in project assignment,
 * `project-view-state.spec.ts` — with the same six selectors and the same
 * comment about `/issues` in each.
 *
 * `/workspace`, not `/issues`: the latter redirects to `/my-issues`, which lists
 * only what you are assigned or watching and carries no create button at all.
 * That was found the hard way — the case in `two-window-sync.spec.ts` was
 * written against `/issues` under TRA-9992 and never executed until TRA-9964.
 *
 * The one thing that is not identical across the three call sites is the wait
 * for the created row: 20s in `two-window-sync.spec.ts`, 30s in the other two.
 * This uses 30s, so that suite now waits 10s longer before giving up. It cannot
 * turn a pass into a failure, only the reverse, and picking the shorter number
 * to preserve it exactly would make the other two suites *less* tolerant than
 * they were written to be.
 */
export async function createIssueViaUi(page: Page, title: string): Promise<string> {
  await page.goto('/workspace');
  await page.waitForLoadState('networkidle');

  // A fresh workspace shows the empty state, which carries its own "New Issue"
  // button alongside the header's — take the first either way.
  await page.getByRole('button', { name: 'New Issue' }).first().click();
  await page.waitForSelector('#issue-title', { timeout: 15_000 });
  await page.fill('#issue-title', title);
  await page.getByRole('button', { name: 'Create Issue' }).click();

  const createdRow = page.locator('a[href*="/issues/"]').filter({ hasText: title }).first();
  await expect(createdRow).toBeVisible({ timeout: 30_000 });
  const href = await createdRow.getAttribute('href');
  expect(href, 'the new issue needs a detail link the test can open').toBeTruthy();
  return href!;
}

// ─────────────────────────────────────────────────────────────────────────────
// Sync probe
// ─────────────────────────────────────────────────────────────────────────────

/** One WebSocket frame, in whichever direction, with the time it crossed. */
export interface FrameRecord {
  t: number;
  /** Index of the socket it crossed — a reconnect gets a new number. */
  socket: number;
  dir: 'in' | 'out';
  payload: string;
}

/**
 * Everything about a page that could explain why it re-read data.
 *
 * The point of recording all of it is that "B eventually showed the milestone"
 * is not evidence of anything on its own — a reload, a refetch-on-focus or a
 * poll would produce the same screen. These make the mechanism checkable.
 *
 * All of it is observed from the Playwright side (CDP events), not injected
 * into the page, so it stays true regardless of which JS API the wasm bundle
 * happens to use to issue a request.
 */
export interface SyncProbe {
  frames: FrameRecord[];
  /** Sockets opened by the page, in order, with their close time if closed. */
  sockets: { t: number; url: string; closedAt: number | null }[];
  /** Requests the page issued to a Leptos server function. */
  serverFnCalls: { t: number; fn: string }[];
  /** Main-frame navigations, i.e. every document this tab has loaded. */
  navigations: { t: number; url: string }[];
}

/** A point in time to measure a probe's records against. */
export interface ProbeMark {
  t: number;
}

/**
 * Start recording sockets, server-function calls and navigations for `page`.
 *
 * Must be attached before the page navigates: `page.on('websocket')` only
 * reports sockets opened after it is registered.
 */
export function attachSyncProbe(page: Page): SyncProbe {
  const probe: SyncProbe = { frames: [], sockets: [], serverFnCalls: [], navigations: [] };

  page.on('websocket', (ws) => {
    const index = probe.sockets.length;
    const record = { t: Date.now(), url: ws.url(), closedAt: null as number | null };
    probe.sockets.push(record);
    ws.on('framereceived', (f) => {
      probe.frames.push({ t: Date.now(), socket: index, dir: 'in', payload: String(f.payload) });
    });
    ws.on('framesent', (f) => {
      probe.frames.push({ t: Date.now(), socket: index, dir: 'out', payload: String(f.payload) });
    });
    ws.on('close', () => {
      record.closedAt = Date.now();
    });
  });

  page.on('request', (r) => {
    const url = new URL(r.url());
    if (!url.pathname.startsWith('/leptos-api/')) return;
    probe.serverFnCalls.push({ t: Date.now(), fn: url.pathname.slice('/leptos-api/'.length) });
  });

  // `page.on('request')` reports the URL as sent, so the recorded name is
  // whatever `#[server(prefix = "/leptos-api")]` produced — see `isServerFn`.

  page.on('framenavigated', (frame) => {
    if (frame !== page.mainFrame()) return;
    probe.navigations.push({ t: Date.now(), url: frame.url() });
  });

  return probe;
}

// ─────────────────────────────────────────────────────────────────────────────
// Simulating an outage
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Give `page` a way to have its live WebSockets terminated from the test.
 *
 * `BrowserContext.setOffline(true)` on its own is not enough, and this is not a
 * guess: with only `setOffline`, window B's socket stayed open for the full 60s
 * the delta test waits, and the "disconnected" half of that test never happened.
 * Chromium's offline emulation refuses *new* connections; it does not tear down
 * an established one.
 *
 * So the outage is made of two halves. `setOffline` stops the client
 * re-establishing anything — its reconnect first calls `get_ws_token`, a server
 * function, which fails offline and re-arms the backoff (`websocket.rs`,
 * `schedule_reconnect`). This hook supplies the other half: closing the socket
 * that is already up. What the app sees is exactly what a dropped connection
 * looks like to it — an `onclose` it did not initiate, so `intentional_close` is
 * false and it starts trying to come back.
 *
 * Must be installed before the page's first navigation: `addInitScript` runs on
 * each new document, and only documents created after it is registered.
 */
export async function installSocketControl(page: Page) {
  await page.addInitScript(() => {
    const live: WebSocket[] = [];
    const Real = window.WebSocket;
    const Wrapped: any = function (url: string | URL, protocols?: string | string[]) {
      const sock = protocols === undefined ? new Real(url) : new Real(url, protocols);
      live.push(sock);
      sock.addEventListener('close', () => {
        const i = live.indexOf(sock);
        if (i >= 0) live.splice(i, 1);
      });
      return sock;
    };
    Wrapped.prototype = Real.prototype;
    Wrapped.OPEN = Real.OPEN;
    Wrapped.CLOSED = Real.CLOSED;
    Wrapped.CONNECTING = Real.CONNECTING;
    Wrapped.CLOSING = Real.CLOSING;
    (window as any).WebSocket = Wrapped;
    (window as any).__trakktDropSockets = () => {
      const n = live.length;
      // Copy first: `close()` fires the listener above, which splices `live`.
      for (const s of [...live]) s.close();
      return n;
    };
  });
}

/**
 * Take `page` off the network and terminate the sockets it already has.
 *
 * Returns how many live sockets were dropped, so a caller can tell "the outage
 * happened" apart from "there was nothing to disconnect".
 */
export async function goOffline(ctx: BrowserContext, page: Page): Promise<number> {
  await ctx.setOffline(true);
  return await page.evaluate(() => (window as any).__trakktDropSockets());
}

/** Put `ctx` back on the network. Reconnection is the app's own business. */
export async function goOnline(ctx: BrowserContext) {
  await ctx.setOffline(false);
}

/** Take a mark to measure subsequent probe records against. */
export function mark(): ProbeMark {
  return { t: Date.now() };
}

/**
 * Wait until `page` has stopped re-reading `serverFn` of its own accord, then
 * take a mark.
 *
 * Marking a fixed moment after the page looks loaded is not enough. A first
 * render can issue the same read twice — `project_detail.rs` keys its resource
 * on a route param that resolves after hydration — and a second call landing
 * just after the mark is indistinguishable from the one a sync frame provokes,
 * which would make `expectDeliveredByCounter` fail for the wrong reason.
 *
 * Waiting for quiescence instead is also the poll check: a page that re-reads on
 * a timer never goes quiet, and this fails saying exactly that.
 */
export async function markWhenQuiet(
  probe: SyncProbe,
  serverFn: string,
  quietMs = 2500,
  timeoutMs = 45_000,
): Promise<ProbeMark> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const calls = probe.serverFnCalls.filter((c) => isServerFn(c, serverFn));
    const last = calls.length > 0 ? calls[calls.length - 1].t : 0;
    const quietFor = Date.now() - last;
    if (quietFor >= quietMs) return { t: Date.now() };
    if (Date.now() > deadline) {
      throw new Error(
        `${serverFn} was still being re-read after ${timeoutMs}ms of waiting for the page ` +
          `to settle (${calls.length} calls, last one ${quietFor}ms ago). Something is ` +
          `re-reading it on a timer, which would make the sync assertions meaningless.`,
      );
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

/**
 * Does a recorded call belong to the server function named `name`?
 *
 * Prefix, not equality: unless a `#[server]` attribute sets `endpoint`, Leptos
 * derives the endpoint from the function name and appends a hash to keep it
 * unique, so `list_milestones` is requested at a path like
 * `/leptos-api/list_milestones1f0a2c`. Matching on equality would silently find
 * nothing and report it as "the page never re-read", which is the wrong
 * diagnosis for the right symptom.
 */
function isServerFn(call: { fn: string }, name: string): boolean {
  return call.fn === name || call.fn.startsWith(name);
}

/** Sync frames the server pushed after `since`, parsed, unparseable ones dropped. */
function inboundActionsSince(probe: SyncProbe, since: ProbeMark) {
  return probe.frames
    .filter((f) => f.dir === 'in' && f.t >= since.t)
    .map((f) => {
      try {
        return { t: f.t, socket: f.socket, body: JSON.parse(f.payload) };
      } catch {
        return null;
      }
    })
    .filter((f): f is { t: number; socket: number; body: any } => f !== null);
}

/**
 * Assert that `page` re-read `serverFn` *because* a sync frame for
 * `entityType` arrived — and not for any of the other reasons a page can
 * re-read data.
 *
 * Four separate claims, because the visible outcome is identical under all of
 * them:
 *
 *  - the document did not reload (no main-frame navigation since the mark), so
 *    this is not an SSR render of fresh data;
 *  - a `sync_action` frame for `entityType` arrived at some time `tf`;
 *  - that frame carried `payloadMustContain`, so it is the frame for the change
 *    under test and not some other row of the same entity type;
 *  - the re-read happened at or after `tf`, and there was no earlier re-read
 *    between the mark and `tf` — so it is not a poll or a refetch-on-focus that
 *    happened to land in the same window.
 *
 * Returns the frame and call it matched, so a caller can assert further (the
 * delta suite checks which socket the frame arrived on).
 */
export function expectDeliveredByCounter(
  probe: SyncProbe,
  since: ProbeMark,
  entityType: string,
  serverFn: string,
  payloadMustContain: string,
): { frame: { t: number; socket: number; body: any }; call: { t: number; fn: string } } {
  expect(
    probe.navigations.filter((n) => n.t >= since.t),
    'the window must not have reloaded — a reload re-reads everything and proves nothing',
  ).toEqual([]);

  // `payloadMustContain` is what ties the frame to the change under test. A
  // window opens sockets as it navigates, and every fresh socket is streamed a
  // bootstrap or a delta containing rows of this same entity type — so matching
  // on `entity_type` alone can pick up a frame that had nothing to do with the
  // edit being asserted on, and quietly turn this into a much weaker check than
  // it reads as. The caller passes the new name, which the payload carries.
  const frames = inboundActionsSince(probe, since).filter(
    (f) =>
      f.body?.type === 'sync_action' &&
      f.body?.entity_type === entityType &&
      JSON.stringify(f.body?.data ?? null).includes(payloadMustContain),
  );
  expect(
    frames.length,
    `no ${entityType} sync_action frame carrying ${JSON.stringify(payloadMustContain)} reached ` +
      `this window — nothing could have bumped its counter for this change`,
  ).toBeGreaterThan(0);
  const frame = frames[0];

  const calls = probe.serverFnCalls.filter((c) => c.t >= since.t && isServerFn(c, serverFn));
  expect(
    calls.filter((c) => c.t < frame.t),
    `${serverFn} was re-read before the ${entityType} frame arrived — something other than the sync counter is refetching`,
  ).toEqual([]);
  const after = calls.filter((c) => c.t >= frame.t);
  expect(
    after.length,
    `${serverFn} was never re-read after the ${entityType} frame arrived`,
  ).toBeGreaterThan(0);

  console.log(
    `[delivered-by-counter] ${entityType} ${frame.body.action} frame for ` +
      `${JSON.stringify(payloadMustContain)} (entity ${frame.body.entity_id}, sync_id ` +
      `${frame.body.sync_id}) at +${frame.t - since.t}ms on socket ${frame.socket}; ` +
      `${serverFn} re-read at +${after[0].t - since.t}ms (${after.length} call(s)); ` +
      `navigations since mark: ${probe.navigations.filter((n) => n.t >= since.t).length}`,
  );

  return { frame, call: after[0] };
}

/**
 * Wait until a sync frame for `entityType` carrying `payloadMustContain` has
 * reached this window, and return the `entity_id` that frame was addressed to.
 *
 * This exists because `expectDeliveredByCounter`'s discriminating string has to
 * appear in the frame's *payload*, and not every payload names something a test
 * chose. `IssueRelation` is the case that forced it: every field on it —
 * `relation_id`, `source_issue_id`, `target_issue_id` — is a UUID the server
 * minted (`crates/trakkt-types/src/models.rs:368-376`), so the only way to say
 * "the frame for THIS relation, not some other row of the same type" is to know
 * one of the two issue ids first. The server has already told this window what
 * it is: the `issue` frame for that issue's creation carries the id as its
 * `entity_id` and the title the test chose in its `data`. So this reads the id
 * back off the wire rather than adding a second channel — a direct API call or a
 * database read — that could disagree with what the browser was actually sent.
 *
 * Every frame the probe holds is searched, not only those after some mark, and
 * that is deliberate: `handle_sync_bootstrap` streams rows as
 * `SyncResponse::SyncAction` as well (`apps/server/src/routes/websocket.rs`,
 * `stream_entities`), so a window that connected *after* the entity was created
 * learns the same id from its bootstrap. The lookup therefore does not constrain
 * the order a test has to create things in.
 */
export async function waitForEntityId(
  probe: SyncProbe,
  entityType: string,
  payloadMustContain: string,
  timeoutMs = 30_000,
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const match = inboundActionsSince(probe, { t: 0 }).find(
      (f) =>
        f.body?.type === 'sync_action' &&
        f.body?.entity_type === entityType &&
        JSON.stringify(f.body?.data ?? null).includes(payloadMustContain),
    );
    const id = match?.body?.entity_id;
    if (typeof id === 'string' && id.length > 0) return id;
    if (Date.now() > deadline) {
      throw new Error(
        `no ${entityType} sync_action frame carrying ${JSON.stringify(payloadMustContain)} ` +
          `reached this window within ${timeoutMs}ms (${probe.frames.length} frames seen). ` +
          `Without it there is no id to identify the frame under test by, so the assertion ` +
          `that depends on it would be weaker than it reads.`,
      );
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

/**
 * Wait until the page's newest socket has asked the server for its data.
 *
 * Scoped to the newest socket on purpose. A probe accumulates every frame the
 * window has ever exchanged, across every document it has loaded, so a check
 * over all frames would be satisfied by a handshake from three navigations ago
 * and return instantly on a window whose current socket is not up yet — a wait
 * that always succeeds is not a wait.
 */
export async function waitForSyncHandshake(probe: SyncProbe, label: string, timeoutMs = 30_000) {
  await expect
    .poll(
      () => {
        const newest = probe.sockets.length - 1;
        if (newest < 0) return false;
        return probe.frames.some(
          (f) =>
            f.socket === newest &&
            f.dir === 'out' &&
            (f.payload.includes('"sync_bootstrap"') || f.payload.includes('"sync_delta"')),
        );
      },
      {
        timeout: timeoutMs,
        message: `window ${label}'s newest socket never sent a sync_bootstrap or sync_delta`,
      },
    )
    .toBe(true);
}
