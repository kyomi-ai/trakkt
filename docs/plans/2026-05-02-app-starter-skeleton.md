# App Starter Skeleton

**Goal:** Get the Tane app starter to a working state where you can sign in (password, passkey, Google OAuth), see settings pages, and have MCP/OAuth infrastructure ready. No domain-specific features yet.

## Approach

For every task: `cp` the file from Kyomi, `sed` rename, then compile and fix errors. No rewriting.

## Task 1: Add back missing auth service files

Copy from `/home/jason/repos/kyomi/crates/kyomi-auth/src/` to `crates/tane-auth/src/`:

**Files to copy:**
- `google_oauth.rs` — Google OAuth login flow
- `email_service.rs` — email sending (SMTP)
- `security_service.rs` — passkey management, session listing, 2FA
- `encryption.rs` — AES-GCM encryption for workspace secrets
- `totp.rs` — TOTP 2FA
- `credential_service.rs` — manages user auth methods (password, passkey, Google)
- `onboarding_service.rs` — new user setup flow

**After copying:** `sed -i 's/kyomi/tane/g'` on each file, update `lib.rs` to export them, `cargo check -p tane-auth` and fix errors.

**Do NOT copy:** analytics, billing, chat, collection, copilot, dashboard, datasource, embedding, feedback, learning, linear, push, sql_history, stripe, subscription, watch, workspace_ai_config, workspace_secrets, connect_token. These are Kyomi domain-specific.

## Task 2: Strip Kyomi migration to auth-only tables

The baseline migration (4078 lines) creates tables for everything Kyomi needs. Extract only the auth-related tables into a clean `001_baseline.sql`:

**Tables to keep:**
- users
- workspaces
- workspace_users
- refresh_tokens
- user_auth_methods
- verification_tokens
- oauth_clients
- sync_log
- workspace_invitations
- ownership_transfers
- api_tokens
- passkey_credentials (if separate from user_auth_methods)

**Tables to remove:** Everything else (dashboards, datasources, chat_sessions, sql_history, collections, watches, push_subscriptions, etc.)

**Method:** Read the Kyomi baseline, copy only the CREATE TABLE + CREATE INDEX blocks for the tables listed above. Also copy the migrations that add columns to those tables from later migration files.

Also create the SQLite equivalent.

## Task 3: Copy Kyomi UI components library

Copy the full `kyomi-ui-components` crate as part of `tane-ui`:

`cp -r /home/jason/repos/kyomi/crates/kyomi-ui-components/src/components/* crates/tane-ui/src/components/`

These are generic UI components (Button, Modal, Card, Toast, etc.) — not domain-specific.

Also copy the component Cargo.toml deps and wire into tane-ui.

## Task 4: Copy auth pages from Kyomi UI

Copy the login/signup/auth pages:

`cp -r /home/jason/repos/kyomi/crates/kyomi-ui/src/pages/auth/* crates/tane-ui/src/pages/auth/`

These handle: login, signup, passkey flows, Google OAuth callback, account recovery.

## Task 5: Copy settings pages from Kyomi UI

Copy only the settings pages that apply to the starter:

**Copy:**
- `settings_shell.rs` — settings layout with tabs
- `profile.rs` — user profile editing
- `security/` — passkey management, session management, 2FA
- `team.rs` — workspace member management
- `workspace.rs` — workspace settings

**Don't copy:** ai.rs, analytics.rs, billing.rs, connect_*.rs, datasources.rs, push_notifications.rs, slack_connection.rs, usage.rs

## Task 6: Copy server_fns needed for settings

Copy from Kyomi's server_fns:
- `security.rs` — passkey CRUD, session listing
- `sidebar.rs` — sidebar user info (needed by layout)
- `ownership.rs` — workspace ownership transfers

These call the auth services we're copying in Task 1.

## Task 7: Wire everything together and boot

- Update `tane-ui/src/lib.rs` to export all new modules
- Update `tane-ui/src/app.rs` with routes for login, settings
- Update `apps/server/src/lib.rs` to mount all routes
- Copy Kyomi's leptos_frontend.rs patterns for SSR login
- `trunk build` the frontend
- Boot the server, test in browser

## Task 8: MCP tool registry (empty)

Wire the MCP route handler to use a real (but empty) tool registry. The MCP server should respond to initialize/tools/list but return zero tools. Domain-specific tools get added when domain code is written.

## For each task

1. `cp` file from Kyomi
2. `sed -i 's/kyomi_core/tane_core/g; s/kyomi-core/tane-core/g; s/kyomi_auth/tane_auth/g; s/kyomi-auth/tane-auth/g; s/kyomi_ui/tane_ui/g; s/kyomi-ui/tane-ui/g; s/kyomi_types/tane_types/g; s/Kyomi/Tane/g; s/kyomi/tane/g'`
3. `cargo check` and fix compile errors by removing references to missing modules
4. Do NOT rewrite any function
