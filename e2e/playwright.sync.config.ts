import { defineConfig, devices } from '@playwright/test';

// Config for the two-client realtime suites (`tests/sync/`) only.
//
// Separate from `playwright.config.ts` for one reason: that config wires in
// `global-setup.ts`, which reads `data/trakkt.db` with the `sqlite3` CLI and
// logs in as `local@localhost`. These suites run against Postgres and do their
// own signup through the real UI, so that setup is not merely redundant here —
// its login branch is not wrapped in a catch, so a login page that is slow to
// render fails the whole run before a single sync assertion has been made.
//
// Run with: npx playwright test --config playwright.sync.config.ts

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:3100';

export default defineConfig({
  testDir: './tests/sync',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,

  // No retries, in CI or out of it. Every test here asserts that a change
  // crossed between two clients; one that only passes on the second attempt is
  // reporting the exact class of defect this suite exists to catch, and a retry
  // would hide it. It would also double the cost of the slowest job in the repo.
  retries: 0,

  // One worker: two browser contexts per suite already, and both suites sign in
  // as the same fixed user against one database.
  workers: 1,

  reporter: [['list']],

  // Per-test budgets are set in the specs themselves (`test.setTimeout`), where
  // the reconnect backoff that drives the delta test's budget is documented.
  timeout: 180_000,
  expect: { timeout: 15_000 },

  use: {
    baseURL: BASE_URL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
