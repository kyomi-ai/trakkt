// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Postgres arms of the dual-dialect query layer, executed.
//!
//! Production runs Postgres. Every other test in the workspace runs SQLite, so
//! a defect confined to a `is_pg` branch — a placeholder index, a missing cast,
//! `RETURNING` versus `last_insert_rowid()` — compiles, passes clippy, and
//! ships without anything having run it. This file is where those branches are
//! run.
//!
//! Every test below is declared with
//! [`dual_backend_test!`](trakkt_core::dual_backend_test), which expands one
//! body into two tests: `<name>::sqlite`, which always runs, and
//! `<name>::postgres`, which is `#[ignore]`d so that a machine with no Postgres
//! reports `ignored` rather than a green tick for a path it never touched. Run
//! the Postgres half with:
//!
//! ```text
//! cargo test -p trakkt-server --test postgres_dialect -- --include-ignored
//! ```
//!
//! See `crates/trakkt-core/src/test_helpers/dual_backend.rs` for the harness and
//! `docs/CODING_STANDARDS.md`, "The Postgres dialect suite", for the container.
//!
//! # Why this file lives in `apps/server`
//!
//! It needs `trakkt-auth`'s services *and* `trakkt-core`'s `test-helpers`
//! feature at once. `trakkt-auth` depends on `trakkt-core`, so the helpers
//! cannot call back into the services, and `trakkt-auth` does not enable the
//! feature. `trakkt-server` already depends on both — the same reason
//! `cascade_delete_sync.rs` is here — so nothing has to be rearranged to make
//! the pair reachable from one test body.

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;

use trakkt_auth::sync_log_service::{get_entries_since, write_sync_entry_in_tx};
use trakkt_core::db::in_clause_placeholders;
use trakkt_core::test_helpers::dual_backend::{
    database_exists, on_postgres, reject_sync_log_inserts,
};
use trakkt_core::test_helpers::{seed_team, seed_user, seed_workspace};
use trakkt_core::{
    db_execute, db_fetch_scalar, dual_backend_test, sql_compat, tx_execute, tx_fetch_all,
    tx_fetch_one, tx_fetch_optional, tx_fetch_scalar, tx_with, DbPool,
};
use trakkt_types::enums::ActionSource;
use trakkt_types::models::{CreateIssueParams, Issue, IssueUpdate};
use trakkt_types::sync::{entity_types, SyncActionType};

const USER: &str = "usr_dialect";
const WORKSPACE: &str = "ws_dialect";
const TEAM: &str = "team_dialect";
const TEAM_KEY: &str = "DIA";

/// The tenancy rows and default statuses every service call below needs.
///
/// The tenancy comes from `trakkt_core::test_helpers`, which asks the pool which
/// backend it is and picks the dialect's `NOW()`/boolean literals accordingly —
/// so the same three calls seed either half of the pair. The statuses come from
/// the real `status_service`, because `create_issue` resolves a default status
/// and a hand-written INSERT here would seed a set the product never produces.
async fn seed_tenancy(db: &DbPool) {
    seed_user(db, USER, "dialect@example.test")
        .await
        .expect("seed the workspace owner");
    seed_workspace(db, WORKSPACE, USER)
        .await
        .expect("seed the workspace and the owner's membership");
    seed_team(db, TEAM, WORKSPACE, TEAM_KEY)
        .await
        .expect("seed the team issues are numbered within");
    trakkt_auth::status_service::seed_default_statuses(db, WORKSPACE)
        .await
        .expect("seed the default statuses create_issue resolves against");
}

/// Create one issue through the real service.
async fn create_issue(db: &DbPool, title: &str) -> Issue {
    trakkt_auth::issue_service::create_issue(
        db,
        &CreateIssueParams {
            workspace_id: WORKSPACE.to_owned(),
            team_id: TEAM.to_owned(),
            creator_id: USER.to_owned(),
            title: title.to_owned(),
            description: Some(format!("the body of {title}")),
            priority: 0,
            assignee_id: None,
            due_date: None,
            label_ids: Vec::new(),
            project_id: None,
            milestone_id: None,
            estimate: None,
        },
        None,
    )
    .await
    .expect("create the issue the rollback assertions are made against")
}

// ─── Migrations ──────────────────────────────────────────────────────────────

dual_backend_test! {
    /// Every migration file on this backend's side is recorded as applied.
    ///
    /// The pool is opened by `DbPool::connect`, which is the same call — over
    /// the same directory — the server makes at startup, so this asserts the
    /// production migration path ran to completion rather than that some
    /// approximation of the schema exists. TRA-9989 shipped a Postgres
    /// migration nothing could run; on the Postgres half this fails on that
    /// migration rather than in production.
    ///
    /// Comparing the recorded versions against the directory listing, rather
    /// than against a count baked in here, means a new migration needs no edit
    /// to this test and a migration that silently never ran still fails it.
    async fn every_migration_file_is_recorded_as_applied(db) {
        let is_pg = db.is_postgres();
        let dir = if is_pg { "migrations" } else { "migrations-sqlite" };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);

        let mut on_disk: Vec<i64> = std::fs::read_dir(&path)
            .unwrap_or_else(|e| panic!("reading the migration directory {}: {e}", path.display()))
            .map(|entry| {
                entry.unwrap_or_else(|e| {
                    panic!("reading an entry of {}: {e}", path.display())
                })
            })
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sql"))
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let (version, _) = name.split_once('_').unwrap_or_else(|| {
                    panic!("migration {name} has no <version>_<description>.sql version prefix")
                });
                version.parse::<i64>().unwrap_or_else(|e| {
                    panic!("migration {name} has a non-numeric version prefix {version:?}: {e}")
                })
            })
            .collect();
        on_disk.sort_unstable();

        assert!(
            !on_disk.is_empty(),
            "no .sql files found in {} — this assertion cannot guard a directory it \
             cannot see",
            path.display()
        );

        // `success` is BOOLEAN on Postgres and INTEGER on SQLite, so the literal
        // it is compared against is dialect-dependent like every other one here.
        #[derive(sqlx::FromRow)]
        struct VersionRow {
            version: i64,
        }
        let sql = format!(
            "SELECT version FROM _sqlx_migrations WHERE success = {} ORDER BY version",
            sql_compat::bool_true(is_pg)
        );
        let applied: Vec<i64> = trakkt_core::db_fetch_all!(db, VersionRow, &sql)
            .expect("read the versions sqlx recorded as applied")
            .into_iter()
            .map(|row| row.version)
            .collect();

        assert_eq!(
            applied, on_disk,
            "every migration in {} must be recorded as applied by DbPool::connect; \
             a version present on disk and missing here never ran",
            path.display()
        );
    }
}

// ─── write_sync_entry_in_tx: RETURNING sync_id / last_insert_rowid() ─────────

dual_backend_test! {
    /// The id `write_sync_entry_in_tx` hands back addresses the row that
    /// committed.
    ///
    /// This is the assertion the Postgres `RETURNING sync_id` arm has never had
    /// run against it. Asserting the id is merely non-zero would pass for any
    /// id the database felt like returning; reading `entity_id` back *by* that
    /// id is what makes it a statement about the committed row. Two entries in
    /// one transaction, so an arm that returned a stale or shared id — the
    /// failure mode `last_insert_rowid()` has on a pool with more than one
    /// connection — cannot satisfy both.
    async fn sync_entry_id_addresses_the_committed_row(db) {
        seed_tenancy(db).await;

        let mut tx = db.begin().await.expect("open the transaction both entries are written on");
        let first = write_sync_entry_in_tx(
            &mut tx,
            entity_types::ISSUE,
            "iss_first",
            WORKSPACE,
            None,
            SyncActionType::Insert,
            Some(serde_json::json!({ "title": "first" })),
        )
        .await
        .expect("write the first sync entry on the open transaction");
        let second = write_sync_entry_in_tx(
            &mut tx,
            entity_types::ISSUE,
            "iss_second",
            WORKSPACE,
            None,
            SyncActionType::Update,
            Some(serde_json::json!({ "title": "second" })),
        )
        .await
        .expect("write the second sync entry on the open transaction");
        tx.commit().await.expect("commit both entries");

        assert_ne!(
            first, second,
            "each entry in one transaction must be given its own id"
        );

        for (sync_id, expected_entity) in [(first, "iss_first"), (second, "iss_second")] {
            let entity_id: String = db_fetch_scalar!(
                db,
                String,
                "SELECT entity_id FROM sync_log WHERE sync_id = $1",
                sync_id
            )
            .expect("read the committed sync_log row the returned id names");
            assert_eq!(
                entity_id, expected_entity,
                "sync_id {sync_id} must address the row it was returned for — a \
                 client told to resume from an id belonging to a different row \
                 skips everything between"
            );
        }

        // The ids are also what a reconnecting client resumes from, so they have
        // to be readable through the production delta read, not only by direct
        // lookup.
        let entries = get_entries_since(db, WORKSPACE, USER, first - 1, 10)
            .await
            .expect("replay the delta a reconnecting client would receive");
        let seen: Vec<(i64, &str)> = entries
            .iter()
            .map(|action| (action.sync_id, action.entity_id.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![(first, "iss_first"), (second, "iss_second")],
            "the delta read must return both committed entries under the ids \
             write_sync_entry_in_tx returned"
        );
    }
}

// ─── The rollback contract ───────────────────────────────────────────────────

dual_backend_test! {
    /// A `sync_log` insert that fails takes the mutation down with it.
    ///
    /// The injection is a real trigger installed by
    /// `dual_backend::reject_sync_log_inserts`, and it is the one genuinely
    /// dialect-specific piece: SQLite raises with `RAISE(ABORT, …)` from a
    /// `BEGIN … END` trigger body, Postgres with `RAISE EXCEPTION` from a
    /// `plpgsql` trigger function. The service is untouched and knows nothing
    /// about either — the failure reaches it as an ordinary sqlx error from a
    /// real schema object.
    ///
    /// What is asserted is the same on both: the caller sees the failure, and
    /// the title is unchanged. A committed title change with no `sync_log` row
    /// to replay it is invisible to every future delta, so every other client
    /// would keep showing the old title forever.
    async fn a_rejected_sync_entry_rolls_back_the_issue_update(db) {
        seed_tenancy(db).await;
        let issue = create_issue(db, "Original title").await;

        reject_sync_log_inserts(db).await;

        let err = trakkt_auth::issue_service::update_issue(
            db,
            WORKSPACE,
            TEAM_KEY,
            issue.number,
            &IssueUpdate {
                title: Some("Renamed".to_string()),
                ..Default::default()
            },
            Some(USER),
            ActionSource::User,
            None,
            None,
        )
        .await
        .expect_err("an update whose sync entry cannot be written must fail");

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the caller must see the sync entry failure rather than a swallowed \
             warning; got: {err}"
        );

        let title: String = db_fetch_scalar!(
            db,
            String,
            "SELECT title FROM issues WHERE issue_id = $1",
            &issue.issue_id
        )
        .expect("read the issue's title back after the failed update");
        assert_eq!(
            title, "Original title",
            "the title change must be rolled back — a committed change with no \
             sync entry never reaches another client"
        );

        let orphans: i64 = db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM sync_log WHERE entity_id = $1 AND action = $2",
            &issue.issue_id,
            "update"
        )
        .expect("count the update's sync entries");
        assert_eq!(
            orphans, 0,
            "the rejected insert must leave no partial entry behind"
        );
    }
}

// ─── The six tx_* macros ─────────────────────────────────────────────────────

/// A `sync_log` row as the transaction macros read it back.
///
/// `data` is cast to TEXT by every query below, because on Postgres it is JSONB
/// and sqlx does not decode that into `String` without the cast
/// (`docs/CODING_STANDARDS.md`).
#[derive(sqlx::FromRow, Debug, PartialEq, Eq)]
struct SyncLogProbe {
    entity_id: String,
    action: String,
    data: Option<String>,
}

impl SyncLogProbe {
    /// The `data` column parsed back into a JSON value.
    ///
    /// Compared as a value, never as bytes. Postgres stores JSONB parsed and
    /// re-serialises it on the way out, so a payload written as
    /// `{"title":"alpha"}` reads back as `{"title": "alpha"}`; SQLite stores the
    /// TEXT verbatim and returns exactly what went in. The bytes therefore
    /// legitimately differ between the two halves of every pair, and only the
    /// decoded value is a thing both backends can be held to. Production agrees:
    /// `SyncLogRow::into_sync_action` parses this column rather than comparing
    /// it.
    fn payload(&self) -> Option<serde_json::Value> {
        self.data.as_deref().map(|raw| {
            serde_json::from_str(raw)
                .unwrap_or_else(|e| panic!("parsing the sync_log data column {raw:?}: {e}"))
        })
    }
}

dual_backend_test! {
    /// All six `tx_*` macros, on one open transaction, on both backends.
    ///
    /// `tx_execute!`, `tx_fetch_one!`, `tx_fetch_optional!`, `tx_fetch_all!`,
    /// `tx_fetch_scalar!` and `tx_with!` each expand to a two-arm match, and
    /// until this ran the Postgres arm of all six was dead code in every test.
    /// They are exercised together on a single transaction rather than one per
    /// test because that is how services use them, and because it also asserts
    /// that each one leaves the transaction usable by the next.
    async fn the_six_tx_macros_round_trip_on_an_open_transaction(db) {
        seed_tenancy(db).await;

        let is_pg = db.is_postgres();
        let now = sql_compat::now(is_pg);
        let data_expr = sql_compat::cast_to_json(is_pg, "$5");
        let data_as_text = sql_compat::cast_to_text(is_pg, "data");

        let insert = format!(
            "INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data, created_at) \
             VALUES ($1, $2, $3, $4, {data_expr}, {now})"
        );

        let mut tx = db.begin().await.expect("open the transaction the six macros run on");

        // 1. tx_execute!
        for (entity_id, action, payload) in [
            ("iss_alpha", "insert", r#"{"title":"alpha"}"#),
            ("iss_beta", "update", r#"{"title":"beta"}"#),
        ] {
            let result = tx_execute!(
                &mut tx,
                &insert,
                entity_types::ISSUE,
                entity_id,
                WORKSPACE,
                action,
                payload
            )
            .expect("insert a probe row on the open transaction");
            assert_eq!(
                result.rows_affected(),
                1,
                "tx_execute! must report the one row it inserted"
            );
        }

        // 2. tx_fetch_scalar!
        let count: i64 = tx_fetch_scalar!(
            &mut tx,
            i64,
            "SELECT COUNT(*) FROM sync_log WHERE workspace_id = $1",
            WORKSPACE
        )
        .expect("count the probe rows on the open transaction");
        assert_eq!(count, 2, "tx_fetch_scalar! must see both uncommitted inserts");

        // 3. tx_fetch_one!
        let select_one = format!(
            "SELECT entity_id, action, {data_as_text} AS data FROM sync_log WHERE entity_id = $1"
        );
        let alpha: SyncLogProbe = tx_fetch_one!(&mut tx, SyncLogProbe, &select_one, "iss_alpha")
            .expect("fetch the alpha probe row on the open transaction");
        assert_eq!(
            (alpha.entity_id.as_str(), alpha.action.as_str()),
            ("iss_alpha", "insert"),
            "tx_fetch_one! must return the row it was asked for"
        );
        assert_eq!(
            alpha.payload(),
            Some(serde_json::json!({ "title": "alpha" })),
            "tx_fetch_one! must return the JSON payload as written"
        );

        // 4. tx_fetch_optional! — present, then absent.
        let beta: Option<SyncLogProbe> =
            tx_fetch_optional!(&mut tx, SyncLogProbe, &select_one, "iss_beta")
                .expect("look for the beta probe row on the open transaction");
        assert_eq!(
            beta.map(|row| row.action),
            Some("update".to_string()),
            "tx_fetch_optional! must find a row that exists"
        );

        let missing: Option<SyncLogProbe> =
            tx_fetch_optional!(&mut tx, SyncLogProbe, &select_one, "iss_nonexistent")
                .expect("look for a row that was never inserted");
        assert!(
            missing.is_none(),
            "tx_fetch_optional! must return None rather than erroring on no rows"
        );

        // 5. tx_fetch_all!
        let select_all = format!(
            "SELECT entity_id, action, {data_as_text} AS data FROM sync_log \
             WHERE workspace_id = $1 ORDER BY entity_id"
        );
        let all: Vec<SyncLogProbe> =
            tx_fetch_all!(&mut tx, SyncLogProbe, &select_all, WORKSPACE)
                .expect("fetch every probe row on the open transaction");
        assert_eq!(
            all.iter().map(|row| row.entity_id.as_str()).collect::<Vec<_>>(),
            vec!["iss_alpha", "iss_beta"],
            "tx_fetch_all! must return both rows in the requested order"
        );
        assert_eq!(
            all.iter().map(SyncLogProbe::payload).collect::<Vec<_>>(),
            vec![
                Some(serde_json::json!({ "title": "alpha" })),
                Some(serde_json::json!({ "title": "beta" })),
            ],
            "tx_fetch_all! must carry each row's own payload"
        );

        // 6. tx_with! — the runtime-bind form. `in_clause_placeholders` numbers
        //    the binds `$1, $2`, which is exactly the numbering a Postgres-only
        //    off-by-one would break; SQLite accepts either numbering, so this
        //    arm is the one that can catch it.
        let wanted = ["iss_alpha", "iss_beta"];
        let (clause, next) = in_clause_placeholders(wanted.len(), 1);
        assert_eq!(next, wanted.len() + 1, "the next free bind index follows the clause");
        let in_sql = format!("SELECT COUNT(*) FROM sync_log WHERE entity_id IN {clause}");
        let matched: i64 = tx_with!(&mut tx, |e| {
            let mut query = sqlx::query_scalar::<_, i64>(&in_sql);
            for entity_id in wanted {
                query = query.bind(entity_id);
            }
            query.fetch_one(&mut *e).await
        })
        .expect("run the runtime-bound IN query on the open transaction");
        assert_eq!(
            matched, 2,
            "tx_with!'s numbered placeholders must bind to the values in order"
        );

        tx.commit().await.expect("commit the probe rows");

        let committed: i64 = db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM sync_log WHERE workspace_id = $1",
            WORKSPACE
        )
        .expect("count the probe rows on the pool after the commit");
        assert_eq!(
            committed, 2,
            "everything the macros wrote must survive the commit"
        );
    }
}

// ─── sql_compat ──────────────────────────────────────────────────────────────

dual_backend_test! {
    /// `sql_compat::now` and `sql_compat::cast_to_json` produce SQL each
    /// backend accepts, and values each backend reads back.
    ///
    /// Both are string fragments spliced into hand-built SQL, so their unit
    /// tests can only assert what they return, never that the database accepts
    /// it. `now` has to yield a value that compares as a timestamp afterwards —
    /// `NOW()` against TIMESTAMPTZ on Postgres, `datetime('now')` against TEXT
    /// on SQLite — and `cast_to_json` has to make a bound JSON string
    /// assignable to a JSONB column on Postgres while leaving SQLite's TEXT
    /// column alone. These are the two `sync_entry_insert_sql` depends on.
    async fn sql_compat_now_and_cast_to_json_produce_working_sql(db) {
        seed_tenancy(db).await;

        let is_pg = db.is_postgres();
        let now = sql_compat::now(is_pg);
        let data_expr = sql_compat::cast_to_json(is_pg, "$5");

        let payload = serde_json::json!({ "title": "compat", "nested": { "n": 1 } });
        let payload_text = serde_json::to_string(&payload).expect("serialise the probe payload");

        let insert = format!(
            "INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data, created_at) \
             VALUES ($1, $2, $3, $4, {data_expr}, {now})"
        );
        db_execute!(
            db,
            &insert,
            entity_types::ISSUE,
            "iss_compat",
            WORKSPACE,
            "insert",
            &payload_text
        )
        .expect("insert a row whose timestamp and JSON payload come from sql_compat");

        // `now` produced something the backend treats as a timestamp: the row is
        // findable by a timestamp comparison, not merely non-empty. 120s rather
        // than a tight bound because the only thing being asserted is that the
        // value is a real current timestamp, and a loaded CI runner is allowed
        // to be slow.
        let recent_filter = sql_compat::within_seconds(is_pg, "created_at", 120);
        let recent: i64 = db_fetch_scalar!(
            db,
            i64,
            &format!("SELECT COUNT(*) FROM sync_log WHERE entity_id = $1 AND {recent_filter}"),
            "iss_compat"
        )
        .expect("compare the created_at sql_compat::now wrote against the current time");
        assert_eq!(
            recent, 1,
            "sql_compat::now must write a value the same backend compares as a \
             current timestamp"
        );

        // `cast_to_json` produced a value the column accepts and gives back
        // unchanged. Read through the production delta path, which is what any
        // client actually receives.
        let entries = get_entries_since(db, WORKSPACE, USER, 0, 10)
            .await
            .expect("read the entry back through the production delta path");
        let compat = entries
            .iter()
            .find(|action| action.entity_id == "iss_compat")
            .expect("the inserted entry is in the delta");
        assert_eq!(
            compat.data.as_ref(),
            Some(&payload),
            "sql_compat::cast_to_json must round-trip the payload — on Postgres \
             the bound TEXT has to become JSONB on the way in and TEXT again on \
             the way out"
        );
    }
}

// ─── The harness itself ──────────────────────────────────────────────────────

/// A failing Postgres body still drops its throwaway database, and still fails.
///
/// `on_postgres` catches the body's panic so teardown runs, then re-raises it.
/// Both halves of that need proving: a catch that swallowed the panic would turn
/// every failing Postgres test green, and a teardown that never ran would leave
/// a `trakkt_test_*` database behind for every failure. Asserting them means
/// deliberately failing a body, which no `dual_backend_test!` can express — so
/// this one is written out, and is Postgres-only because the SQLite half has no
/// database to leak.
///
/// `#[ignore]`d on the same terms as every Postgres half here.
#[tokio::test]
#[ignore = "requires a live Postgres — see crates/trakkt-core/src/test_helpers/dual_backend.rs"]
async fn a_panicking_postgres_body_still_drops_its_database_and_still_fails() {
    const MESSAGE: &str = "deliberate failure inside a dual-backend Postgres body";

    let recorded: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&recorded);

    let outcome = AssertUnwindSafe(on_postgres(move |db| async move {
        let name: String = db_fetch_scalar!(&db, String, "SELECT current_database()")
            .expect("read the name of the throwaway database the body is running in");
        *captured
            .lock()
            .expect("record the throwaway database name for the assertions below") = Some(name);
        panic!("{MESSAGE}");
    }))
    .catch_unwind()
    .await;

    // Checked before the recorded name, deliberately: when `on_postgres` fails
    // to reach Postgres at all it panics before the body runs, and asserting on
    // the name first would report "the body never ran" while hiding the panic
    // that says why. This way the connection failure is what the assertion
    // prints.
    let panic = outcome.expect_err(
        "on_postgres must re-raise the body's panic — a swallowed panic reports \
         every failing Postgres test as a pass",
    );
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .expect("the panic payload is the formatted String the body raised");
    assert_eq!(
        message, MESSAGE,
        "the payload must be passed through untouched so libtest reports the \
         original assertion message and location"
    );

    let name = recorded
        .lock()
        .expect("read back the recorded throwaway database name")
        .clone()
        .expect("the body ran far enough to record its database name");

    assert!(
        !database_exists(&name).await,
        "{name} must be dropped even though the body panicked — otherwise every \
         failing Postgres test leaves a database behind"
    );
}
