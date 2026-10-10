import { defineConfig, devices } from '@playwright/test';

// Dedicated configuration avoids the general suite's development DB and auth
// setup. The server must be a disposable migrated personal-mode fixture.
const baseURL = process.env.BASE_URL;
if (!baseURL || !process.env.GITHUB_SETTINGS_E2E_DB || process.env.TRAKKT_MODE !== 'personal') {
  throw new Error('Set BASE_URL, GITHUB_SETTINGS_E2E_DB and TRAKKT_MODE=personal for the isolated GitHub settings fixture');
}
if (!['localhost', '127.0.0.1', '[::1]'].includes(new URL(baseURL).hostname)) {
  throw new Error('GitHub settings fixtures require a localhost server');
}

export default defineConfig({
  testDir: './tests/integrations',
  testMatch: 'github-connections.spec.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [['list']],
  use: {
    baseURL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    actionTimeout: 10_000,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
