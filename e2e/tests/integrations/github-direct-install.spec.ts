import { test, expect, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { resolve } from 'node:path';
import argon2 from 'argon2';

const callback = '/integrations/github/callback?installation_id=424242&setup_action=install';
const db = process.env.GITHUB_DIRECT_E2E_DB;
function sql(statement: string): string {
  if (!db || process.env.TRAKKT_MODE !== 'saas' || !process.env.BASE_URL ||
      !['localhost', '127.0.0.1', '[::1]'].includes(new URL(process.env.BASE_URL).hostname)) {
    throw new Error('A disposable localhost SaaS SQLite fixture is required');
  }
  return execFileSync('sqlite3', [resolve(db), '.timeout 5000', statement], { encoding: 'utf8' }).trim();
}
const quote = (value: string) => `'${value.replace(/'/g, "''")}'`;

test.beforeEach(async ({ page }) => {
  // Specific controlled authorization responses are registered later; every
  // other GitHub navigation/request is blocked, including provider API hosts.
  await page.route(/^https:\/\/([^.]+\.)?github\.com\//, route => route.abort());
});

test('direct install survives a real password sign-in and reload without linking an account', async ({ page }) => {
  const id = randomUUID();
  const workspace = `direct-workspace-${id}`;
  const email = `direct-${id}@example.test`;
  const password = 'DisposableFixturePassword123';
  const hash = await argon2.hash(password);
  sql(`INSERT INTO users (user_id, email, name, active, verified, last_workspace_id) VALUES (${quote(id)}, ${quote(email)}, 'Direct install fixture', 1, 1, ${quote(workspace)});
    INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES (${quote(workspace)}, 'Direct install workspace', ${quote(id)});
    INSERT INTO workspace_users (workspace_id, user_id, role) VALUES (${quote(workspace)}, ${quote(id)}, 'workspace_admin');
    INSERT INTO user_auth_methods (user_id, auth_type, auth_data) VALUES (${quote(id)}, 'password', ${quote(JSON.stringify({ hash }))});`);
  try {
    await page.goto(callback);
    await expect(page.getByText('Connect GitHub to Trakkt', { exact: true })).toBeVisible();
    const signIn = page.getByRole('link', { name: 'Sign in to continue', exact: true });
    await expect(signIn).toBeVisible();
    const target = new URL(await signIn.getAttribute('href') as string, page.url());
    expect(target.searchParams.get('redirect')).toBe(callback);
    await signIn.click();
    await page.locator('#login-email').fill(email);
    await page.locator('#login-password').fill(password);
    await page.locator('button[type="submit"]').click();
    await expect(page).toHaveURL(new RegExp('/integrations/github/callback\\?'));
    await expect(page.getByRole('button', { name: 'Continue with GitHub', exact: true })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Direct install workspace', exact: true })).toBeVisible();
    await page.reload();
    await expect(page.getByRole('button', { name: 'Continue with GitHub', exact: true })).toBeEnabled();
    expect(new URL(page.url()).searchParams.get('installation_id')).toBe('424242');
    expect(sql(`SELECT count(*) FROM github_installations WHERE workspace_id=${quote(workspace)}`)).toBe('0');
    expect(sql(`SELECT count(*) FROM github_connection_states WHERE workspace_id=${quote(workspace)}`)).toBe('0');
    let oauthURL: URL | undefined;
    await page.route('https://github.com/login/oauth/authorize?*', route => {
      oauthURL = new URL(route.request().url());
      return route.fulfill({ body: 'Controlled OAuth authorization landing', contentType: 'text/plain' });
    });
    await page.getByRole('button', { name: 'Continue with GitHub', exact: true }).click();
    await expect(page).toHaveURL(/^https:\/\/github\.com\/login\/oauth\/authorize\?/);
    expect(oauthURL?.searchParams.get('state')).toHaveLength(43);
    expect(oauthURL?.searchParams.get('code_challenge')).toHaveLength(43);
    expect(oauthURL?.searchParams.get('code_challenge_method')).toBe('S256');
    expect(sql(`SELECT user_id || '|' || installation_id || '|' || phase FROM github_connection_states WHERE workspace_id=${quote(workspace)}`)).toBe(`${id}|424242|oauth`);
    expect(sql(`SELECT count(*) FROM github_installations WHERE workspace_id=${quote(workspace)}`)).toBe('0');
  } finally {
    sql(`DELETE FROM github_connection_states WHERE user_id=${quote(id)};
      DELETE FROM refresh_tokens WHERE user_id=${quote(id)};
      DELETE FROM user_auth_methods WHERE user_id=${quote(id)};
      DELETE FROM workspace_users WHERE user_id=${quote(id)};
      DELETE FROM workspaces WHERE workspace_id=${quote(workspace)};
      DELETE FROM users WHERE user_id=${quote(id)};`);
  }
});

async function mockIdentity(page: Page) {
  await page.route('**/leptos-api/get_user_context*', route => route.fulfill({ json: {
    user_id: 'direct-ui-fixture', email: 'fixture@example.test', name: 'Fixture',
    workspace_id: 'current-member', workspace_name: 'Member workspace', workspace_roles: ['workspace_user'],
    is_owner: false, is_personal_mode: false, is_self_hosted: false, billing_enabled: false,
    subscription_status: null, capabilities: {},
  } }));
  await page.route('**/leptos-api/get_github_connect_workspaces*', route => route.fulfill({ json: [
    { workspace_id: 'current-member', name: 'Member workspace', is_current: true, is_admin: false },
    { workspace_id: 'chosen-admin', name: 'Admin workspace', is_current: false, is_admin: true },
  ] }));
}

test('explicit destination selection checks admin capability before starting OAuth (UI routing fixture)', async ({ page }) => {
  await mockIdentity(page);
  const starts: URLSearchParams[] = [];
  await page.route('**/leptos-api/start_direct_github_connection*', async route => {
    starts.push(new URLSearchParams(route.request().postData() ?? ''));
    await route.fulfill({ json: '/direct-fixture-oauth' });
  });
  await page.route('**/direct-fixture-oauth', route => route.fulfill({ body: 'OAuth redirect fixture', contentType: 'text/plain' }));
  await page.goto(callback);
  const continueButton = page.getByRole('button', { name: 'Continue with GitHub', exact: true });
  await expect(continueButton).toBeDisabled();
  expect(starts).toHaveLength(0);
  await page.getByRole('button', { name: 'Member workspace', exact: true }).click();
  await page.getByText('Admin workspace', { exact: true }).click();
  await expect(continueButton).toBeEnabled();
  expect(starts).toHaveLength(0);
  await continueButton.click();
  await expect(page).toHaveURL(/direct-fixture-oauth$/);
  expect(starts).toHaveLength(1);
  expect(starts[0].get('installation_id')).toBe('424242');
  expect(starts[0].get('workspace_id')).toBe('chosen-admin');
});

test('a supplied state stays on the strict callback path and is scrubbed (UI routing fixture)', async ({ page }) => {
  await mockIdentity(page);
  let strictCalls = 0;
  let directCalls = 0;
  await page.route('**/leptos-api/process_github_callback*', route => {
    strictCalls++;
    return route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ ServerError: 'Invalid GitHub authorization state' }) });
  });
  await page.route('**/leptos-api/start_direct_github_connection*', route => { directCalls++; return route.abort(); });
  await page.goto(`${callback}&state=fixture-replayed-state`);
  await expect(page.getByText('Failed to connect GitHub', { exact: true })).toBeVisible();
  expect(strictCalls).toBe(1);
  expect(directCalls).toBe(0);
  expect(new URL(page.url()).searchParams.has('state')).toBe(false);
  await expect(page.getByRole('button', { name: 'Continue with GitHub', exact: true })).toHaveCount(0);
});

test('an empty state cannot enter the direct-install fallback', async ({ page }) => {
  await mockIdentity(page);
  let directCalls = 0;
  await page.route('**/leptos-api/start_direct_github_connection*', route => { directCalls++; return route.abort(); });
  await page.goto(`${callback}&state=`);
  await expect(page.getByText('Failed to connect GitHub', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Continue with GitHub', exact: true })).toHaveCount(0);
  expect(directCalls).toBe(0);
});

test('update callbacks retain only the numeric installation candidate', async ({ page }) => {
  await mockIdentity(page);
  await page.goto('/integrations/github/callback?installation_id=424242&setup_action=update&ignored=discard-me');
  await expect(page.getByText('Connect GitHub to Trakkt', { exact: true })).toBeVisible();
  const url = new URL(page.url());
  expect(url.searchParams.get('installation_id')).toBe('424242');
  expect(url.searchParams.get('setup_action')).toBe('install');
  expect(url.searchParams.has('ignored')).toBe(false);
});

test('an installation-only setup redirect opens the direct-install landing', async ({ page }) => {
  await mockIdentity(page);
  await page.goto('/integrations/github/callback?installation_id=424242');
  await expect(page.getByText('Connect GitHub to Trakkt', { exact: true })).toBeVisible();
  expect(new URL(page.url()).searchParams.get('setup_action')).toBe('install');
  await expect(page.getByText('Failed to connect GitHub', { exact: true })).toHaveCount(0);
});

test('a stateful installation-only redirect keeps authorization on the strict path (UI routing fixture)', async ({ page }) => {
  let submitted: URLSearchParams | undefined;
  await page.route('**/leptos-api/process_github_callback*', route => {
    submitted = new URLSearchParams(route.request().postData() ?? '');
    return route.fulfill({ json: '/stateful-authorization-fixture' });
  });
  await page.route('**/stateful-authorization-fixture', route => route.fulfill({ body: 'Strict setup authorization fixture' }));
  await page.goto('/integrations/github/callback?installation_id=424242&state=fixture-state');
  await expect(page).toHaveURL(/stateful-authorization-fixture$/);
  expect(submitted?.get('installation_id')).toBe('424242');
  expect(submitted?.get('state')).toBe('fixture-state');
  expect(submitted?.get('setup_action')).toBe('install');
});

test('successful OAuth selects the verified workspace before reloading settings (UI routing fixture)', async ({ page }) => {
  const switched: string[] = [];
  await page.route('**/leptos-api/complete_github_authorization*', route => route.fulfill({ json: 'verified-destination' }));
  await page.route('**/leptos-api/switch_workspace*', route => {
    switched.push(new URLSearchParams(route.request().postData() ?? '').get('workspace_id') ?? '');
    return route.fulfill({ json: null });
  });
  await page.route('**/settings/integrations', route => route.fulfill({ body: 'Verified workspace settings fixture' }));
  await page.goto('/integrations/github/oauth/callback?state=fixture-state&code=fixture-code');
  await expect(page).toHaveURL(/settings\/integrations$/);
  expect(switched).toEqual(['verified-destination']);
});

test('workspace switching failure preserves successful connection and scrubs OAuth credentials (UI routing fixture)', async ({ page }) => {
  await page.route('**/leptos-api/complete_github_authorization*', route => route.fulfill({ json: 'verified-destination' }));
  await page.route('**/leptos-api/switch_workspace*', route => route.fulfill({
    status: 500, contentType: 'application/json', body: JSON.stringify({ ServerError: 'Fixture workspace switch failure' }),
  }));
  await page.goto('/integrations/github/oauth/callback?state=fixture-state&code=fixture-code');
  await expect(page.getByText('GitHub connected successfully!', { exact: true })).toBeVisible();
  await expect(page.getByText('Connected. Choose the workspace in settings to view this connection.', { exact: true })).toBeVisible();
  await expect(page.getByText('Failed to connect GitHub', { exact: true })).toHaveCount(0);
  expect(new URL(page.url()).search).toBe('');
});

test('a signed-in workspace lookup failure offers a working retry (UI routing fixture)', async ({ page }) => {
  await mockIdentity(page);
  let calls = 0;
  await page.route('**/leptos-api/get_github_connect_workspaces*', route => {
    calls++;
    if (calls === 1) return route.fulfill({ status: 500, contentType: 'application/json',
      body: JSON.stringify({ ServerError: 'Fixture workspace lookup failed' }) });
    return route.fulfill({ json: [{ workspace_id: 'chosen-admin', name: 'Admin workspace', is_current: true, is_admin: true }] });
  });
  await page.goto(callback);
  await expect(page.getByText('Could not load your workspaces. Try again.', { exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Sign in to continue', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Continue with GitHub', exact: true })).toBeEnabled();
  expect(calls).toBe(2);
});
