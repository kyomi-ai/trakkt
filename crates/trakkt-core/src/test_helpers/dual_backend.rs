// SPDX-License-Identifier: AGPL-3.0-or-later

//! Runs one test body against both backends: SQLite and a real Postgres.
//!
//! # Why this exists
//!
//! Production runs Postgres; every other test in the workspace runs SQLite. A
//! defect confined to a Postgres query arm — a placeholder index, a missing
//! cast, `RETURNING` versus `last_insert_rowid()` — compiles, passes clippy and
//! ships, because nothing ever executes it. Two such bugs have shipped already
//! (`sort_order` decoded as `f64` from a `FLOAT4` column, twice).
//!
//! The unit of work here is therefore a *pair* of tests over a *single* body.
//! Two independent files, one per backend, would answer "does some Postgres
//! test exist" — which is not the question. The question is whether the same
//! assertion holds on both, and only a shared body can keep answering it after
//! someone edits one half.
//!
//! # How to tell whether the Postgres half ran
//!
//! From the test output, and only from the test output:
//!
//! ```text
//! test sync_entry_id_addresses_the_committed_row::sqlite   ... ok
//! test sync_entry_id_addresses_the_committed_row::postgres ... ignored, requires a live Postgres — see crates/trakkt-core/src/test_helpers/dual_backend.rs
//! ```
//!
//! `ignored` in the per-test line, and a non-zero `N ignored` in the summary,
//! is the suite saying plainly that nothing was verified on Postgres. When the
//! Postgres half does run it prints `ok` and the summary reports `0 ignored`.
//!
//! This mirrors [`crate::redis`]'s reasoning verbatim, and for the same reason:
//! a harness that catches the connection error and returns early prints `ok`,
//! which is indistinguishable from a run where Postgres was present and the
//! code genuinely worked. Every machine without Postgres would then report
//! success for a path it never touched — the exact failure mode this module was
//! written to remove. Do not "improve" the `#[ignore]` into a silent skip.
//!
//! # Running the Postgres half
//!
//! ```text
//! podman run -d --name trakkt-postgres-test -p 5436:5432 \
//!     -e POSTGRES_USER=trakkt -e POSTGRES_PASSWORD=trakkt -e POSTGRES_DB=postgres \
//!     docker.io/library/postgres:16
//!
//! cargo test -p trakkt-server --test postgres_dialect -- --include-ignored
//! ```
//!
//! See `docs/CODING_STANDARDS.md`, "The Postgres dialect suite".

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;
use sqlx::postgres::PgPoolOptions;

use crate::db::DbPool;
use crate::db_execute;

/// Where the Postgres half connects when `TEST_DATABASE_URL` is unset.
///
/// Port 5436, not 5432 or 5433. These projects allocate database ports in a
/// ladder so a test run can never reach a server the developer is using for
/// something else, and the rungs below are taken: on the machine this was
/// written for, 5433 and 5434 belong to another project's development and test
/// databases and 5435 is Trakkt's own development Postgres. 5436 is Trakkt's
/// test rung. The CI job publishes its service container on the same port, so
/// the default is the configuration CI exercises rather than an untested
/// fallback.
///
/// The database named here is only ever connected to in order to issue
/// `CREATE DATABASE` / `DROP DATABASE`; no test reads or writes a table in it.
const DEFAULT_PG_TEST_URL: &str = "postgres://trakkt:trakkt@localhost:5436/postgres";

/// How long to wait for the maintenance connection before declaring Postgres
/// unreachable.
///
/// A refused connection fails immediately; this bounds the case that does not —
/// a host that accepts the SYN and never answers, where sqlx would otherwise
/// wait out its own much longer timeouts and the test would appear to hang.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The maintenance URL the Postgres half uses, from `TEST_DATABASE_URL` or
/// [`DEFAULT_PG_TEST_URL`].
///
/// This is not the URL tests run against: each test gets its own database
/// created through this connection (see [`PgTestDatabase`]).
pub fn pg_maintenance_url() -> String {
    std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_PG_TEST_URL.to_string())
}

/// Replace the database component of a Postgres URL, keeping everything else.
fn with_database(url: &str, database: &str) -> String {
    let mut parsed = url::Url::parse(url)
        .unwrap_or_else(|e| panic!("TEST_DATABASE_URL is not a valid URL ({url}): {e}"));
    parsed.set_path(database);
    parsed.to_string()
}

/// Reject a maintenance URL that does not name a Postgres server.
///
/// [`DbPool::connect`] chooses its backend from exactly this prefix, and every
/// other scheme falls through to SQLite — so a `TEST_DATABASE_URL` pointing at
/// anything else would hand the "postgres" half of every pair a SQLite pool and
/// print `ok` for a run that touched no Postgres arm at all. That is the silent
/// pass this whole module exists to make impossible, so it is refused here
/// rather than discovered later.
fn require_postgres_url(url: &str) {
    assert!(
        url.starts_with("postgres://") || url.starts_with("postgresql://"),
        "TEST_DATABASE_URL must name a Postgres server, got {url}. DbPool::connect \
         treats every other scheme as SQLite, so the Postgres half of each test \
         pair would silently run on SQLite and report success for a Postgres arm \
         it never executed."
    );
}

/// A throwaway Postgres database with the production migrations applied.
///
/// Per test, not per run: the rollback tests install rejection triggers on
/// `sync_log`, which one shared database would leak into every test running
/// beside them.
pub struct PgTestDatabase {
    pool: DbPool,
    maintenance_url: String,
    name: String,
}

impl PgTestDatabase {
    /// Create the database and apply the migrations, or panic saying how to get
    /// a Postgres.
    ///
    /// Migrations are applied by [`DbPool::connect`] — the same call, over the
    /// same `apps/server/migrations` directory, that the server makes at
    /// startup. Nothing here reimplements or trims the schema, so a migration
    /// that exists on only one dialect fails a test here rather than in
    /// production.
    ///
    /// `CREATE DATABASE` is issued without any cross-test serialisation.
    /// Measured against the PostgreSQL 16.14 this suite targets, 24 concurrent
    /// `CREATE DATABASE` statements and 24 concurrent
    /// `DROP DATABASE … WITH (FORCE)` statements all succeeded; no test here
    /// connects to `template1`, which is the documented way for a concurrent
    /// creation to be refused.
    pub async fn create() -> Self {
        let maintenance_url = pg_maintenance_url();
        require_postgres_url(&maintenance_url);

        // Hex from a v4 UUID: `CREATE DATABASE` cannot take a bind parameter,
        // so the name is interpolated, and it is generated rather than derived
        // from anything a caller supplies.
        let name = format!("trakkt_test_{}", uuid::Uuid::new_v4().simple());

        let admin = connect_maintenance(&maintenance_url).await;
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("creating throwaway test database {name}: {e}"));
        admin.close().await;

        let pool = DbPool::connect(&with_database(&maintenance_url, &name))
            .await
            .unwrap_or_else(|e| {
                panic!("connecting to {name} and applying apps/server/migrations: {e}")
            });

        assert!(
            pool.is_postgres(),
            "the Postgres half must hold a Postgres pool; {name} opened as SQLite"
        );

        Self { pool, maintenance_url, name }
    }

    /// The pool tests run against.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// The generated database name, for tests that assert on teardown.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Close the pool and drop the database.
    ///
    /// Consumes `self` because the pool must be closed before the drop: a
    /// `DROP DATABASE` with a session still attached fails.
    pub async fn teardown(self) {
        self.pool.pg_pool().close().await;

        let admin = connect_maintenance(&self.maintenance_url).await;
        // FORCE terminates any session that outlived the pool close — a
        // connection sqlx had not finished tearing down, say. Without it a
        // stray session leaves the database behind for the developer to find
        // weeks later.
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)", self.name))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("dropping throwaway test database {}: {e}", self.name));
        admin.close().await;
    }
}

/// Whether a database of this name exists on the maintenance server.
///
/// Exists so a test can assert that [`PgTestDatabase::teardown`] really removed
/// its database — including on the path where the body panicked — without
/// reimplementing the maintenance connection.
pub async fn database_exists(name: &str) -> bool {
    let url = pg_maintenance_url();
    require_postgres_url(&url);
    let admin = connect_maintenance(&url).await;

    let found: Option<String> = sqlx::query_scalar("SELECT datname FROM pg_database WHERE datname = $1")
        .bind(name)
        .fetch_optional(&admin)
        .await
        .unwrap_or_else(|e| panic!("querying pg_database for {name}: {e}"));

    admin.close().await;
    found.is_some()
}

/// Open the single-connection maintenance pool, or panic with instructions.
async fn connect_maintenance(url: &str) -> sqlx::PgPool {
    let connect = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(CONNECT_TIMEOUT)
        .connect(url);

    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(pool)) => pool,
        Ok(Err(e)) => panic!("{}", unreachable_message(url, &e.to_string())),
        Err(_elapsed) => panic!(
            "{}",
            unreachable_message(url, &format!("no answer within {}s", CONNECT_TIMEOUT.as_secs()))
        ),
    }
}

fn unreachable_message(url: &str, cause: &str) -> String {
    format!(
        "no Postgres answered at {url} ({cause}).\n\
         This test is #[ignore]d precisely so that it never runs — and never \
         reports success — on a machine without one. It was asked to run \
         anyway, so it fails rather than pretending.\n\
         Start one with:\n\
         \x20 podman run -d --name trakkt-postgres-test -p 5436:5432 \
         -e POSTGRES_USER=trakkt -e POSTGRES_PASSWORD=trakkt \
         -e POSTGRES_DB=postgres docker.io/library/postgres:16\n\
         or point TEST_DATABASE_URL at an existing server. The URL is used only \
         to CREATE and DROP throwaway `trakkt_test_*` databases; no table in the \
         database it names is read or written."
    )
}

/// Run `body` against a freshly migrated in-memory SQLite database.
pub async fn on_sqlite<F, Fut>(body: F)
where
    F: FnOnce(DbPool) -> Fut,
    Fut: Future<Output = ()>,
{
    let db = super::test_pool()
        .await
        .expect("opening an in-memory SQLite pool and applying migrations-sqlite");
    body(db).await;
}

/// Run `body` against a freshly created, freshly migrated Postgres database,
/// then drop that database.
///
/// The body's panic — which is how a failed assertion arrives — is caught so
/// that teardown still runs, and then re-raised unchanged. Without the catch,
/// every failing Postgres test would leave its database behind; with anything
/// less than `resume_unwind` the failure would be softened, which is the one
/// thing this module must never do. The payload is passed through untouched, so
/// libtest reports the original assertion message and location.
///
/// The catch depends on tests unwinding rather than aborting. `cargo test`
/// builds with the `dev` profile, which does not set `panic = "abort"` (only
/// `[profile.release]` in the workspace `Cargo.toml` does), and
/// `panicking_body_still_drops_its_database` asserts the behaviour directly
/// rather than trusting that reading.
pub async fn on_postgres<F, Fut>(body: F)
where
    F: FnOnce(DbPool) -> Fut,
    Fut: Future<Output = ()>,
{
    let db = PgTestDatabase::create().await;
    let outcome = AssertUnwindSafe(body(db.pool().clone())).catch_unwind().await;
    db.teardown().await;

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Make every `sync_log` INSERT fail, at the database, on whichever backend is
/// active.
///
/// This is the injection mechanism for the rollback contract — a sync entry
/// write that fails after the mutation statements have already run — and it is
/// the one piece of that test that is genuinely dialect-specific, which is why
/// it lives here rather than in a test module of one crate:
///
/// - SQLite: `RAISE(ABORT, …)` from a `BEGIN … END` trigger body. It fails the
///   statement and backs out its changes while leaving the surrounding
///   transaction open and usable.
/// - Postgres: `RAISE EXCEPTION` from a `plpgsql` trigger function. There is no
///   `RAISE(ABORT)` and no statement-level equivalent — the exception aborts the
///   whole transaction, which then rejects every further statement with
///   `25P02 in_failed_sql_transaction` until it is rolled back.
///
/// That difference does not change what the rollback tests assert, because the
/// services under test propagate the error with `?` the moment it arrives and
/// so never issue another statement on the poisoned transaction. It does mean a
/// service that tried to *continue* past a rejected sync entry would fail
/// differently on the two backends — which is itself worth catching, and is a
/// reason to run these tests on both.
///
/// The service code is untouched and knows nothing about any of it: the failure
/// arrives as an ordinary sqlx error from a real schema object.
pub async fn reject_sync_log_inserts(db: &DbPool) {
    if db.is_postgres() {
        db_execute!(
            db,
            "CREATE FUNCTION reject_sync_log() RETURNS trigger LANGUAGE plpgsql AS $fn$ \
             BEGIN RAISE EXCEPTION 'sync_log insert rejected'; END; $fn$"
        )
        .expect("create the Postgres sync_log rejection trigger function");

        db_execute!(
            db,
            "CREATE TRIGGER reject_sync_log BEFORE INSERT ON sync_log \
             FOR EACH ROW EXECUTE FUNCTION reject_sync_log()"
        )
        .expect("install the Postgres sync_log rejection trigger");
    } else {
        db_execute!(
            db,
            "CREATE TRIGGER reject_sync_log BEFORE INSERT ON sync_log \
             BEGIN SELECT RAISE(ABORT, 'sync_log insert rejected'); END"
        )
        .expect("install the SQLite sync_log rejection trigger");
    }
}

/// Declare one test body and run it on both backends.
///
/// ```rust,ignore
/// dual_backend_test! {
///     /// What the assertion is for.
///     async fn a_committed_row_is_readable(db) {
///         // `db` is a `&DbPool`, Postgres in one run and SQLite in the other.
///     }
/// }
/// ```
///
/// expands to a module named after the body containing two tests:
///
/// - `a_committed_row_is_readable::sqlite` — always runs.
/// - `a_committed_row_is_readable::postgres` — `#[ignore]`d, so `cargo test`
///   with no Postgres running reports it as `ignored` rather than passing it.
///   Run it with `-- --include-ignored`.
///
/// Both call the same `body`, so the two backends cannot be given different
/// assertions without deleting one of them — which is the whole point.
///
/// The expansion names `::tokio::test`, so the calling crate needs `tokio` with
/// the `macros` and `rt` features as a (dev-)dependency.
#[macro_export]
macro_rules! dual_backend_test {
    (
        $(#[$meta:meta])*
        async fn $name:ident($db:ident) $body:block
    ) => {
        $(#[$meta])*
        mod $name {
            use super::*;

            async fn body($db: &$crate::db::DbPool) $body

            #[::tokio::test]
            async fn sqlite() {
                $crate::test_helpers::dual_backend::on_sqlite(|db| async move {
                    body(&db).await
                })
                .await;
            }

            #[::tokio::test]
            #[ignore = "requires a live Postgres — see crates/trakkt-core/src/test_helpers/dual_backend.rs"]
            async fn postgres() {
                $crate::test_helpers::dual_backend::on_postgres(|db| async move {
                    body(&db).await
                })
                .await;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `with_database` has to produce a URL sqlx can parse, which means the
    /// database has to land in the path with its leading slash. `Url::set_path`
    /// supplies that slash for a bare name; asserting it here means a change in
    /// the `url` crate's normalisation surfaces as this one-line failure rather
    /// than as "database trakkt_test_… does not exist" inside every Postgres
    /// test.
    #[test]
    fn with_database_replaces_only_the_database_name() {
        assert_eq!(
            with_database("postgres://trakkt:trakkt@localhost:5436/postgres", "trakkt_test_abc"),
            "postgres://trakkt:trakkt@localhost:5436/trakkt_test_abc"
        );
    }

    /// Query parameters carry sslmode and friends; replacing the database must
    /// not drop them.
    #[test]
    fn with_database_keeps_the_query_string() {
        assert_eq!(
            with_database("postgres://u:p@db.example:5432/maint?sslmode=require", "trakkt_test_1"),
            "postgres://u:p@db.example:5432/trakkt_test_1?sslmode=require"
        );
    }

    #[test]
    fn a_sqlite_url_is_refused_as_a_postgres_maintenance_url() {
        let err = std::panic::catch_unwind(|| require_postgres_url("sqlite::memory:"))
            .expect_err("a SQLite URL must be rejected as a Postgres maintenance URL");
        let message = err
            .downcast_ref::<String>()
            .map(String::as_str)
            .expect("the rejection panics with a formatted String message");
        assert!(
            message.contains("must name a Postgres server"),
            "the message has to say what is wrong with the URL; got: {message}"
        );
    }

    #[test]
    fn a_postgres_url_is_accepted_under_either_scheme() {
        require_postgres_url("postgres://trakkt:trakkt@localhost:5436/postgres");
        require_postgres_url("postgresql://trakkt:trakkt@localhost:5436/postgres");
    }
}
