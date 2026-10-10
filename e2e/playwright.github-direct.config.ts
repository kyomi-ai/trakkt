import { defineConfig, devices } from '@playwright/test';

const baseURL = process.env.BASE_URL;
if (!baseURL || !process.env.GITHUB_DIRECT_E2E_DB || process.env.TRAKKT_MODE !== 'saas') {
  throw new Error('Set BASE_URL, GITHUB_DIRECT_E2E_DB and TRAKKT_MODE=saas for a disposable direct-install fixture');
}
if (!['localhost', '127.0.0.1', '[::1]'].includes(new URL(baseURL).hostname)) {
  throw new Error('Direct-install tests require a localhost server');
}

export default defineConfig({
  testDir: './tests/integrations', testMatch: 'github-direct-install.spec.ts',
  outputDir: './test-results/github-direct',
  workers: 1, retries: 0, timeout: 60_000, reporter: [['list']],
  expect: { timeout: 15_000 },
  use: { baseURL, trace: 'retain-on-failure', screenshot: 'only-on-failure' },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
