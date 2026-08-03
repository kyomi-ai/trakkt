// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scaffolding for tests that need a real database.
//!
//! Enabled by the `test-helpers` feature, which only the workspace's own
//! `dev-dependencies` turn on — production builds never compile this module.
//!
//! # What belongs here
//!
//! The pool, and the tenancy rows every other entity hangs off: a user, a
//! workspace (plus the owner's membership), and a team. These have no service
//! layer visible from `trakkt-core`, so the SQL lives here, mirroring
//! `trakkt_server::auto_provision_personal_mode` — the production path that
//! creates the same minimal set, in the same order the foreign keys require.
//!
//! [`dual_backend`] also lives here: it runs one test body against both SQLite
//! and a real Postgres. Each seed function below takes a `&DbPool` and asks it
//! which backend it is, so the same three calls seed either half of a
//! [`dual_backend_test!`](crate::dual_backend_test) pair.
//!
//! # What does not, and why
//!
//! Issues, labels and statuses are *not* seeded here, deliberately. Every one
//! of them already has a creator in `trakkt-auth` (`issue_service::create_issue`,
//! `label_service::create_label`, `status_service::seed_default_statuses`), and
//! those functions own more than an `INSERT`: per-team number allocation,
//! default-status lookup, the `sync_log` append, the transaction boundary. A
//! second implementation here would drift from the first and would seed rows
//! the real services never would.
//!
//! Calling them from here is not possible either: `trakkt-auth` depends on
//! `trakkt-core`, so a dependency in the other direction is a cycle Cargo
//! rejects — a feature gate does not change that, and a dev-dependency is not
//! visible to this crate's `lib` target. Tests that want domain entities should
//! seed the tenancy with these helpers and then call the real services, which
//! is also the only way to get rows the rest of the system agrees with.
//!
//! # Example
//!
//! ```rust,ignore
//! let db = trakkt_core::test_helpers::test_pool().await?;
//! seed_user(&db, "usr_1", "one@example.test").await?;
//! seed_workspace(&db, "ws_1", "usr_1").await?;
//! seed_team(&db, "team_1", "ws_1", "TST").await?;
//! // Domain entities go through the real services:
//! trakkt_auth::team_service::add_team_member(&db, "team_1", "usr_1", "lead", "ws_1").await?;
//! trakkt_auth::status_service::seed_default_statuses(&db, "ws_1").await?;
//! ```

pub mod dual_backend;

use crate::db::DbPool;
use crate::db_execute;
use crate::sql_compat;
use crate::Result;

/// A migrated, empty, in-memory SQLite database.
///
/// Each call gets its own database, so tests sharing a process never see each
/// other's rows. Runs the real `migrations-sqlite` set — roughly 0.6s, which is
/// the price of testing against the schema that ships rather than a hand-rolled
/// approximation of it.
pub async fn test_pool() -> Result<DbPool> {
    DbPool::connect("sqlite::memory:").await
}

/// Insert an active, verified user.
///
/// `email` is `UNIQUE`, so callers seeding more than one user must vary it.
pub async fn seed_user(db: &DbPool, user_id: &str, email: &str) -> Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let bool_true = sql_compat::bool_true(is_pg);

    let sql = format!(
        "INSERT INTO users (user_id, email, name, verified, active, created_at, updated_at) \
         VALUES ($1, $2, $3, {bool_true}, {bool_true}, {now}, {now})"
    );
    db_execute!(db, &sql, user_id, email, "Test User")?;

    Ok(())
}

/// Insert an active workspace owned by `owner_user_id`, and enrol that user in
/// it as a `workspace_admin`.
///
/// The membership row is not optional dressing: `workspace_users` is what
/// `WebSocketManager::broadcast_to_workspace` reads to decide who receives a
/// change, so a workspace without it is one no member can be notified about.
///
/// `seed_user` must have run for `owner_user_id` — `workspaces.owner_user_id`
/// is a foreign key, and the in-memory pool enforces them.
pub async fn seed_workspace(db: &DbPool, workspace_id: &str, owner_user_id: &str) -> Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let bool_true = sql_compat::bool_true(is_pg);

    let workspace_sql = format!(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id, status, created_at, updated_at) \
         VALUES ($1, $2, $3, 'active', {now}, {now})"
    );
    db_execute!(db, &workspace_sql, workspace_id, "Test Workspace", owner_user_id)?;

    let membership_sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
         VALUES ($1, $2, 'workspace_admin', {bool_true}, {now})"
    );
    db_execute!(db, &membership_sql, workspace_id, owner_user_id)?;

    Ok(())
}

/// Insert a team into `workspace_id`.
///
/// Membership is *not* created here. `trakkt_auth::team_service::add_team_member`
/// owns that: besides the row it writes the `sync_log` entry the clients need,
/// and skipping it would seed a team that behaves differently from every team
/// the product creates. Note that a team with no members is invisible to
/// `list_teams(db, workspace_id, Some(user_id))` — and therefore to sync
/// bootstrap — which is usually not what a test wants.
///
/// `key` is unique per workspace (e.g. `"TST"`); it prefixes issue identifiers.
pub async fn seed_team(db: &DbPool, team_id: &str, workspace_id: &str, key: &str) -> Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);

    let sql = format!(
        "INSERT INTO teams (team_id, workspace_id, name, key, created_at) \
         VALUES ($1, $2, $3, $4, {now})"
    );
    db_execute!(db, &sql, team_id, workspace_id, "Test Team", key)?;

    Ok(())
}
