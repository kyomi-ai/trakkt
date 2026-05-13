import { test, expect } from '@playwright/test';
import {
  gotoAuthenticated,
  seedUser,
  seedWorkspaceMember,
  cleanTestData,
  getDefaultUserId,
  getDefaultWorkspaceId,
  walCheckpoint,
  db,
} from '../../helpers/test-helpers';

const TEAM_URL = '/settings/team';

test.describe('TC-019: Team - Transfer Ownership', () => {
  let targetMemberId: string;
  let targetMemberEmail: string;

  test.beforeAll(() => {
    // Clean any pending transfers from previous runs before seeding.
    // The server function checks for existing pending transfers before creating new ones.
    db.exec(
      `DELETE FROM ownership_transfers WHERE workspace_id = '${getDefaultWorkspaceId()}'`
    );
    cleanTestData();
    const member = seedUser({ name: 'Transfer Recipient' });
    targetMemberId = member.userId;
    targetMemberEmail = member.email;
    seedWorkspaceMember({
      workspaceId: getDefaultWorkspaceId(),
      userId: targetMemberId,
      role: 'workspace_admin',
    });
    walCheckpoint();
  });

  test.afterAll(() => {
    // The server creates transfers with xfer-{uuid} IDs which don't match cleanTestData's pattern
    db.exec(
      `DELETE FROM ownership_transfers WHERE workspace_id = '${getDefaultWorkspaceId()}' AND to_user_id LIKE 'user-test%'`
    );
    cleanTestData();
    walCheckpoint();
  });

  test('should initiate ownership transfer via the two-step modal', async ({ page }) => {
    await gotoAuthenticated(page, TEAM_URL);

    await expect(page.getByRole('heading', { name: 'Team Members' })).toBeVisible({ timeout: 10_000 });

    // "Transfer Ownership" button is only visible to the owner
    const transferButton = page.getByRole('button', { name: /Transfer Ownership/i });
    await expect(transferButton).toBeVisible({ timeout: 10_000 });
    await transferButton.click();

    // Step 1: Modal opens with warning and member selection
    const modalTitle = page.getByRole('heading', { name: 'Transfer Workspace Ownership' });
    await expect(modalTitle).toBeVisible({ timeout: 5_000 });

    await expect(
      page.getByText('Transferring ownership will remove your owner privileges')
    ).toBeVisible();

    // Select the target member from the DynSelect dropdown
    const memberSelect = page.locator('button[aria-haspopup="listbox"]').filter({
      hasText: /Choose a workspace member/i,
    });
    await expect(memberSelect).toBeVisible();
    await memberSelect.click();

    // The option label is "Transfer Recipient (email)" or just the email
    const memberOption = page.getByRole('option').filter({ hasText: targetMemberEmail });
    await expect(memberOption).toBeVisible({ timeout: 5_000 });
    await memberOption.click();

    // Click "Next" to proceed to step 2
    const nextButton = page.getByRole('button', { name: 'Next' });
    await expect(nextButton).toBeEnabled();
    await nextButton.click();

    // Step 2: Confirmation — type workspace name
    await expect(page.getByText('Final Confirmation Required')).toBeVisible({ timeout: 5_000 });
    await expect(page.getByText('Transfer ownership to:')).toBeVisible();

    // Read the workspace name from the confirmation label in the modal
    // (the span with font-mono text-primary styling after "Type the workspace name to confirm:")
    const workspaceNameLabel = page.locator('span.font-mono.text-primary');
    await expect(workspaceNameLabel).toBeVisible();
    const workspaceName = (await workspaceNameLabel.textContent()) ?? '';
    expect(workspaceName.trim()).toBeTruthy();

    const confirmInput = page.locator('input[type="text"][placeholder="Enter workspace name"]');
    await expect(confirmInput).toBeVisible();

    // Scope to the modal overlay to avoid ambiguity with the page's Transfer Ownership button
    const modalOverlay = page.locator('div.fixed.inset-0.z-\\[1000\\]');

    // Type a wrong name first to verify validation
    await confirmInput.fill('wrong-name');
    await expect(page.getByText('Workspace name does not match')).toBeVisible();

    // "Transfer Ownership" button in the modal footer should be disabled with wrong name
    const transferConfirmButton = modalOverlay.getByRole('button', { name: 'Transfer Ownership' });
    await expect(transferConfirmButton).toBeDisabled();

    // Type the correct workspace name
    await confirmInput.fill(workspaceName.trim());

    await expect(page.getByText('Workspace name does not match')).not.toBeVisible({ timeout: 5_000 });
    await expect(transferConfirmButton).toBeEnabled();

    // Submit the transfer
    await transferConfirmButton.click();

    // Modal should close
    await expect(modalTitle).not.toBeVisible({ timeout: 10_000 });

    // Verify a pending transfer was created in the DB
    const pendingTransfers = db.query(
      `SELECT transfer_id FROM ownership_transfers WHERE workspace_id = '${getDefaultWorkspaceId()}' AND from_user_id = '${getDefaultUserId()}' AND to_user_id = '${targetMemberId}' AND status = 'pending'`
    );
    expect(pendingTransfers.length).toBeGreaterThan(0);

    // The page should show the pending transfer
    await expect(
      page.getByText(`Pending transfer to ${targetMemberEmail}`)
    ).toBeVisible({ timeout: 10_000 });
  });
});
