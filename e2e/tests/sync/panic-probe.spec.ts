import { test, expect, chromium, type Browser, type BrowserContext, type Page } from '@playwright/test';

// Narrow probe: which pages panic, and does it need two windows or just one?
//
// The two-window suite surfaced two WASM panics on /settings/workspace:
//   reactive_graph traits.rs:394 — "Tried to access a reactive value that has
//                                   already been disposed"
//   tachys class.rs:82           — "entered unreachable code"
// It passed anyway, because its assertions were about field values. This
// isolates the panic itself.

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:3100';
const EMAIL = 'sync-verify@example.com';
const PASSWORD = 'TestPassword1234';

let browser: Browser;
let ctx: BrowserContext;

test.describe.configure({ mode: 'serial' });
test.setTimeout(120_000);

async function login(page: Page) {
  await page.goto('/login');
  await page.waitForSelector('#login-email', { timeout: 30_000 });
  await page.waitForTimeout(1000);
  await page.fill('#login-email', EMAIL);
  await page.fill('#login-password', PASSWORD);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL((u) => !/\/(signup|login)/.test(u.pathname), { timeout: 30_000 });
}

test.beforeAll(async () => {
  browser = await chromium.launch();
  ctx = await browser.newContext({ baseURL: BASE_URL });
  const boot = await ctx.newPage();
  await login(boot);
  await boot.close();
});

test.afterAll(async () => {
  await browser?.close();
});

const ROUTES = [
  '/',
  '/settings/workspace',
  '/settings/notifications',
  '/settings/profile',
];

for (const route of ROUTES) {
  test(`single window: ${route} loads without a wasm panic`, async () => {
    const page = await ctx.newPage();
    const panics: string[] = [];
    const http4xx: string[] = [];

    page.on('pageerror', (e) => panics.push(`pageerror: ${e.message}`));
    page.on('console', (m) => {
      const t = m.text();
      if (t.includes('panicked at') || t.includes('unreachable')) {
        panics.push(t.split('\n')[0]);
      }
    });
    page.on('response', (r) => {
      if (r.status() >= 400) http4xx.push(`${r.status()} ${r.url()}`);
    });

    await page.goto(route);
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(4000); // let hydration + effects settle

    if (http4xx.length) console.log(`[${route}] HTTP >=400: ${JSON.stringify(http4xx, null, 2)}`);
    if (panics.length) console.log(`[${route}] PANICS: ${JSON.stringify(panics, null, 2)}`);

    await page.close();
    expect(panics, `${route} produced wasm panics`).toEqual([]);

    // The >=400 responses were already being collected and printed here, and
    // for the whole life of TRA-9985 they printed a 404 for the document of
    // every route in this list — the SPA fallback was attached with
    // `ServeDir::not_found_service`, which rewrote the shell's 200 to 404. The
    // page rendered, so the panic assertion above passed and the printed 404s
    // read as noise. Collecting without asserting is what let that run green
    // for months; this closes it.
    //
    // Deliberately not scoped to the document response. A missing subresource
    // is the other half of the same fix — the fallback must keep answering 404
    // for a static file that is not in `dist`, and if it ever starts answering
    // the app shell with 200 instead, the browser reports it here as a script
    // that failed to parse rather than as a clean 404.
    expect(http4xx, `${route} produced HTTP >=400 responses`).toEqual([]);
  });
}

test('navigating between settings tabs does not panic', async () => {
  // Disposal bugs typically need a teardown, which a single load never does.
  const page = await ctx.newPage();
  const panics: string[] = [];
  page.on('pageerror', (e) => panics.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    const t = m.text();
    if (t.includes('panicked at') || t.includes('unreachable')) panics.push(t.split('\n')[0]);
  });

  await page.goto('/settings/profile');
  await page.waitForLoadState('networkidle');

  for (const tab of ['Workspace', 'Notifications', 'Profile', 'Workspace']) {
    const link = page.locator(`text=${tab}`).first();
    if (await link.isVisible({ timeout: 5000 }).catch(() => false)) {
      await link.click();
      await page.waitForTimeout(2500);
    }
  }

  if (panics.length) console.log(`[tab navigation] PANICS: ${JSON.stringify(panics, null, 2)}`);
  await page.close();
  expect(panics, 'tab navigation produced wasm panics').toEqual([]);
});
