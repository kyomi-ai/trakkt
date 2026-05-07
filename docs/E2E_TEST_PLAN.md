# E2E Test Plan

Comprehensive test plan for the Tane app starter template. Covers all user-facing authentication, settings, workspace management, and MCP flows. Intended for implementation as Playwright tests.

---

## Test Environment

- Server running in `saas` mode (default) unless otherwise noted
- PostgreSQL database with migrations applied
- SMTP configured (use Mailpit or similar for local capture)
- WebAuthn tests require HTTPS or localhost

---

## TC-001: Signup Flow (Email Verification Path)

**Description:** New user signs up via email, receives verification link, completes account creation.

**Steps:**
1. Navigate to `/signup`
2. Enter a valid email address in the email field
3. Click "Continue" / submit the form
4. Verify the UI shows a "check your email" confirmation message
5. Retrieve the verification email from the test mailbox
6. Extract the verification link from the email body
7. Navigate to the verification link (should be `/signup/complete?token=...`)
8. Verify the completion form is displayed (name, password, confirm password, terms checkbox)
9. Enter display name
10. Enter password and confirm password
11. Check the terms acceptance checkbox
12. Click "Create Account"
13. Verify redirect to the application (e.g., `/onboarding` or main app page)
14. Verify the user is authenticated (user menu visible, no login redirect)

**Expected Result:** User account is created, user is logged in, and lands inside the application without a page reload on the final redirect.

---

## TC-002: Signup With Invitation

**Description:** An invited user signs up with their invited email and is auto-joined to the inviting workspace.

**Steps:**
1. As an existing workspace owner, invite a new email address via Team settings
2. Verify invitation email is sent
3. Open the invitation link from the email (or navigate to `/signup`)
4. Enter the invited email address
5. Complete the signup flow (verification email, complete form)
6. Verify the user lands in the inviting workspace (not a personal workspace)
7. Verify the user appears in the inviting workspace's member list
8. Verify no separate personal workspace was created for this user

**Expected Result:** The invited user is automatically added to the workspace that invited them. No personal workspace is created.

---

## TC-003: Login Flow (Password)

**Description:** Existing user logs in with email and password.

**Steps:**
1. Navigate to `/login`
2. Enter the user's email address
3. Enter the user's password
4. Click "Sign In" / submit the form
5. Verify the user lands inside the application
6. Verify no full page reload occurs during the transition (SPA navigation)
7. Verify the user menu shows the correct display name

**Expected Result:** User is authenticated and navigated to the app via client-side routing (no page reload).

---

## TC-004: Login Flow (Passkey)

**Description:** Existing user logs in using a registered passkey (WebAuthn).

**Steps:**
1. Navigate to `/login`
2. Click "Sign in with Passkey" button
3. Complete the WebAuthn authentication prompt (browser/OS credential dialog)
4. Verify the user lands inside the application
5. Verify no full page reload occurs

**Expected Result:** User is authenticated via passkey and navigated to the app.

**Notes:** Requires HTTPS or localhost. Playwright's `browserContext.addInitScript` or CDP can mock WebAuthn authenticators.

---

## TC-005: Logout

**Description:** Authenticated user logs out.

**Steps:**
1. Log in as an existing user
2. Click the user avatar / user menu button
3. Click "Sign Out" in the dropdown menu
4. Verify redirect to `/login`
5. Verify attempting to navigate to `/settings` redirects back to `/login`
6. Verify auth cookies are cleared

**Expected Result:** User is logged out, session is invalidated, and all protected routes redirect to login.

---

## TC-006: Auth Guard (Unauthenticated Access)

**Description:** Unauthenticated user attempting to access a protected route is redirected to login.

**Steps:**
1. Clear all cookies / use a fresh browser context
2. Navigate directly to `/settings`
3. Verify redirect to `/login`
4. Navigate directly to `/settings/profile`
5. Verify redirect to `/login`
6. Navigate directly to `/settings/security`
7. Verify redirect to `/login`

**Expected Result:** All protected routes redirect unauthenticated users to `/login`.

---

## TC-007: Already Authenticated Redirect

**Description:** Authenticated user visiting login or signup pages is redirected to the app.

**Steps:**
1. Log in as an existing user
2. Navigate to `/login`
3. Verify redirect to `/settings/profile` (or main app page)
4. Navigate to `/signup`
5. Verify redirect to `/settings/profile` (or main app page)

**Expected Result:** Authenticated users cannot access auth pages; they are redirected into the app.

---

## TC-008: Settings Navigation

**Description:** Navigation between settings tabs uses client-side routing (no page reload).

**Steps:**
1. Log in and navigate to `/settings/profile`
2. Click "Security" tab
3. Verify URL changes to `/settings/security`
4. Verify no full page reload (check that a persistent DOM element remains)
5. Click "Workspace" tab
6. Verify URL changes to `/settings/workspace`
7. Verify no full page reload
8. Click "Team" tab
9. Verify URL changes to `/settings/team`
10. Verify no full page reload
11. Click "Profile" tab
12. Verify URL changes back to `/settings/profile`
13. Verify no full page reload

**Expected Result:** All settings tab switches happen via SPA navigation with no page reloads.

---

## TC-009: Profile Settings

**Description:** User can update their display name.

**Steps:**
1. Log in and navigate to `/settings/profile`
2. Note the current display name value
3. Clear the name field and enter a new display name
4. Click "Save" / submit the form
5. Verify a success indicator appears (toast, inline message, or field border)
6. Refresh the page
7. Verify the new display name persists

**Expected Result:** Display name is saved and persists across page loads.

---

## TC-010: Appearance (Theme Switching)

**Description:** User can switch between light, dark, and system themes.

**Steps:**
1. Log in and navigate to settings (appearance section)
2. Select "Dark" theme
3. Verify the UI immediately switches to dark mode (check `<html>` class or CSS variables)
4. Verify no page reload occurs
5. Select "Light" theme
6. Verify the UI immediately switches to light mode
7. Verify no page reload occurs
8. Select "System" theme
9. Verify the UI matches the OS preference (or reverts to default)

**Expected Result:** Theme changes apply immediately without page reload.

---

## TC-011: Security - Password Management

**Description:** User can set or change their password.

**Steps:**
1. Log in and navigate to `/settings/security`
2. Locate the password section

**Set password (if no password exists):**
3. Enter new password
4. Enter confirm password
5. Submit
6. Verify success message

**Change password (if password exists):**
3. Enter current password
4. Enter new password
5. Enter confirm password
6. Submit
7. Verify success message
8. Log out
9. Log in with the new password
10. Verify login succeeds

**Expected Result:** Password is set/changed successfully. Login with new password works.

---

## TC-012: Security - TOTP Two-Factor Authentication

**Description:** User can set up, enable, and disable TOTP 2FA.

**Steps:**
1. Log in and navigate to `/settings/security`
2. Locate the 2FA / TOTP section
3. Click "Enable 2FA" / "Set up"
4. Verify a QR code and/or secret key is displayed
5. Extract the TOTP secret from the displayed data
6. Generate a valid TOTP code from the secret
7. Enter the code in the verification field
8. Submit
9. Verify 2FA is now shown as enabled
10. Log out
11. Log in with email and password
12. Verify a TOTP code prompt appears
13. Enter a valid TOTP code
14. Verify login completes successfully

**Disable TOTP:**
15. Navigate to `/settings/security`
16. Click "Disable 2FA"
17. Confirm the action (enter password or TOTP code if required)
18. Verify 2FA is now shown as disabled
19. Log out and log in again
20. Verify no TOTP prompt appears

**Expected Result:** TOTP can be enabled and disabled. When enabled, login requires a valid TOTP code.

---

## TC-013: Security - Passkey Management

**Description:** User can add, rename, and delete passkeys.

**Steps:**

**Add passkey:**
1. Log in and navigate to `/settings/security`
2. Locate the passkeys section
3. Click "Add Passkey"
4. Complete the WebAuthn registration prompt
5. Verify the new passkey appears in the list with a default name

**Rename passkey:**
6. Click the rename / edit button on the passkey
7. Enter a new name
8. Submit
9. Verify the passkey list shows the updated name

**Delete passkey:**
10. Click the delete button on a passkey
11. Confirm the deletion
12. Verify the passkey is removed from the list
13. Verify the deleted passkey can no longer be used for login

**Expected Result:** Passkeys can be registered, renamed, and deleted. Deleted passkeys no longer work for authentication.

**Notes:** Requires HTTPS or localhost for WebAuthn.

---

## TC-014: Security - Session Management

**Description:** User can view active sessions, revoke individual sessions, and log out all sessions.

**Steps:**

**View sessions:**
1. Log in and navigate to `/settings/security`
2. Locate the sessions section
3. Verify at least one active session is listed (the current one)
4. Verify session details are shown (e.g., creation time, current indicator)

**Revoke a session:**
5. Log in from a second browser/context to create a second session
6. Return to the first session's security settings
7. Verify two sessions are listed
8. Click "Revoke" on the other session (not the current one)
9. Verify it is removed from the list
10. Switch to the second browser/context
11. Attempt to navigate to a protected route
12. Verify the second session is now invalid (redirected to login)

**Logout all sessions:**
13. Log in from multiple contexts
14. Click "Logout All Sessions" (or equivalent)
15. Verify all other sessions are invalidated
16. Verify the current session remains active (or user is also logged out and must re-authenticate)

**Expected Result:** Sessions are visible, individually revocable, and bulk logout works.

---

## TC-015: Workspace Settings

**Description:** Workspace owner/admin can change the workspace name.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/workspace`
3. Note the current workspace name
4. Clear the name field and enter a new workspace name
5. Click "Save" / submit
6. Verify a success indicator appears
7. Refresh the page
8. Verify the new workspace name persists
9. Verify the workspace name is updated in the sidebar/header

**Expected Result:** Workspace name is updated and reflected across the UI.

---

## TC-016: Team - Invite Member

**Description:** Workspace owner invites a new member by email.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Click "Invite" or locate the invite form
4. Enter an email address
5. Submit the invitation
6. Verify the invited email appears in the pending invitations list (or member list as pending)
7. Verify an invitation email is sent (check test mailbox)
8. As the invited user, complete signup via the invitation
9. Return to the owner's team settings
10. Verify the new user now appears as an active member in the list

**Expected Result:** Invitation is sent, and upon signup the invited user appears in the workspace member list.

---

## TC-017: Team - Remove Member

**Description:** Workspace owner removes a member from the workspace.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Verify at least one non-owner member exists in the list
4. Click "Remove" (or equivalent) on a member
5. Confirm the removal action
6. Verify the member is removed from the list
7. Log in as the removed user
8. Verify they no longer have access to the workspace

**Expected Result:** The removed member loses access to the workspace immediately.

---

## TC-018: Team - Change Role

**Description:** Workspace owner promotes or demotes a member between admin and user roles.

**Steps:**

**Promote to admin:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Find a member with "member" role
4. Change their role to "admin" (via dropdown, button, or role selector)
5. Verify the role change is reflected in the UI
6. Refresh the page
7. Verify the role persists

**Demote to member:**
8. Find the admin user
9. Change their role back to "member"
10. Verify the role change is reflected in the UI

**Expected Result:** Roles can be changed between admin and member by the workspace owner.

---

## TC-019: Team - Transfer Ownership

**Description:** Workspace owner initiates ownership transfer; recipient accepts.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Click "Transfer Ownership" (or equivalent)
4. Select or enter the recipient (must be an existing member)
5. Confirm the transfer initiation
6. Verify emails are sent to both the current owner and the recipient
7. Extract the acceptance link from the recipient's email
8. As the recipient, navigate to the acceptance link
9. Confirm the ownership transfer
10. Verify ownership has changed:
    - The recipient is now shown as "owner"
    - The previous owner is now shown as "admin" (or "member")
11. Verify the new owner can access owner-only settings

**Expected Result:** Ownership transfers after both parties are notified and the recipient accepts.

---

## TC-020: Workspace Switcher

**Description:** User who belongs to multiple workspaces can switch between them.

**Steps:**
1. Create a user who is a member of at least two workspaces
2. Log in as that user
3. Open the user menu (or workspace selector)
4. Verify both workspaces are listed
5. Click on the other workspace
6. Verify the UI switches to the selected workspace context
7. Verify workspace-specific data (e.g., workspace name in header) reflects the switch
8. Switch back to the original workspace
9. Verify it switches back correctly

**Expected Result:** Workspace switcher shows all user workspaces and switching updates the active context.

---

## TC-021: Account Recovery (Forgot Password)

**Description:** User recovers their account via the forgot password flow.

**Steps:**
1. Navigate to `/login`
2. Click "Forgot password?" link
3. Enter the account email address
4. Submit the recovery request
5. Verify a "check your email" message is displayed
6. Retrieve the recovery email from the test mailbox
7. Extract the recovery link
8. Navigate to the recovery link
9. Verify the password reset form is displayed
10. Enter a new password and confirm it
11. Submit the form
12. Verify success message or redirect to login
13. Navigate to `/login`
14. Log in with the email and new password
15. Verify login succeeds

**Expected Result:** Password is reset via email link. Login with new password works.

---

## TC-022: MCP Endpoint

**Description:** The MCP server responds to initialize, tools/list, and tools/call requests.

**Steps:**
1. Send a POST request to `/mcp` with the JSON-RPC `initialize` method:
   ```json
   {
     "jsonrpc": "2.0",
     "id": 1,
     "method": "initialize",
     "params": {
       "protocolVersion": "2025-03-26",
       "capabilities": {},
       "clientInfo": { "name": "test", "version": "1.0" }
     }
   }
   ```
2. Verify the response contains `serverInfo` and `capabilities`
3. Extract the session ID from the `Mcp-Session-Id` response header
4. Send a POST to `/mcp` with the session ID header and `tools/list` method:
   ```json
   {
     "jsonrpc": "2.0",
     "id": 2,
     "method": "tools/list",
     "params": {}
   }
   ```
5. Verify the response contains a `tools` array
6. Verify the `hello` tool is present in the list
7. Send a POST to `/mcp` with `tools/call` for the `hello` tool:
   ```json
   {
     "jsonrpc": "2.0",
     "id": 3,
     "method": "tools/call",
     "params": {
       "name": "hello",
       "arguments": {}
     }
   }
   ```
8. Verify a successful response with content

**Expected Result:** MCP endpoint handles the full lifecycle: initialize, list tools, call a tool.

---

## TC-023: Personal Mode

**Description:** In personal mode, no login is required and a user/workspace is auto-provisioned.

**Steps:**
1. Start the server with `TRAKKT_MODE=personal`
2. Navigate to the root URL (e.g., `http://localhost:8003`)
3. Verify no login page is shown
4. Verify the user lands directly in the application
5. Verify a user menu is visible (auto-provisioned user)
6. Navigate to `/settings/profile`
7. Verify settings are accessible without authentication
8. Verify a workspace exists (auto-provisioned)

**Expected Result:** Personal mode skips all authentication. The app is immediately usable with an auto-provisioned user and workspace.

---

## TC-024: Self-Hosted Mode Without SMTP

**Description:** In self-hosted mode without SMTP configured, the first user can create an account directly without email verification.

**Steps:**
1. Start the server with `TRAKKT_MODE=self_hosted` and no `SMTP_HOST` / `SMTP_USER` environment variables
2. Navigate to `/signup`
3. Enter an email address
4. Verify that instead of "check your email," the user is taken directly to the account completion form (or account is created immediately)
5. Enter display name and password
6. Submit
7. Verify the account is created and the user is logged in
8. Verify the user lands in the application with a workspace

**Expected Result:** Without SMTP, self-hosted mode allows direct account creation without email verification.

---

## TC-025: Onboarding Flow

**Description:** New user completing signup is guided through the onboarding flow before reaching the main app.

**Steps:**
1. Create a new user account (via direct DB seeding or API)
2. Log in as the new user
3. Verify the user is redirected to `/onboarding`
4. Complete each onboarding step (fill required fields, click "Next" / "Continue")
5. After the final step, verify the user is redirected to the main app (e.g., `/settings/profile`)
6. Log out and log back in
7. Verify the user is NOT shown onboarding again (goes directly to the app)

**Expected Result:** Onboarding is shown once for new users, and once completed it is not repeated.

---

## TC-026: Rate Limiting (Login)

**Description:** Excessive failed login attempts trigger rate limiting.

**Steps:**
1. Navigate to `/login`
2. Enter a valid email with an incorrect password
3. Submit the form repeatedly (exceed the configured rate limit threshold, e.g., 10 attempts)
4. Verify that after exceeding the threshold, subsequent attempts return a rate limit error (429 or inline error message)
5. Verify the error message indicates too many attempts (e.g., "Too many login attempts, please try again later")
6. Wait for the rate limit window to expire (or use a fresh IP/context)
7. Verify login works again with correct credentials

**Expected Result:** Login is rate-limited after excessive failed attempts. The user is informed and can retry after the cooldown period.

---

## TC-027: Rate Limiting (Signup)

**Description:** Excessive signup attempts from the same source trigger rate limiting.

**Steps:**
1. Navigate to `/signup`
2. Submit the signup form repeatedly with different email addresses (exceed the configured threshold)
3. Verify that after exceeding the threshold, subsequent attempts return a rate limit error
4. Verify the error message indicates too many attempts

**Expected Result:** Signup is rate-limited to prevent abuse.

---

## TC-028: Token Refresh (Silent Re-authentication)

**Description:** An expired access token is automatically refreshed without user interaction.

**Steps:**
1. Log in as an existing user
2. Verify the user is authenticated (can access `/settings/profile`)
3. Expire the access token (e.g., manipulate the cookie expiry or wait for short-lived token to expire in a test configuration)
4. Attempt to perform an authenticated action (e.g., navigate to `/settings/profile` or call a server function)
5. Verify the action succeeds without showing a login page
6. Verify the access token cookie has been refreshed (new expiry time)

**Expected Result:** Expired access tokens are silently refreshed using the refresh token. The user experiences no interruption.

---

## TC-029: Token Refresh - Rotation Detection

**Description:** Reusing a refresh token that has already been rotated invalidates the entire token family.

**Steps:**
1. Log in as an existing user
2. Capture the current refresh token value
3. Trigger a token refresh (manually call `POST /api/v1/auth/refresh`)
4. Verify a new refresh token is issued (the old one is consumed)
5. Attempt to use the OLD refresh token again (replay attack)
6. Verify the server rejects the old token
7. Verify the ENTIRE token family is invalidated (the new token also stops working)
8. Verify the user must re-authenticate (next protected page request redirects to `/login`)

**Expected Result:** Token reuse detection invalidates all tokens in the family, forcing re-authentication.

---

## TC-030: Workspace Settings - Model Selection

**Description:** Workspace owner can change the configured Anthropic model.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/workspace`
3. Locate the model selection field
4. Note the current model value
5. Select a different model from the available options
6. Click "Save" / submit
7. Verify a success indicator appears
8. Refresh the page
9. Verify the new model selection persists

**Expected Result:** Workspace model selection is saved and persists across page loads.

---

## TC-031: Workspace Settings - ChartML Configuration

**Description:** Workspace owner can update the ChartML palette configuration.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/workspace`
3. Locate the ChartML configuration section
4. Modify the palette setting (e.g., select a different color palette)
5. Click "Save" / submit
6. Verify a success indicator appears
7. Refresh the page
8. Verify the new ChartML configuration persists

**Expected Result:** ChartML palette configuration is saved and persists.

---

## TC-032: Team - Cancel Invitation

**Description:** Workspace owner can cancel a pending invitation before it is accepted.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Invite a new email address (create a pending invitation)
4. Verify the invitation appears in the pending invitations list
5. Click "Cancel" (or equivalent) on the pending invitation
6. Confirm the cancellation if prompted
7. Verify the invitation is removed from the pending list
8. (If possible) Verify that navigating to the invitation link now fails or shows an error

**Expected Result:** Pending invitations can be cancelled by the workspace owner, and cancelled invitation links no longer work.

---

## TC-033: Team - Invitation Expiry

**Description:** Workspace invitations expire after the configured time period (7 days).

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Invite a new email address
4. Verify the invitation appears as pending
5. Advance the system clock or modify the invitation's `created_at` timestamp in the database to be older than 7 days
6. Attempt to use the invitation token (via the API or a signup attempt)
7. Verify the invitation is rejected as expired
8. Return to the team settings page
9. Verify the expired invitation is shown as expired or removed from the active list

**Expected Result:** Invitations older than 7 days are rejected and no longer usable.

---

## TC-034: Team - Ownership Transfer Decline

**Description:** The recipient of an ownership transfer can decline it.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Initiate an ownership transfer to another member
4. Verify the pending transfer is shown in the UI
5. Log in as the transfer recipient
6. Navigate to the accept ownership page (`/accept-ownership/:transfer_id`)
7. Click "Decline" (or equivalent)
8. Verify the transfer is marked as declined
9. Log in as the original owner
10. Verify the owner still retains ownership
11. Verify the transfer is no longer shown as pending

**Expected Result:** Ownership transfers can be declined by the recipient. The original owner retains ownership.

---

## TC-035: Team - Ownership Transfer Expiry

**Description:** Ownership transfers expire after the configured time period (7 days).

**Steps:**
1. Log in as workspace owner
2. Initiate an ownership transfer to another member
3. Verify the pending transfer exists
4. Advance the system clock or modify the transfer's `created_at` timestamp to be older than 7 days
5. As the recipient, attempt to accept the transfer
6. Verify the transfer is rejected as expired
7. Verify the original owner still retains ownership
8. Verify the expired transfer is no longer shown as pending

**Expected Result:** Ownership transfers expire after 7 days and can no longer be accepted.

---

## TC-036: Team - Ownership Transfer Cancellation

**Description:** The workspace owner can cancel a pending ownership transfer before it is accepted.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Initiate an ownership transfer to another member
4. Verify the pending transfer is shown
5. Click "Cancel Transfer" (or equivalent)
6. Confirm the cancellation
7. Verify the transfer is removed from the pending list
8. As the recipient, attempt to accept the now-cancelled transfer
9. Verify it is rejected

**Expected Result:** Pending ownership transfers can be cancelled by the initiating owner.

---

## TC-037: Role-Based Access Control - Member Cannot Access Admin Operations

**Description:** A workspace member (non-admin, non-owner) cannot perform admin-only operations.

**Steps:**
1. Log in as a user with "member" role in a workspace
2. Navigate to `/settings/workspace`
3. Verify the workspace name field is read-only or the save button is disabled (or the page shows an access denied message)
4. Attempt to update the workspace name via the server function directly (if possible)
5. Verify the server rejects the request with a permission error
6. Navigate to `/settings/team`
7. Verify the "Invite" button is not shown or is disabled
8. Verify the role change controls are not visible for other members
9. Verify "Remove" buttons are not shown for other members

**Expected Result:** Members without admin/owner role cannot modify workspace settings, invite members, change roles, or remove members.

---

## TC-038: Role-Based Access Control - Admin Permissions

**Description:** A workspace admin can manage members but cannot transfer ownership or perform owner-only actions.

**Steps:**
1. Log in as a user with "admin" role in a workspace
2. Navigate to `/settings/team`
3. Verify the admin CAN invite new members
4. Verify the admin CAN remove members (but not the owner)
5. Verify the admin CAN change member roles (but cannot change the owner's role)
6. Verify "Transfer Ownership" is NOT available to the admin
7. Navigate to `/settings/workspace`
8. Verify the admin CAN update the workspace name

**Expected Result:** Admins have elevated permissions for team and workspace management but cannot transfer ownership.

---

## TC-039: Workspace Switcher - Context Isolation

**Description:** Switching workspaces fully isolates data between workspace contexts.

**Steps:**
1. Create a user who is a member of two workspaces (Workspace A and Workspace B)
2. As workspace owner of A, invite a unique member (member-A-only)
3. As workspace owner of B, invite a different unique member (member-B-only)
4. Log in as the multi-workspace user
5. Switch to Workspace A
6. Navigate to `/settings/team`
7. Verify member-A-only is listed, member-B-only is NOT listed
8. Switch to Workspace B
9. Navigate to `/settings/team`
10. Verify member-B-only is listed, member-A-only is NOT listed

**Expected Result:** Team member lists and workspace data are fully isolated between workspaces.

---

## TC-040: Duplicate Workspace Invitation

**Description:** Inviting an email that already has a pending invitation is handled gracefully.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Invite `duplicate@example.com`
4. Verify the invitation appears as pending
5. Attempt to invite `duplicate@example.com` again
6. Verify an appropriate error message is displayed (e.g., "Already invited" or "Invitation already pending")
7. Verify only one invitation exists for that email in the list

**Expected Result:** Duplicate invitations are rejected with a clear error message.

---

## TC-041: Invite Existing Workspace Member

**Description:** Inviting an email that already belongs to the workspace is handled gracefully.

**Steps:**
1. Log in as workspace owner
2. Navigate to `/settings/team`
3. Note an existing member's email address
4. Attempt to invite that email address
5. Verify an appropriate error message is displayed (e.g., "Already a member")
6. Verify no duplicate invitation is created

**Expected Result:** Inviting an existing member is rejected with a clear error message.

---

## TC-042: Session Persistence Across Browser Restart

**Description:** User session survives browser closure (persistent auth cookies).

**Steps:**
1. Log in as an existing user
2. Verify authentication succeeds
3. Close the browser context (simulating browser restart) and create a new context with the same cookie storage
4. Navigate to `/settings/profile`
5. Verify the user is still authenticated (no redirect to login)
6. Verify the correct user context is loaded (display name matches)

**Expected Result:** Auth cookies persist across browser sessions, keeping the user logged in.

---

## Notes for Test Implementation

- **Parallel isolation:** Each test should use a fresh database state or unique email addresses to avoid cross-test contamination.
- **Email capture:** Use Mailpit, MailHog, or a similar tool to capture and inspect outbound emails during tests.
- **WebAuthn mocking:** Use Playwright's CDP session to create virtual authenticators for passkey tests (TC-004, TC-013).
- **Theme detection:** Check for `class="dark"` on `<html>` element or inspect CSS custom property values.
- **SPA navigation assertion:** Store a reference to a persistent DOM element before navigation and verify it remains in the DOM after navigation (proving no full page reload occurred).
- **Mode switching:** TC-023 and TC-024 require restarting the server with different environment variables. Use separate test suites or Playwright projects for these.
