import { defineConfig, devices } from '@playwright/test';

// This suite owns a fresh personal-mode server and DB. Never use global-setup,
// storageState, BASE_URL, or the ordinary E2E suite's shared application.
export default defineConfig({
  testDir: './tests/connect',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  outputDir: './test-results/connect',
  reporter: 'list',
  use: {
    ...devices['Desktop Chrome'],
    baseURL: 'http://localhost:3441',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    actionTimeout: 15_000,
  },
});
