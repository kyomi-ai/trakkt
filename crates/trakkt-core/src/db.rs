// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database pool abstraction — supports Postgres and SQLite at runtime.

use std::str::FromStr;

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

/// Runtime-selected database pool.
///
/// `DATABASE_URL` prefix determines which backend is used:
/// - `postgresql://` or `postgres://` → Postgres
/// - Anything else (e.g. `sqlite://path.db`) → SQLite
#[derive(Clone, Debug)]
pub enum DbPool {
    Postgres(PgPool),
    Sqlite(SqlitePool),
}

impl DbPool {
    /// Connect to the database and run migrations.
    pub async fn connect(url: &str) -> crate::Result<Self> {
        if url.starts_with("postgresql://") || url.starts_with("postgres://") {
            let pool = PgPoolOptions::new()
                .max_connections(10)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(url)
                .await?;
            sqlx::migrate!("../../apps/server/migrations").run(&pool).await?;
            tracing::info!("PostgreSQL pool connected, migrations applied");
            Ok(Self::Postgres(pool))
        } else {
            // SQLite WAL mode serialises writes; a single connection avoids contention.
            let opts = sqlx::sqlite::SqliteConnectOptions::from_str(url)?
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await?;
            sqlx::query("PRAGMA journal_mode=WAL")
                .execute(&pool)
                .await
                .map_err(crate::Error::Sqlx)?;
            sqlx::query("PRAGMA foreign_keys=ON")
                .execute(&pool)
                .await
                .map_err(crate::Error::Sqlx)?;
            sqlx::migrate!("../../apps/server/migrations-sqlite")
                .run(&pool)
                .await?;
            tracing::info!("SQLite pool connected, migrations applied");
            Ok(Self::Sqlite(pool))
        }
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }

    pub fn is_sqlite(&self) -> bool {
        matches!(self, Self::Sqlite(_))
    }

    /// Open a transaction on whichever backend is active.
    ///
    /// See [`DbTx`] for the rules that apply while one is open — in particular
    /// that the pool must not be queried until the transaction ends.
    pub async fn begin(&self) -> crate::Result<DbTx> {
        match self {
            Self::Postgres(pg) => {
                let tx = pg.begin().await.map_err(|e| {
                    crate::Error::Internal(format!("failed to begin transaction: {e}"))
                })?;
                Ok(DbTx::Postgres(tx))
            }
            Self::Sqlite(sq) => {
                let tx = sq.begin().await.map_err(|e| {
                    crate::Error::Internal(format!("failed to begin transaction: {e}"))
                })?;
                Ok(DbTx::Sqlite(tx))
            }
        }
    }

    /// Extract the inner `PgPool` for Postgres-only code paths.
    ///
    /// Panics if called on a SQLite pool.
    pub fn pg_pool(&self) -> &PgPool {
        match self {
            Self::Postgres(pg) => pg,
            Self::Sqlite(_) => panic!("pg_pool() called on SQLite pool"),
        }
    }
}

/// An open database transaction — the transactional counterpart of [`DbPool`].
///
/// One variant per backend, each holding that backend's `sqlx::Transaction`.
/// Statements run on it through the `tx_*` macros ([`tx_execute!`],
/// [`tx_fetch_one!`], [`tx_fetch_optional!`], [`tx_fetch_all!`],
/// [`tx_fetch_scalar!`], [`tx_with!`]), which mirror the `db_*` macros one for
/// one and take a `&mut DbTx` in place of a `&DbPool`.
///
/// # Rollback
///
/// Dropping a `DbTx` without calling [`DbTx::commit`] rolls the transaction
/// back, so propagating an error out of a function that owns one — `?` on any
/// statement — is a rollback. [`DbTx::rollback`] states that explicitly for
/// error paths that are not a `?` (a business-rule rejection, say).
///
/// # No pool access while open
///
/// The SQLite pool is pinned to a single connection (see [`DbPool::connect`]),
/// which the transaction holds for its whole lifetime. Any `db_*` call issued
/// against the pool before the transaction ends therefore waits on a connection
/// that cannot be released until it does. Everything needed between `begin` and
/// `commit` must go through the transaction, and side effects that touch the
/// pool — WebSocket broadcasts, notification writes — belong after the commit.
///
/// The same property is what makes `SELECT last_insert_rowid()` correct on
/// SQLite: the INSERT and the rowid read share one connection by construction
/// rather than by pool configuration.
pub enum DbTx {
    Postgres(sqlx::Transaction<'static, sqlx::Postgres>),
    Sqlite(sqlx::Transaction<'static, sqlx::Sqlite>),
}

impl DbTx {
    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }

    /// Commit the transaction, making every statement run on it durable.
    pub async fn commit(self) -> crate::Result<()> {
        match self {
            Self::Postgres(tx) => tx.commit().await,
            Self::Sqlite(tx) => tx.commit().await,
        }
        .map_err(|e| crate::Error::Internal(format!("failed to commit transaction: {e}")))
    }

    /// Roll the transaction back, discarding every statement run on it.
    pub async fn rollback(self) -> crate::Result<()> {
        match self {
            Self::Postgres(tx) => tx.rollback().await,
            Self::Sqlite(tx) => tx.rollback().await,
        }
        .map_err(|e| crate::Error::Internal(format!("failed to roll back transaction: {e}")))
    }
}

/// Backwards-compatible pool constructor for tests.
pub async fn create_pool(url: &str) -> crate::Result<DbPool> {
    DbPool::connect(url).await
}

/// Run a quick connectivity check — useful for health endpoints.
pub async fn ping(pool: &DbPool) -> crate::Result<()> {
    match pool {
        DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(pg)
                .await?;
        }
        DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(sq)
                .await?;
        }
    }
    Ok(())
}

/// Backend-agnostic query result for `db_execute!`.
///
/// Wraps the `rows_affected()` value from either `PgQueryResult` or
/// `SqliteQueryResult` so callers don't need to care about the backend.
pub struct DbQueryResult {
    rows: u64,
}

impl DbQueryResult {
    pub fn from_pg(r: sqlx::postgres::PgQueryResult) -> Self {
        Self { rows: r.rows_affected() }
    }
    pub fn from_sqlite(r: sqlx::sqlite::SqliteQueryResult) -> Self {
        Self { rows: r.rows_affected() }
    }
    pub fn rows_affected(&self) -> u64 {
        self.rows
    }
}

/// Execute a query that returns typed rows via `query_as`. Fetches all rows.
#[macro_export]
macro_rules! db_fetch_all {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_all(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_all(sq).await,
        }
    }
}

/// Fetch exactly one row.
#[macro_export]
macro_rules! db_fetch_one {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_one(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_one(sq).await,
        }
    }
}

/// Fetch zero or one row.
#[macro_export]
macro_rules! db_fetch_optional {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_optional(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_optional(sq).await,
        }
    }
}

/// Execute a query without returning rows (INSERT, UPDATE, DELETE).
///
/// Returns `Result<DbQueryResult, sqlx::Error>` which provides
/// `rows_affected()` regardless of the backend.
#[macro_export]
macro_rules! db_execute {
    ($pool:expr, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query($query)$(.bind($bind))*.execute(pg).await
                    .map($crate::db::DbQueryResult::from_pg),
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query($query)$(.bind($bind))*.execute(sq).await
                    .map($crate::db::DbQueryResult::from_sqlite),
        }
    }
}

/// Dispatch to whichever pool variant is active.
///
/// Use this when both the Postgres and SQLite arms would be **identical**
/// except for the pool variable name.  The closure receives `p` which is
/// either a `&PgPool` or a `&SqlitePool`.
///
/// ```rust,ignore
/// let row = db_with_pool!(pool, |p| {
///     sqlx::query("SELECT 1").fetch_one(p).await?
/// });
/// ```
#[macro_export]
macro_rules! db_with_pool {
    ($pool:expr, |$p:ident| $body:expr) => {
        match &$pool {
            $crate::db::DbPool::Postgres($p) => { $body }
            $crate::db::DbPool::Sqlite($p) => { $body }
        }
    }
}

/// Fetch a single scalar value.
#[macro_export]
macro_rules! db_fetch_scalar {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_scalar::<_, $type>($query)$(.bind($bind))*.fetch_one(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_scalar::<_, $type>($query)$(.bind($bind))*.fetch_one(sq).await,
        }
    }
}

// ─── Transaction-scoped query macros ─────────────────────────────────────────
//
// One per `db_*` macro above, with the same argument order and the same return
// type. The first argument is a `&mut DbTx` rather than a `&DbPool`: write
// `tx_execute!(&mut tx, …)` where `tx` is owned, or `tx_execute!(&mut *tx, …)`
// to reborrow a `&mut DbTx` parameter.

/// Execute a query on an open transaction without returning rows.
///
/// Transaction-scoped [`db_execute!`].
#[macro_export]
macro_rules! tx_execute {
    ($tx:expr, $query:expr $(, $bind:expr)*) => {
        match $tx {
            $crate::db::DbTx::Postgres(t) =>
                sqlx::query($query)$(.bind($bind))*.execute(&mut **t).await
                    .map($crate::db::DbQueryResult::from_pg),
            $crate::db::DbTx::Sqlite(t) =>
                sqlx::query($query)$(.bind($bind))*.execute(&mut **t).await
                    .map($crate::db::DbQueryResult::from_sqlite),
        }
    }
}

/// Fetch exactly one typed row on an open transaction.
///
/// Transaction-scoped [`db_fetch_one!`].
#[macro_export]
macro_rules! tx_fetch_one {
    ($tx:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match $tx {
            $crate::db::DbTx::Postgres(t) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_one(&mut **t).await,
            $crate::db::DbTx::Sqlite(t) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_one(&mut **t).await,
        }
    }
}

/// Fetch zero or one typed row on an open transaction.
///
/// Transaction-scoped [`db_fetch_optional!`].
#[macro_export]
macro_rules! tx_fetch_optional {
    ($tx:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match $tx {
            $crate::db::DbTx::Postgres(t) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_optional(&mut **t).await,
            $crate::db::DbTx::Sqlite(t) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_optional(&mut **t).await,
        }
    }
}

/// Fetch all typed rows on an open transaction.
///
/// Transaction-scoped [`db_fetch_all!`].
#[macro_export]
macro_rules! tx_fetch_all {
    ($tx:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match $tx {
            $crate::db::DbTx::Postgres(t) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_all(&mut **t).await,
            $crate::db::DbTx::Sqlite(t) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_all(&mut **t).await,
        }
    }
}

/// Fetch a single scalar value on an open transaction.
///
/// Transaction-scoped [`db_fetch_scalar!`].
#[macro_export]
macro_rules! tx_fetch_scalar {
    ($tx:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match $tx {
            $crate::db::DbTx::Postgres(t) =>
                sqlx::query_scalar::<_, $type>($query)$(.bind($bind))*.fetch_one(&mut **t).await,
            $crate::db::DbTx::Sqlite(t) =>
                sqlx::query_scalar::<_, $type>($query)$(.bind($bind))*.fetch_one(&mut **t).await,
        }
    }
}

/// Dispatch to whichever transaction variant is active, for queries whose binds
/// are built at runtime.
///
/// Transaction-scoped [`db_with_pool!`]. The closure receives `e`, a
/// `&mut PgConnection` or `&mut SqliteConnection` inside the transaction.
#[macro_export]
macro_rules! tx_with {
    ($tx:expr, |$e:ident| $body:expr) => {
        match $tx {
            $crate::db::DbTx::Postgres(t) => { let $e = &mut **t; $body }
            $crate::db::DbTx::Sqlite(t) => { let $e = &mut **t; $body }
        }
    }
}

/// Build a SQL IN clause with numbered placeholders starting at `start_idx`.
/// Returns the clause `($N, $N+1, ...)` and the next available index.
pub fn in_clause_placeholders(count: usize, start_idx: usize) -> (String, usize) {
    let placeholders: Vec<String> = (0..count)
        .map(|i| format!("${}", start_idx + i))
        .collect();
    let clause = format!("({})", placeholders.join(", "));
    (clause, start_idx + count)
}
