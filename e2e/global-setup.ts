import { execSync } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';

const BASE_URL = process.env.BASE_URL ?? 'http://localhost:8099';
const DB_PATH = path.resolve(__dirname, '../data/trakkt.db');
const AUTH_STATE_PATH = path.resolve(__dirname, '.auth-state.json');

function getTrakktMode(): string {
  if (process.env.TRAKKT_MODE) return process.env.TRAKKT_MODE;
  try {
    const envPath = path.resolve(__dirname, '../.env');
    const envContent = fs.readFileSync(envPath, 'utf-8');
    const match = envContent.match(/^TRAKKT_MODE=(.+)$/m);
    return match?.[1]?.trim() ?? 'saas';
  } catch {
    return 'saas';
  }
}

function sqliteExec(sql: string): string {
  return execSync(`sqlite3 "${DB_PATH}" "${sql.replace(/"/g, '\\"')}"`, {
    encoding: 'utf-8',
    timeout: 5000,
  }).trim();
}

export default async function globalSetup() {
  const mode = getTrakktMode();
  console.log(`[global-setup] TRAKKT_MODE=${mode}`);

  if (mode === 'personal') {
    try { fs.unlinkSync(AUTH_STATE_PATH); } catch {}
    return;
  }

  let userExists = false;
  try {
    const count = sqliteExec("SELECT COUNT(*) FROM users WHERE email = 'local@localhost'");
    userExists = parseInt(count) > 0;
  } catch {}

  const pw = await import('playwright-core');

  if (!userExists && mode === 'self_hosted') {
    console.log('[global-setup] Creating user via signup...');
    const browser = await pw.chromium.launch();
    const context = await browser.newContext();
    const page = await context.newPage();

    try {
      await page.goto(`${BASE_URL}/signup`);
      await page.waitForSelector('#signup-email', { timeout: 15000 });
      await page.waitForTimeout(1000);
      await page.fill('#signup-email', 'local@localhost');
      const nameInput = page.locator('#signup-name');
      if (await nameInput.isVisible({ timeout: 5000 }).catch(() => false)) {
        await nameInput.fill('Local User');
        await page.fill('#signup-password', 'TestPassword1234');
      }
      await page.click('button[type="submit"]');
      await page.waitForURL(url => !url.pathname.includes('/signup'), { timeout: 15000 }).catch(() => {});
      await page.waitForTimeout(2000);
      console.log(`[global-setup] Signup done, URL: ${page.url()}`);
      await context.storageState({ path: AUTH_STATE_PATH });
      console.log('[global-setup] storageState saved from signup');
    } finally {
      await context.close();
      await browser.close();
    }
    return;
  }

  // User exists — log in to get fresh storageState
  console.log('[global-setup] Logging in...');
  const browser = await pw.chromium.launch();
  const context = await browser.newContext();
  const page = await context.newPage();

  try {
    await page.goto(`${BASE_URL}/login`);
    await page.waitForSelector('text=or sign in with email', { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(500);
    await page.locator('#login-email').click();
    await page.locator('#login-email').type('local@localhost', { delay: 10 });
    await page.locator('#login-password').click();
    await page.locator('#login-password').type('TestPassword1234', { delay: 10 });
    await page.waitForTimeout(300);
    await page.click('button[type="submit"]');
    await page.waitForTimeout(5000);

    const cookies = await context.cookies();
    const authCookies = cookies.filter(c => c.name === 'access_token' || c.name === 'refresh_token');
    if (authCookies.length > 0) {
      await context.storageState({ path: AUTH_STATE_PATH });
      console.log(`[global-setup] storageState saved (${authCookies.length} cookies)`);
    } else {
      console.log('[global-setup] WARNING: no auth cookies after login');
    }
  } finally {
    await context.close();
    await browser.close();
  }
}
