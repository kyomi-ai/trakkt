import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { resolve } from 'node:path';
import { gotoAuthenticated } from '../../helpers/test-helpers';

// Run against a disposable, migrated personal-mode fixture with GitHub App and
// OAuth configuration enabled. No GitHub network call is needed by disconnect.
// An explicit database path prevents modifying the normal development database.
const fixtureDatabase = process.env.GITHUB_SETTINGS_E2E_DB;
const fixtureUrl = process.env.BASE_URL;
function sql(statement: string): string {
  if (!fixtureDatabase || !fixtureUrl || process.env.TRAKKT_MODE !== 'personal') {
    throw new Error('Set GITHUB_SETTINGS_E2E_DB, BASE_URL and TRAKKT_MODE=personal for a disposable GitHub-configured fixture');
  }
  const url = new URL(fixtureUrl);
  if (!['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) {
    throw new Error('GitHub settings fixtures require a localhost server');
  }
  return execFileSync('sqlite3', [resolve(fixtureDatabase), '.timeout 5000', statement], { encoding: 'utf8' }).trim();
}

test('two GitHub cards retain independent state and durable disconnect reaches another tab', async ({ page, context }) => {
  const suffix = randomUUID();
  const appId = `github-app-e2e-${suffix}`;
  const personalId = `github-personal-e2e-${suffix}`;
  const organizationId = `github-org-e2e-${suffix}`;
  const numericId = Date.now();
  const workspace = 'workspace-local';
  const personal = `personal-${suffix}`;
  const organization = `org-${suffix}`;
  const quote = (value: string) => `'${value.replace(/'/g, "''")}'`;
  const connectionState = (id: string) => sql(`SELECT disconnected_at IS NOT NULL FROM github_installations WHERE installation_id = ${quote(id)}`);
  const existingCount = sql(`SELECT count(*) FROM github_installations WHERE workspace_id = '${workspace}'`);
  expect(existingCount, 'fixture must start with no connections in the personal workspace').toBe('0');
  sql(`INSERT INTO github_apps (github_app_id, app_id, app_name, client_id, client_secret_encrypted, private_key_encrypted, webhook_secret_encrypted) VALUES (${quote(appId)}, ${numericId}, 'fixture', 'fixture', '', '', '');
    INSERT INTO github_installations (installation_id, workspace_id, github_app_id, github_installation_id, github_account_id, account_login, account_type, target_repos, repository_selection, authorization_verified_at) VALUES
    (${quote(personalId)}, '${workspace}', ${quote(appId)}, ${numericId}, ${numericId}, ${quote(personal)}, 'User', NULL, 'all', strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    (${quote(organizationId)}, '${workspace}', ${quote(appId)}, ${numericId + 1}, ${numericId + 1}, ${quote(organization)}, 'Organization', '[]', 'selected', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));`);
  sql(`INSERT INTO github_account_claims (app_id, account_id, account_type, workspace_id) VALUES
    (${numericId}, ${numericId}, 'User', '${workspace}'),
    (${numericId}, ${numericId + 1}, 'Organization', '${workspace}');
    INSERT INTO github_installation_claims (installation_id, app_id, account_id, workspace_id) VALUES
    (${numericId}, ${numericId}, ${numericId}, '${workspace}'),
    (${numericId + 1}, ${numericId}, ${numericId + 1}, '${workspace}');`);
  const secondTab = await context.newPage();
  try {
    await gotoAuthenticated(page, '/settings/integrations');
    await gotoAuthenticated(secondTab, '/settings/integrations');
    const card = (tab: typeof page, login: string) => tab.locator('[data-github-connection]').filter({ hasText: `@${login}` });
    await expect(page.getByRole('button', { name: 'Add account', exact: true })).toBeVisible();
    await expect(card(page, personal)).toContainText('User account');
    await expect(card(page, personal)).toContainText('All repositories');
    await expect(card(page, organization)).toContainText('Organization');
    await expect(card(page, organization)).toContainText('No repositories selected');
    await expect(secondTab.locator('[data-github-connection]')).toHaveCount(2);
    await expect(page.getByText('Status Transitions', { exact: true })).toHaveCount(1);

    // Navigation would destroy the marker; receiving durable sync must refetch
    // the connection snapshot in place, including BroadcastChannel followers.
    await secondTab.evaluate(() => { (window as any).__githubSettingsMarker = 'alive'; });
    await card(page, personal).getByRole('button', { name: 'Disconnect', exact: true }).click();
    await card(page, personal).getByRole('button', { name: 'Yes, disconnect', exact: true }).click();
    await expect(card(page, personal)).toContainText('Disconnected');
    await expect(card(secondTab, personal)).toContainText('Disconnected', { timeout: 15_000 });
    await expect(card(secondTab, organization).getByRole('button', { name: 'Disconnect', exact: true })).toBeEnabled();
    await expect(card(secondTab, personal).getByRole('button', { name: 'Reconnect GitHub', exact: true })).toBeVisible();
    await expect(secondTab.getByRole('button', { name: 'Add account', exact: true })).toBeVisible();
    expect(await secondTab.evaluate(() => (window as any).__githubSettingsMarker)).toBe('alive');
    expect(connectionState(personalId)).toBe('1');
    expect(connectionState(organizationId)).toBe('0');
    expect(sql(`SELECT count(*) FROM github_installation_claims WHERE app_id = ${numericId}`)).toBe('2');
    expect(sql(`SELECT count(*) FROM github_account_claims WHERE app_id = ${numericId}`)).toBe('2');
    expect(sql(`SELECT count(*) FROM sync_log WHERE entity_type = 'GITHUB_INSTALLATION' AND entity_id = ${quote(personalId)}`)).toBe('1');
    expect(sql(`SELECT count(*) FROM github_installations WHERE workspace_id = '${workspace}'`)).toBe('2');
  } finally {
    await secondTab.close();
    sql(`DELETE FROM sync_log WHERE entity_id IN (${quote(personalId)}, ${quote(organizationId)});
      DELETE FROM github_installations WHERE installation_id IN (${quote(personalId)}, ${quote(organizationId)});
      DELETE FROM github_installation_claims WHERE app_id = ${numericId};
      DELETE FROM github_account_claims WHERE app_id = ${numericId};
      DELETE FROM github_apps WHERE github_app_id = ${quote(appId)};`);
  }
});
