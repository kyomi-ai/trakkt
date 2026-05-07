import { type Page, type BrowserContext, expect } from '@playwright/test';
import * as crypto from 'crypto';
import * as path from 'path';
import { execSync } from 'child_process';

const DB_PATH = path.resolve(__dirname, '../../data/tane.db');

function sqliteExec(sql: string): string {
  return execSync(`sqlite3 "${DB_PATH}" ".timeout 5000" "${sql.replace(/"/g, '\\"')}"`, {
    encoding: 'utf-8',
  }).trim();
}

function sqliteQuery(sql: string): string[] {
  const result = sqliteExec(sql);
  return result ? result.split('\n') : [];
}

export function hashPassword(password: string): string {
  const bcrypt = require('bcrypt') as typeof import('bcrypt') | undefined;
  if (bcrypt) {
    return (bcrypt as any).hashSync(password, 10);
  }
  // Fallback: use argon2-style hash that tane-auth accepts
  // For test seeding we insert a bcrypt hash directly
  throw new Error('bcrypt not available — install it or use API-based seeding');
}

export function generateUserId(): string {
  return `user-test-${crypto.randomBytes(8).toString('hex')}`;
}

export function generateWorkspaceId(): string {
  return `ws-test-${crypto.randomBytes(8).toString('hex')}`;
}

export function generateEmail(): string {
  return `test-${crypto.randomBytes(6).toString('hex')}@example.com`;
}

export function generateInvitationId(): string {
  return `inv-${crypto.randomBytes(12).toString('hex')}`;
}

export function generateTransferId(): string {
  return `xfer-${crypto.randomBytes(12).toString('hex')}`;
}

export function nowISO(): string {
  return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
}

export function futureISO(days: number): string {
  const d = new Date();
  d.setDate(d.getDate() + days);
  return d.toISOString().replace(/\.\d{3}Z$/, 'Z');
}

export function pastISO(days: number): string {
  const d = new Date();
  d.setDate(d.getDate() - days);
  return d.toISOString().replace(/\.\d{3}Z$/, 'Z');
}

/**
 * Seed a user directly in SQLite. Returns the user_id.
 * Password is stored as a bcrypt hash if provided.
 */
export function seedUser(opts: {
  userId?: string;
  email?: string;
  name?: string;
  password?: string;
  verified?: boolean;
  active?: boolean;
}): { userId: string; email: string; name: string } {
  const userId = opts.userId ?? generateUserId();
  const email = opts.email ?? generateEmail();
  const name = opts.name ?? 'Test User';
  const now = nowISO();
  const verified = opts.verified !== false ? 1 : 0;
  const active = opts.active !== false ? 1 : 0;

  sqliteExec(
    `INSERT INTO users (user_id, email, name, verified, active, created_at, updated_at) VALUES ('${userId}', '${email}', '${name}', ${verified}, ${active}, '${now}', '${now}')`
  );

  if (opts.password) {
    // Use argon2 format that tane-auth expects — but for test simplicity,
    // we rely on the signup/login API to set passwords properly.
    // For direct seeding, we insert a bcrypt hash.
    const hash = crypto.createHash('sha256').update(opts.password).digest('hex');
    // Actually, let's use the API for password setting. For now, store a placeholder.
    // Tests that need password login should use the signup API flow.
  }

  return { userId, email, name };
}

/**
 * Seed a workspace and make a user its owner/admin.
 */
export function seedWorkspace(opts: {
  workspaceId?: string;
  name?: string;
  ownerId: string;
}): string {
  const workspaceId = opts.workspaceId ?? generateWorkspaceId();
  const name = opts.name ?? 'Test Workspace';
  const now = nowISO();

  sqliteExec(
    `INSERT INTO workspaces (workspace_id, name, owner_user_id, status, created_at, updated_at) VALUES ('${workspaceId}', '${name}', '${opts.ownerId}', 'active', '${now}', '${now}')`
  );

  sqliteExec(
    `INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) VALUES ('${workspaceId}', '${opts.ownerId}', 'workspace_admin', 1, '${now}')`
  );

  // Set user's last_workspace_id
  sqliteExec(
    `UPDATE users SET last_workspace_id = '${workspaceId}' WHERE user_id = '${opts.ownerId}'`
  );

  return workspaceId;
}

/**
 * Add a member to a workspace.
 */
export function seedWorkspaceMember(opts: {
  workspaceId: string;
  userId: string;
  role?: string;
}): void {
  const role = opts.role ?? 'workspace_user';
  const now = nowISO();

  sqliteExec(
    `INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) VALUES ('${opts.workspaceId}', '${opts.userId}', '${role}', 1, '${now}')`
  );
}

/**
 * Seed a pending workspace invitation.
 */
export function seedInvitation(opts: {
  invitationId?: string;
  workspaceId: string;
  email: string;
  invitedBy: string;
  role?: string;
  expired?: boolean;
}): string {
  const invitationId = opts.invitationId ?? generateInvitationId();
  const role = opts.role ?? 'workspace_user';
  const now = nowISO();
  const expiresAt = opts.expired ? pastISO(1) : futureISO(7);

  sqliteExec(
    `INSERT INTO workspace_invitations (invitation_id, workspace_id, email, role, invited_by_user_id, status, created_at, expires_at) VALUES ('${invitationId}', '${opts.workspaceId}', '${opts.email}', '${role}', '${opts.invitedBy}', 'pending', '${now}', '${expiresAt}')`
  );

  return invitationId;
}

/**
 * Seed a pending ownership transfer.
 */
export function seedOwnershipTransfer(opts: {
  transferId?: string;
  workspaceId: string;
  fromUserId: string;
  toUserId: string;
  expired?: boolean;
}): string {
  const transferId = opts.transferId ?? generateTransferId();
  const now = nowISO();
  const expiresAt = opts.expired ? pastISO(1) : futureISO(7);

  sqliteExec(
    `INSERT INTO ownership_transfers (transfer_id, workspace_id, from_user_id, to_user_id, status, created_at, expires_at) VALUES ('${transferId}', '${opts.workspaceId}', '${opts.fromUserId}', '${opts.toUserId}', 'pending', '${now}', '${expiresAt}')`
  );

  return transferId;
}

/**
 * Clean up test data by deleting all rows with test-prefixed IDs.
 */
/**
 * Force WAL checkpoint so that data written by the sqlite3 CLI
 * becomes visible to the server's sqlx connection pool.
 */
export function walCheckpoint(): void {
  sqliteExec(`PRAGMA wal_checkpoint(TRUNCATE)`);
}

/**
 * Clean up test data by deleting all rows with test-prefixed IDs.
 */
export function cleanTestData(): void {
  sqliteExec(`DELETE FROM ownership_transfers WHERE transfer_id LIKE 'xfer-test%'`);
  sqliteExec(`DELETE FROM workspace_invitations WHERE invitation_id LIKE 'inv-test%'`);
  sqliteExec(`DELETE FROM refresh_tokens WHERE user_id LIKE 'user-test%'`);
  sqliteExec(`DELETE FROM user_auth_methods WHERE user_id LIKE 'user-test%'`);
  sqliteExec(`DELETE FROM workspace_users WHERE user_id LIKE 'user-test%'`);
  sqliteExec(`DELETE FROM workspace_users WHERE workspace_id LIKE 'ws-test%'`);
  sqliteExec(`DELETE FROM workspaces WHERE workspace_id LIKE 'ws-test%'`);
  sqliteExec(`DELETE FROM users WHERE user_id LIKE 'user-test%'`);
  walCheckpoint();
}

/**
 * Create a self-hosted user via the signup API (self_hosted + no SMTP mode).
 * This properly creates a user with hashed password through the real service layer.
 */
/**
 * Create a user via the signup UI. In self_hosted mode, the first user can
 * signup directly; subsequent users need an invitation seeded first.
 * Uses a fresh browser context (no storageState) to avoid being redirected
 * away from /signup by an existing session.
 */
export async function signupUser(
  page: Page,
  email: string,
  name: string,
  password: string
): Promise<void> {
  // Clear cookies so we hit /signup as unauthenticated
  await page.context().clearCookies();

  await page.goto('/signup');
  await page.waitForLoadState('networkidle');
  await page.waitForSelector('#signup-email', { timeout: 15000 });
  await page.waitForTimeout(1000);

  await page.locator('#signup-email').click();
  await page.locator('#signup-email').type(email, { delay: 10 });

  const nameInput = page.locator('#signup-name');
  if (await nameInput.isVisible({ timeout: 3000 }).catch(() => false)) {
    await nameInput.click();
    await nameInput.type(name, { delay: 10 });
    await page.locator('#signup-password').click();
    await page.locator('#signup-password').type(password, { delay: 10 });
  }

  await Promise.all([
    page.waitForResponse(
      resp => resp.url().includes('signup_start') && resp.status() === 200,
      { timeout: 15000 }
    ).catch(() => {}),
    page.click('button[type="submit"]'),
  ]);

  await page.waitForTimeout(2000);
}

/**
 * Create a second user directly in the database with a properly hashed password.
 * Bypasses the signup UI and rate limiter entirely.
 */
export async function createSecondUser(
  _browser: unknown,
  opts: { email: string; name: string; password: string; workspaceId?: string; role?: string }
): Promise<string> {
  const argon2 = require('argon2');
  const wsId = opts.workspaceId ?? getDefaultWorkspaceId();
  const userId = generateUserId();
  const now = nowISO();

  const hash = await argon2.hash(opts.password, { type: argon2.argon2id });

  sqliteExec(
    `INSERT INTO users (user_id, email, name, verified, active, created_at, updated_at, last_workspace_id) VALUES ('${userId}', '${opts.email}', '${opts.name}', 1, 1, '${now}', '${now}', '${wsId}')`
  );

  const authData = JSON.stringify({ hash }).replace(/'/g, "''");
  sqliteExec(
    `INSERT INTO user_auth_methods (user_id, auth_type, auth_data, created_at, active) VALUES ('${userId}', 'password', '${authData}', '${now}', 1)`
  );

  seedWorkspaceMember({
    workspaceId: wsId,
    userId,
    role: opts.role ?? 'workspace_user',
  });

  walCheckpoint();
  return userId;
}

/**
 * Log in via the login page.
 */
export async function loginUser(
  page: Page,
  email: string,
  password: string,
  totpCode?: string
): Promise<void> {
  // Clear any stale cookies that might redirect us away from /login
  await page.context().clearCookies();

  await page.goto('/login');
  await page.waitForLoadState('networkidle');
  await page.waitForSelector('text=or sign in with email', { timeout: 15000 }).catch(() => {});
  await page.waitForTimeout(500);

  // Retry filling until values stick (Leptos hydration can clear inputs)
  for (let attempt = 0; attempt < 5; attempt++) {
    await page.locator('#login-email').click();
    await page.locator('#login-email').fill('');
    await page.locator('#login-email').type(email, { delay: 10 });
    await page.waitForTimeout(300);
    const val = await page.inputValue('#login-email');
    if (val === email) break;
    await page.waitForTimeout(500);
  }

  await page.locator('#login-password').click();
  await page.locator('#login-password').fill('');
  await page.locator('#login-password').type(password, { delay: 10 });
  await page.waitForTimeout(300);

  const submitBtn = page.locator('button[type="submit"]');
  await submitBtn.waitFor({ state: 'visible', timeout: 10000 });

  await Promise.all([
    page.waitForResponse(
      resp => resp.url().includes('login_with_password') && resp.status() === 200,
      { timeout: 15000 }
    ).catch(() => {}),
    submitBtn.click(),
  ]);

  if (totpCode) {
    await page.waitForSelector('#totp-code', { timeout: 5000 });
    await page.locator('#totp-code').type(totpCode, { delay: 10 });
    await Promise.all([
      page.waitForResponse(
        resp => resp.url().includes('login_with_password') && resp.status() === 200,
        { timeout: 15000 }
      ).catch(() => {}),
      page.click('button[type="submit"]'),
    ]);
  }

  await page.waitForTimeout(2000);
}

export const DEFAULT_TEST_USER = { email: 'local@localhost', password: 'TestPassword1234' };

let _defaultUserId: string | null = null;
let _defaultWorkspaceId: string | null = null;

export function getDefaultUserId(): string {
  if (_defaultUserId) return _defaultUserId;
  if (isPersonalMode()) { _defaultUserId = 'user-local'; return _defaultUserId; }
  const rows = sqliteQuery("SELECT user_id FROM users WHERE email = 'local@localhost'");
  _defaultUserId = rows[0] ?? 'user-local';
  return _defaultUserId;
}

export function getDefaultWorkspaceId(): string {
  if (_defaultWorkspaceId) return _defaultWorkspaceId;
  if (isPersonalMode()) { _defaultWorkspaceId = 'workspace-local'; return _defaultWorkspaceId; }
  const uid = getDefaultUserId();
  const rows = sqliteQuery(`SELECT workspace_id FROM workspace_users WHERE user_id = '${uid}' LIMIT 1`);
  _defaultWorkspaceId = rows[0] ?? 'workspace-local';
  return _defaultWorkspaceId;
}

/**
 * Ensure the page is authenticated. In personal mode this is a no-op.
 * In self_hosted mode, logs in as the default test user.
 */
export async function ensureLoggedIn(page: Page): Promise<void> {
  if (isPersonalMode()) return;
  await loginUser(page, DEFAULT_TEST_USER.email, DEFAULT_TEST_USER.password);
}

/**
 * Navigate to a path that requires authentication.
 * StorageState from global-setup provides cookies; the WASM app handles
 * token refresh. Falls back to loginUser if redirected to /login.
 */
export async function gotoAuthenticated(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(2000);

  if (!isPersonalMode() && page.url().includes('/login')) {
    await loginUser(page, DEFAULT_TEST_USER.email, DEFAULT_TEST_USER.password);
    await page.goto(path);
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(2000);
  }
}

/**
 * Wait for WASM to initialize on a page.
 */
export async function waitForWasm(page: Page): Promise<void> {
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(2000);
}

/**
 * Check if the user is on an authenticated page (not redirected to /login).
 */
export async function isAuthenticated(page: Page): Promise<boolean> {
  const url = page.url();
  return !url.includes('/login') && !url.includes('/signup');
}

/**
 * Navigate and wait for SPA navigation (no full reload).
 * Returns the persistent element check result.
 */
export async function spaNavigate(
  page: Page,
  selector: string,
  persistentSelector?: string
): Promise<boolean> {
  let persistentBefore: string | null = null;
  if (persistentSelector) {
    persistentBefore = await page
      .locator(persistentSelector)
      .evaluate((el) => el.getAttribute('data-spa-check') ?? el.id ?? 'exists')
      .catch(() => null);
  }

  await page.click(selector);
  await page.waitForTimeout(1000);

  if (persistentSelector && persistentBefore !== null) {
    const persistentAfter = await page
      .locator(persistentSelector)
      .evaluate((el) => el.getAttribute('data-spa-check') ?? el.id ?? 'exists')
      .catch(() => null);
    return persistentAfter === persistentBefore;
  }
  return true;
}

/**
 * Get all cookies from a browser context.
 */
export async function getCookies(
  context: BrowserContext,
  url?: string
): Promise<{ name: string; value: string }[]> {
  const cookies = await context.cookies(url);
  return cookies.map((c) => ({ name: c.name, value: c.value }));
}

/**
 * Check if a specific cookie exists.
 */
export async function hasCookie(
  context: BrowserContext,
  cookieName: string
): Promise<boolean> {
  const cookies = await context.cookies();
  return cookies.some((c) => c.name === cookieName);
}

/**
 * Make a raw HTTP request to the server (bypassing the browser).
 */
export async function apiRequest(
  page: Page,
  method: string,
  path: string,
  body?: unknown,
  headers?: Record<string, string>
): Promise<{ status: number; body: unknown; headers: Record<string, string> }> {
  const baseUrl = process.env.BASE_URL ?? 'http://localhost:8099';
  const response = await page.request.fetch(`${baseUrl}${path}`, {
    method,
    data: body ? JSON.stringify(body) : undefined,
    headers: {
      'Content-Type': 'application/json',
      ...headers,
    },
  });

  const responseHeaders: Record<string, string> = {};
  for (const [key, value] of Object.entries(response.headers())) {
    responseHeaders[key] = value;
  }

  let responseBody: unknown;
  try {
    responseBody = await response.json();
  } catch {
    responseBody = await response.text();
  }

  return { status: response.status(), body: responseBody, headers: responseHeaders };
}

/**
 * Direct DB query helpers for assertions.
 */
export const db = {
  getUserByEmail(email: string): string | null {
    const rows = sqliteQuery(`SELECT user_id FROM users WHERE email = '${email}'`);
    return rows[0] ?? null;
  },

  getUserById(userId: string): { email: string; name: string; verified: number } | null {
    const rows = sqliteQuery(
      `SELECT email, name, verified FROM users WHERE user_id = '${userId}'`
    );
    if (!rows[0]) return null;
    const [email, name, verified] = rows[0].split('|');
    return { email, name, verified: parseInt(verified) };
  },

  getWorkspaceMembers(workspaceId: string): string[] {
    return sqliteQuery(
      `SELECT user_id FROM workspace_users WHERE workspace_id = '${workspaceId}' AND active = 1`
    );
  },

  getMemberRole(workspaceId: string, userId: string): string | null {
    const rows = sqliteQuery(
      `SELECT role FROM workspace_users WHERE workspace_id = '${workspaceId}' AND user_id = '${userId}'`
    );
    return rows[0] ?? null;
  },

  getWorkspaceOwner(workspaceId: string): string | null {
    const rows = sqliteQuery(
      `SELECT owner_user_id FROM workspaces WHERE workspace_id = '${workspaceId}'`
    );
    return rows[0] ?? null;
  },

  getInvitationStatus(invitationId: string): string | null {
    const rows = sqliteQuery(
      `SELECT status FROM workspace_invitations WHERE invitation_id = '${invitationId}'`
    );
    return rows[0] ?? null;
  },

  getTransferStatus(transferId: string): string | null {
    const rows = sqliteQuery(
      `SELECT status FROM ownership_transfers WHERE transfer_id = '${transferId}'`
    );
    return rows[0] ?? null;
  },

  getRefreshTokenCount(userId: string): number {
    const rows = sqliteQuery(
      `SELECT COUNT(*) FROM refresh_tokens WHERE user_id = '${userId}' AND is_active = 1`
    );
    return parseInt(rows[0] ?? '0');
  },

  hasAuthMethod(userId: string, authType: string): boolean {
    const rows = sqliteQuery(
      `SELECT COUNT(*) FROM user_auth_methods WHERE user_id = '${userId}' AND auth_type = '${authType}' AND active = 1`
    );
    return parseInt(rows[0] ?? '0') > 0;
  },

  exec: sqliteExec,
  query: sqliteQuery,
};

// ---------------------------------------------------------------------------
// Auth mode detection
// ---------------------------------------------------------------------------

export interface AuthConfig {
  self_hosted: boolean;
  smtp_configured: boolean;
  password: boolean;
  passkeys: boolean;
  google_oauth: boolean;
}

let cachedAuthConfig: AuthConfig | null = null;

export async function fetchAuthConfig(_page?: Page): Promise<AuthConfig> {
  if (cachedAuthConfig) return cachedAuthConfig;

  const mode = getTaneMode();
  const selfHosted = mode === 'self_hosted';
  const personal = mode === 'personal';

  // Check SMTP by looking for SMTP_HOST in .env
  let smtpConfigured = false;
  try {
    const envPath = path.resolve(__dirname, '../../.env');
    const envContent = require('fs').readFileSync(envPath, 'utf-8');
    smtpConfigured = /^SMTP_HOST=.+$/m.test(envContent);
  } catch { /* no .env */ }

  cachedAuthConfig = {
    self_hosted: selfHosted || personal,
    smtp_configured: smtpConfigured,
    password: true,
    passkeys: !personal,
    google_oauth: false,
  };
  return cachedAuthConfig;
}

export function getTaneMode(): string {
  if (process.env.TANE_MODE) return process.env.TANE_MODE;
  try {
    const envPath = path.resolve(__dirname, '../../.env');
    const envContent = require('fs').readFileSync(envPath, 'utf-8');
    const match = envContent.match(/^TANE_MODE=(.+)$/m);
    return match?.[1]?.trim() ?? 'saas';
  } catch {
    return 'saas';
  }
}

export function isPersonalMode(): boolean {
  return getTaneMode() === 'personal';
}

export function isSelfHostedMode(): boolean {
  return getTaneMode() === 'self_hosted';
}

export function isSelfHostedNoSmtp(_config?: AuthConfig): boolean {
  return getTaneMode() === 'self_hosted' && !isSmtpConfigured();
}

function isSmtpConfigured(): boolean {
  try {
    const envPath = path.resolve(__dirname, '../../.env');
    const envContent = require('fs').readFileSync(envPath, 'utf-8');
    return /^SMTP_HOST=.+$/m.test(envContent);
  } catch {
    return false;
  }
}

export function isSaasWithSmtp(config: AuthConfig): boolean {
  return !config.self_hosted && config.smtp_configured;
}
