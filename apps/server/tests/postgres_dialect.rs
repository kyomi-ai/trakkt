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

use std::collections::{BTreeSet, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;

use trakkt_auth::sync_log_service::{get_entries_since, write_sync_entry_in_tx, SyncAudience};
use trakkt_core::db::in_clause_placeholders;
use trakkt_core::test_helpers::dual_backend::{
    clear_sync_log_rejection, database_exists, on_postgres, reject_sync_log_inserts,
    reject_sync_log_inserts_of_type,
};
use trakkt_core::test_helpers::{seed_team, seed_user, seed_workspace, test_pool};
use trakkt_core::{
    db_execute, db_fetch_all, db_fetch_scalar, dual_backend_test, sql_compat, tx_execute,
    tx_fetch_all, tx_fetch_one, tx_fetch_optional, tx_fetch_scalar, tx_with, DbPool,
};
use trakkt_types::enums::{ActionSource, FavoriteTarget};
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
            SyncAudience::Workspace,
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
            SyncAudience::Workspace,
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

// ─── get_entries_since: the TEAM membership predicate ────────────────────────

/// The second user the TEAM visibility body needs, enrolled in `WORKSPACE`.
///
/// `seed_workspace` enrols only the owner, and `WebSocketManager` and the delta
/// read both key off `workspace_users` — so a user without this row is not a
/// member of the workspace at all, and "a workspace member who is not a team
/// member" is exactly the case under test.
async fn seed_second_workspace_member(db: &DbPool, user_id: &str) {
    seed_user(db, user_id, &format!("{user_id}@example.test"))
        .await
        .expect("seed the workspace member who joins no team");

    let now = sql_compat::now(db.is_postgres());
    let bool_true = sql_compat::bool_true(db.is_postgres());
    db_execute!(
        db,
        &format!(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
             VALUES ($1, $2, 'member', {bool_true}, {now})"
        ),
        WORKSPACE,
        user_id
    )
    .expect("enrol the second user in the workspace");
}

/// Every TEAM entry `user_id` receives from a delta-from-zero, as
/// `(entity_id, action)`.
async fn team_delta(db: &DbPool, user_id: &str) -> Vec<(String, SyncActionType)> {
    get_entries_since(db, WORKSPACE, user_id, 0, 10_000)
        .await
        .expect("read the delta stream a reconnecting client would be sent")
        .into_iter()
        .filter(|action| action.entity_type == entity_types::TEAM)
        .map(|action| (action.entity_id, action.action))
        .collect()
}

dual_backend_test! {
    /// TEAM rows that add or refresh a team reach only that team's current
    /// members; TEAM rows that remove one reach everybody.
    ///
    /// TRA-10013 put a correlated `EXISTS` over `team_members`, and a comparison
    /// against string literals for `entity_type` and `action`, into
    /// `get_entries_since` — the query every delta runs. That is new SQL on a hot
    /// path, and the two engines do not have to agree about it: Postgres decides
    /// `VARCHAR(50) <> 'team'` under its own type resolution and may plan the
    /// `EXISTS` as a subplan or a semi-join, while SQLite compares TEXT with its
    /// own affinity rules. A filter that silently matched nothing on one backend
    /// would pass every SQLite test in the workspace and ship as a disclosure, so
    /// the assertion is made against both.
    ///
    /// Driven through the real `team_service` rather than hand-written
    /// `sync_log` rows: what is being checked is which rows the product's own
    /// writers produce and how this query then treats them.
    async fn team_delta_entries_are_scoped_to_current_members(db) {
        const OUTSIDER: &str = "usr_dialect_outsider";

        seed_tenancy(db).await;
        seed_second_workspace_member(db, OUTSIDER).await;

        let team = trakkt_auth::team_service::create_team(
            db,
            &trakkt_auth::team_service::CreateTeamParams {
                workspace_id: WORKSPACE,
                name: "Members Only",
                key: "MEM",
                description: None,
                icon: None,
                creator_id: Some(USER),
            },
            None,
        )
        .await
        .expect("create the team the outsider is not a member of");

        trakkt_auth::team_service::update_team(
            db,
            &team.team_id,
            WORKSPACE,
            Some("Members Only, Renamed".to_owned()),
            None,
            None,
        )
        .await
        .expect("rename the team");

        assert_eq!(
            team_delta(db, OUTSIDER).await,
            Vec::new(),
            "a workspace member who is not a team member must receive no TEAM \
             row for it — not the create, and not the rename"
        );
        assert_eq!(
            team_delta(db, USER).await,
            vec![
                (team.team_id.clone(), SyncActionType::Insert),
                (team.team_id.clone(), SyncActionType::Update),
                (team.team_id.clone(), SyncActionType::Update),
            ],
            "the member must still receive the create, the creator's member-add \
             update and the rename"
        );

        // The delete is written after `DELETE FROM teams`, and `team_members`
        // cascades from `teams(team_id)` on both backends — so by the time this
        // row is read there is no membership left to authorise it. It has to
        // reach the member anyway, or the deleted team stays in their cache.
        trakkt_auth::team_service::delete_team(db, &team.team_id, WORKSPACE, None, None, None)
            .await
            .expect("delete the team");

        assert_eq!(
            team_delta(db, USER).await.last(),
            Some(&(team.team_id.clone(), SyncActionType::Delete)),
            "the member's stream must end with the delete even though their \
             team_members row cascaded away with the team"
        );

        let memberships: i64 = db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM team_members WHERE team_id = $1",
            &team.team_id
        )
        .expect("count the memberships left after the cascade");
        assert_eq!(
            memberships, 0,
            "the assertion above is only about the cascade if the cascade really \
             happened on this backend"
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

// ─── Schema parity between the dialects ──────────────────────────────────────
//
// TRA-9999. Everything above executes a query on both backends; this section
// compares the two *schemas* the queries run against. It exists because the
// drift it now guards had accumulated invisibly for months: measured on the
// migrated schemas, Postgres declared 78 foreign keys and SQLite 66. Twelve
// existed only on Postgres, four more differed in ON DELETE, and three TEXT
// primary keys were nullable on SQLite alone.
//
// The four differing-ON-DELETE keys were the dangerous ones. All four cascaded
// on Postgres and were NO ACTION on SQLite, so deleting the parent succeeded in
// production and was rejected outright on SQLite — a change ships green through
// CI and breaks on one backend. TRA-9989 was that shape and made an issue
// undeletable in production.
//
// The two checks below compare live schemas rather than a table of expected
// constraints written out here. A hand-maintained table would answer "does the
// schema match what someone once wrote down", which decays; comparing the two
// migrated schemas to each other answers "do the dialects still agree", needs
// no edit when a migration adds a constraint to both, and fails the moment one
// adds it to only one.

/// One foreign key, reduced to the form both dialects can be read into.
///
/// Deliberately no constraint name: Postgres names its keys
/// (`api_tokens_user_id_fkey`) and SQLite numbers them per table, so the names
/// can never match and comparing them would report drift on every row.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKey {
    table: String,
    columns: String,
    references_table: String,
    references_columns: String,
    on_delete: String,
}

impl std::fmt::Display for ForeignKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({}) -> {}({}) ON DELETE {}",
            self.table, self.columns, self.references_table, self.references_columns,
            self.on_delete
        )
    }
}

/// One column of one foreign key, as either dialect's catalogue reports it.
///
/// A row per column rather than per constraint so that a composite key is
/// assembled here, in one place, from both dialects' orderings —
/// `information_schema` and `PRAGMA foreign_key_list` agree on neither the
/// column order nor how to aggregate it in SQL.
#[derive(sqlx::FromRow)]
struct ForeignKeyColumnRow {
    table_name: String,
    /// Groups the columns of one constraint together. Dialect-local and never
    /// compared across backends — see [`ForeignKey`].
    constraint_key: String,
    ordinal: i64,
    column_name: String,
    references_table: String,
    references_column: Option<String>,
    on_delete: String,
}

/// One primary-key column and whether the schema forbids NULL in it.
#[derive(sqlx::FromRow)]
struct PrimaryKeyColumnRow {
    table_name: String,
    column_name: String,
    not_null: bool,
}

// Both dialects' queries below skip `_sqlx_migrations`. It is sqlx's own
// bookkeeping table, created by the migrator rather than declared in either
// migration directory, and its two dialects genuinely differ: `version` is
// `BIGINT PRIMARY KEY` on SQLite, which is not the exact `INTEGER` spelling a
// rowid alias needs and so carries no implicit NOT NULL, while Postgres has it
// NOT NULL. Nothing a migration can write changes that, so reporting it would
// be a permanent failure over a table this codebase does not own.

/// Every foreign key Postgres declares, one row per column.
const PG_FOREIGN_KEY_COLUMNS: &str = "\
    SELECT c.conrelid::regclass::text AS table_name, \
           c.conname AS constraint_key, \
           x.ord AS ordinal, \
           a.attname AS column_name, \
           c.confrelid::regclass::text AS references_table, \
           fa.attname AS references_column, \
           CASE c.confdeltype \
                WHEN 'a' THEN 'NO ACTION' WHEN 'r' THEN 'RESTRICT' \
                WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' \
                WHEN 'd' THEN 'SET DEFAULT' END AS on_delete \
      FROM pg_constraint c \
      JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS x(attnum, ord) ON true \
      JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = x.attnum \
      JOIN pg_attribute fa ON fa.attrelid = c.confrelid AND fa.attnum = c.confkey[x.ord] \
     WHERE c.contype = 'f' AND c.connamespace = 'public'::regnamespace \
       AND c.conrelid::regclass::text <> '_sqlx_migrations'";

/// Every foreign key SQLite declares, one row per column.
const SQLITE_FOREIGN_KEY_COLUMNS: &str = "\
    SELECT m.name AS table_name, \
           CAST(f.id AS TEXT) AS constraint_key, \
           f.seq AS ordinal, \
           f.\"from\" AS column_name, \
           f.\"table\" AS references_table, \
           f.\"to\" AS references_column, \
           f.on_delete AS on_delete \
      FROM sqlite_master AS m \
      JOIN pragma_foreign_key_list(m.name) AS f \
     WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
       AND m.name <> '_sqlx_migrations'";

/// Every primary-key column Postgres declares, and its NOT NULL.
const PG_PRIMARY_KEY_COLUMNS: &str = "\
    SELECT c.conrelid::regclass::text AS table_name, \
           a.attname AS column_name, \
           a.attnotnull AS not_null \
      FROM pg_constraint c \
      JOIN LATERAL unnest(c.conkey) AS k(attnum) ON true \
      JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum \
     WHERE c.contype = 'p' AND c.connamespace = 'public'::regnamespace \
       AND c.conrelid::regclass::text <> '_sqlx_migrations'";

/// Every primary-key column SQLite declares, and its NOT NULL.
const SQLITE_PRIMARY_KEY_COLUMNS: &str = "\
    SELECT m.name AS table_name, \
           ti.name AS column_name, \
           ti.\"notnull\" AS not_null \
      FROM sqlite_master AS m \
      JOIN pragma_table_info(m.name) AS ti \
     WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
       AND m.name <> '_sqlx_migrations' AND ti.pk > 0";

/// Foreign keys Postgres declares that SQLite deliberately does not, and why.
///
/// An entry here is a promise that the gap is understood, not that it is
/// acceptable forever. The check asserts in both directions — an entry whose
/// key has since appeared on SQLite fails just as loudly as an undocumented
/// gap, so closing the gap forces the entry to be deleted rather than leaving a
/// permanent exemption behind.
const FOREIGN_KEYS_POSTGRES_ONLY: &[(&str, &str, &str)] = &[(
    "issues",
    "milestone_id",
    "TRA-9999 closed eleven of the twelve missing keys and left this one. \
     Re-measure with `PRAGMA foreign_key_list` before attempting it rather than \
     trusting a figure here: measured on the current schema, eleven foreign \
     keys across ten tables point at `issues`, and two of those TRA-9999 added \
     itself. SQLite fires ON DELETE actions during DROP TABLE, so rebuilding \
     `issues` means rebuilding every one of those tables alongside it, on top \
     of the 19 migrations that have touched `issues`, to gain one SET NULL. \
     Both backends accept the milestone delete either way; \
     what SQLite misses is the clearing of `issues.milestone_id` that \
     `project_service::delete_milestone` leaves entirely to the schema. See the \
     closing note in \
     apps/server/migrations-sqlite/20260803100000_dual_backend_fk_parity.sql.",
)];

/// Primary-key columns SQLite leaves nullable that Postgres does not, and why.
///
/// All three are `INTEGER PRIMARY KEY AUTOINCREMENT`, i.e. rowid aliases, where
/// binding NULL is the documented way to ask for the next value. Adding NOT
/// NULL would break every insert that does so, which
/// [`rowid_alias_primary_keys_still_accept_a_null_bind`] pins down. This is not
/// the legacy quirk the check is looking for — that one is a non-INTEGER
/// PRIMARY KEY silently missing its NOT NULL, which lets a row with no primary
/// key at all be inserted, and more than one, because NULLs do not compare
/// equal.
const NULLABLE_PRIMARY_KEYS_SQLITE_ONLY: &[(&str, &str, &str)] = &[
    ("sync_log", "sync_id", "rowid alias: INTEGER PRIMARY KEY AUTOINCREMENT"),
    ("user_auth_methods", "id", "rowid alias: INTEGER PRIMARY KEY AUTOINCREMENT"),
    ("workspace_users", "id", "rowid alias: INTEGER PRIMARY KEY AUTOINCREMENT"),
];

/// Read one dialect's foreign keys, assembling multi-column keys in ordinal
/// order.
async fn foreign_keys(db: &DbPool, query: &str, dialect: &str) -> BTreeSet<ForeignKey> {
    let rows: Vec<ForeignKeyColumnRow> = db_fetch_all!(db, ForeignKeyColumnRow, query)
        .unwrap_or_else(|e| panic!("read {dialect}'s foreign keys from its catalogue: {e}"));

    assert!(
        !rows.is_empty(),
        "{dialect} reported no foreign keys at all — this check cannot compare a \
         catalogue it failed to read, and reporting agreement here would be the \
         silent pass the whole suite exists to prevent"
    );

    let mut grouped: HashMap<(String, String), Vec<ForeignKeyColumnRow>> = HashMap::new();
    for row in rows {
        grouped
            .entry((row.table_name.clone(), row.constraint_key.clone()))
            .or_default()
            .push(row);
    }

    grouped
        .into_values()
        .map(|mut columns| {
            columns.sort_by_key(|column| column.ordinal);
            let first = columns
                .first()
                .expect("a group exists only because a column was pushed into it");

            let references_columns = columns
                .iter()
                .map(|column| {
                    column.references_column.clone().unwrap_or_else(|| {
                        panic!(
                            "{dialect} reports no target column for the foreign key on \
                             {}.{} -> {}. Every REFERENCES in both migration \
                             directories names its target column, and this check \
                             compares those names; a key relying on the implicit \
                             primary-key target needs the check taught to resolve it.",
                            first.table_name, column.column_name, first.references_table
                        )
                    })
                })
                .collect::<Vec<_>>()
                .join(",");

            ForeignKey {
                table: first.table_name.clone(),
                columns: columns
                    .iter()
                    .map(|column| column.column_name.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                references_table: first.references_table.clone(),
                references_columns,
                on_delete: first.on_delete.clone(),
            }
        })
        .collect()
}

/// Read one dialect's primary-key columns that permit NULL.
async fn nullable_primary_key_columns(
    db: &DbPool,
    query: &str,
    dialect: &str,
) -> BTreeSet<(String, String)> {
    let rows: Vec<PrimaryKeyColumnRow> = db_fetch_all!(db, PrimaryKeyColumnRow, query)
        .unwrap_or_else(|e| panic!("read {dialect}'s primary-key columns: {e}"));

    assert!(
        !rows.is_empty(),
        "{dialect} reported no primary-key columns at all — this check cannot \
         compare a catalogue it failed to read"
    );

    rows.into_iter()
        .filter(|row| !row.not_null)
        .map(|row| (row.table_name, row.column_name))
        .collect()
}

/// Render a set of differences as one indented line each, for an assert message.
fn listed<T: std::fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    items.into_iter().map(|item| format!("\n    {item}")).collect()
}

/// The two dialects declare the same foreign keys, with the same ON DELETE.
///
/// This is the check that found TRA-9999, TRA-9969 and TRA-9990, and the one
/// that has to keep reporting no drift for them to stay closed. It runs against
/// two freshly migrated databases — `DbPool::connect` over
/// `apps/server/migrations` and over `apps/server/migrations-sqlite`, the same
/// calls the server makes at startup — so it compares the schemas that ship
/// rather than what the migration files appear to say. That distinction is the
/// point: earlier migrations rebuild tables, so a constraint declared in one
/// file need not survive into the final schema, and every attempt to measure
/// this drift by reading SQL got it wrong.
///
/// Written out rather than declared with `dual_backend_test!` because it needs
/// both backends at once; `#[ignore]`d on the same terms as every Postgres half
/// here, since without a Postgres there is nothing to compare against.
#[tokio::test]
#[ignore = "requires a live Postgres — see crates/trakkt-core/src/test_helpers/dual_backend.rs"]
async fn the_two_dialects_declare_the_same_foreign_keys() {
    on_postgres(|pg| async move {
        let sqlite = test_pool()
            .await
            .expect("open an in-memory SQLite pool and apply migrations-sqlite");

        let pg_keys = foreign_keys(&pg, PG_FOREIGN_KEY_COLUMNS, "Postgres").await;
        let sqlite_keys = foreign_keys(&sqlite, SQLITE_FOREIGN_KEY_COLUMNS, "SQLite").await;

        // Each documented exception has to be real on both sides. Without these
        // two assertions the list would only ever grow: an entry naming a key
        // Postgres no longer has, or one SQLite has since gained, would sit here
        // exempting nothing and nobody would find out.
        for (table, column, reason) in FOREIGN_KEYS_POSTGRES_ONLY {
            assert!(
                pg_keys.iter().any(|key| key.table == *table && key.columns == *column),
                "FOREIGN_KEYS_POSTGRES_ONLY exempts {table}.{column}, but Postgres \
                 declares no foreign key on that column. The exemption is stale — \
                 delete it. Recorded reason: {reason}"
            );
            assert!(
                !sqlite_keys.iter().any(|key| key.table == *table && key.columns == *column),
                "FOREIGN_KEYS_POSTGRES_ONLY exempts {table}.{column}, but SQLite now \
                 declares a foreign key on it. The gap is closed — delete the \
                 exemption so this check compares the key like every other one. \
                 Recorded reason: {reason}"
            );
        }

        let expected: BTreeSet<ForeignKey> = pg_keys
            .iter()
            .filter(|key| {
                !FOREIGN_KEYS_POSTGRES_ONLY
                    .iter()
                    .any(|(table, column, _)| key.table == *table && key.columns == *column)
            })
            .cloned()
            .collect();

        let missing: Vec<&ForeignKey> = expected.difference(&sqlite_keys).collect();
        let extra: Vec<&ForeignKey> = sqlite_keys.difference(&expected).collect();

        assert!(
            missing.is_empty() && extra.is_empty(),
            "the two dialects declare different foreign keys.\n\
             \n\
             Declared on Postgres, not matched on SQLite:{}\n\
             \n\
             Declared on SQLite, not matched on Postgres:{}\n\
             \n\
             A key listed on both sides differs only in ON DELETE, which is the \
             shape that ships green and breaks on one backend: the parent delete \
             succeeds in production and is rejected on SQLite. Production runs \
             Postgres, so Postgres is the reference and the SQLite migration is \
             what moves. A gap that is genuinely intended goes in \
             FOREIGN_KEYS_POSTGRES_ONLY with its reasoning.",
            listed(&missing),
            listed(&extra)
        );
    })
    .await;
}

/// The two dialects agree on which primary-key columns permit NULL.
///
/// SQLite gives a non-`INTEGER PRIMARY KEY` no implicit NOT NULL — a legacy
/// quirk that made a row with no primary key insertable into `feedback`,
/// `issue_activities` and `notification_preferences`, and more than one of
/// them, since NULLs do not compare equal. Postgres has never permitted it.
/// TRA-10000.
#[tokio::test]
#[ignore = "requires a live Postgres — see crates/trakkt-core/src/test_helpers/dual_backend.rs"]
async fn the_two_dialects_agree_on_primary_key_nullability() {
    on_postgres(|pg| async move {
        let sqlite = test_pool()
            .await
            .expect("open an in-memory SQLite pool and apply migrations-sqlite");

        let pg_nullable =
            nullable_primary_key_columns(&pg, PG_PRIMARY_KEY_COLUMNS, "Postgres").await;
        let sqlite_nullable =
            nullable_primary_key_columns(&sqlite, SQLITE_PRIMARY_KEY_COLUMNS, "SQLite").await;

        assert!(
            pg_nullable.is_empty(),
            "Postgres declares primary-key columns that permit NULL, which it \
             should not be able to: {}",
            listed(pg_nullable.iter().map(|(table, column)| format!("{table}.{column}")))
        );

        for (table, column, reason) in NULLABLE_PRIMARY_KEYS_SQLITE_ONLY {
            assert!(
                sqlite_nullable.contains(&((*table).to_string(), (*column).to_string())),
                "NULLABLE_PRIMARY_KEYS_SQLITE_ONLY exempts {table}.{column}, but \
                 SQLite now forbids NULL there. Either the exemption is stale and \
                 should be deleted, or a migration has just made a rowid alias \
                 NOT NULL and broken every insert that binds NULL to get the next \
                 value. Recorded reason: {reason}"
            );
        }

        let undocumented: Vec<String> = sqlite_nullable
            .iter()
            .filter(|(table, column)| {
                !NULLABLE_PRIMARY_KEYS_SQLITE_ONLY
                    .iter()
                    .any(|(exempt_table, exempt_column, _)| {
                        table == exempt_table && column == exempt_column
                    })
            })
            .map(|(table, column)| format!("{table}.{column}"))
            .collect();

        assert!(
            undocumented.is_empty(),
            "SQLite permits NULL in primary-key columns Postgres does not:{}\n\
             \n\
             A TEXT PRIMARY KEY gets no implicit NOT NULL in SQLite, so rows with \
             no primary key — several of them — are insertable. Add NOT NULL in a \
             table rebuild. The only legitimate entries are rowid aliases, which \
             belong in NULLABLE_PRIMARY_KEYS_SQLITE_ONLY.",
            listed(&undocumented)
        );
    })
    .await;
}

/// The three rowid aliases still take a NULL bind and assign a value.
///
/// The counterweight to [`the_two_dialects_agree_on_primary_key_nullability`]:
/// that check names three columns SQLite may leave nullable, and this one is why
/// it must. `INTEGER PRIMARY KEY AUTOINCREMENT` is an alias for the rowid, and
/// binding NULL is how a caller asks for the next value — `write_sync_entry_in_tx`
/// and the auth-method and membership inserts all rely on it. A future rebuild
/// that swept these up with the TEXT primary keys would break every one.
///
/// SQLite-only, and not a `dual_backend_test!` pair, because the claim is about
/// SQLite's rowid rule. The Postgres columns are `bigint NOT NULL DEFAULT
/// nextval(...)`, where a DEFAULT applies to an *omitted* column and an explicit
/// NULL is rejected — so the same body would assert the opposite there.
#[tokio::test]
async fn rowid_alias_primary_keys_still_accept_a_null_bind() {
    let db = test_pool()
        .await
        .expect("open an in-memory SQLite pool and apply migrations-sqlite");

    seed_user(&db, USER, "rowid@example.test")
        .await
        .expect("seed the user the membership and auth-method rows hang off");
    seed_workspace(&db, WORKSPACE, USER)
        .await
        .expect("seed the workspace the membership and sync entries hang off");

    // seed_workspace already enrolled USER, so this is a second membership for a
    // second user, inserted here rather than through the helper because the
    // point is the explicit NULL bind on `id`.
    seed_user(&db, "usr_rowid_second", "rowid2@example.test")
        .await
        .expect("seed the second user its membership row names");

    db_execute!(
        &db,
        "INSERT INTO workspace_users (id, workspace_id, user_id, role, active, created_at) \
         VALUES (NULL, $1, $2, 'member', 1, datetime('now'))",
        WORKSPACE,
        "usr_rowid_second"
    )
    .expect("insert a workspace_users row binding NULL to its rowid-alias id");

    db_execute!(
        &db,
        "INSERT INTO user_auth_methods (id, user_id, auth_type, auth_data, created_at) \
         VALUES (NULL, $1, 'password', 'argon2-hash-placeholder', datetime('now'))",
        USER
    )
    .expect("insert a user_auth_methods row binding NULL to its rowid-alias id");

    db_execute!(
        &db,
        "INSERT INTO sync_log (sync_id, workspace_id, entity_type, entity_id, action, created_at) \
         VALUES (NULL, $1, 'issue', 'iss_rowid', 'insert', datetime('now'))",
        WORKSPACE
    )
    .expect("insert a sync_log row binding NULL to its rowid-alias sync_id");

    for (table, column) in [
        ("workspace_users", "id"),
        ("user_auth_methods", "id"),
        ("sync_log", "sync_id"),
    ] {
        let unassigned: i64 = db_fetch_scalar!(
            &db,
            i64,
            &format!("SELECT COUNT(*) FROM {table} WHERE {column} IS NULL")
        )
        .unwrap_or_else(|e| panic!("count the {table} rows left with a NULL {column}: {e}"));

        assert_eq!(
            unassigned, 0,
            "{table}.{column} is an INTEGER PRIMARY KEY AUTOINCREMENT, so a NULL \
             bind must be replaced with the next value rather than stored. A row \
             still holding NULL means the rowid alias was lost — most likely to a \
             table rebuild that added NOT NULL, or one that changed the declared \
             type away from INTEGER."
        );
    }
}

// ─── ON DELETE behaviour the two dialects now share ──────────────────────────
//
// The four keys of TRA-9999's category B — the ones that existed on both
// backends with a different ON DELETE. Each body deletes a parent and asserts
// what happens to the child, so a rebuild that restores the constraint but gets
// the action wrong fails here rather than passing the structural check above.
//
// Every one of these bodies was run against the pre-migration SQLite schema and
// failed there, with the parent delete rejected outright by a NO ACTION key.

/// Free the workspace of the rows whose foreign keys are NO ACTION on purpose.
///
/// Deleting a workspace is blocked by `workspace_users.workspace_id`, and
/// `seed_workspace` writes exactly one such row — the owner's membership, which
/// is not optional dressing there because it is what decides who receives a
/// broadcast. Clearing it is setup for the assertion, not part of it: the
/// question each caller asks is what happens to its *child* rows, and this
/// removes the unrelated key that would otherwise reject the delete first and
/// make the test pass or fail for the wrong reason.
async fn release_workspace_memberships(db: &DbPool) {
    db_execute!(db, "DELETE FROM workspace_users WHERE workspace_id = $1", WORKSPACE)
        .expect("clear the owner membership that would otherwise reject the workspace delete");
}

dual_backend_test! {
    /// Deleting a team deletes the views scoped to it, and leaves the rest.
    ///
    /// `views.team_id` cascaded on Postgres and was NO ACTION on SQLite
    /// (TRA-9969), so a team with any saved view could be deleted in production
    /// and not on SQLite. A view with no team is a workspace-level view and has
    /// to survive, which is the second assertion — a cascade written against
    /// the wrong column would take it too.
    async fn deleting_a_team_deletes_the_views_scoped_to_it(db) {
        seed_user(db, USER, "views@example.test")
            .await
            .expect("seed the workspace owner the views are created by");
        seed_workspace(db, WORKSPACE, USER)
            .await
            .expect("seed the workspace the views belong to");
        seed_team(db, TEAM, WORKSPACE, TEAM_KEY)
            .await
            .expect("seed the team one of the views is scoped to");

        db_execute!(
            db,
            "INSERT INTO views (view_id, workspace_id, created_by, name, team_id) \
             VALUES ($1, $2, $3, 'Team view', $4)",
            "view_team", WORKSPACE, USER, TEAM
        )
        .expect("insert the team-scoped view the team delete must take with it");

        db_execute!(
            db,
            "INSERT INTO views (view_id, workspace_id, created_by, name, team_id) \
             VALUES ($1, $2, $3, 'Workspace view', NULL)",
            "view_workspace", WORKSPACE, USER
        )
        .expect("insert the workspace-level view the team delete must leave alone");

        db_execute!(db, "DELETE FROM teams WHERE team_id = $1", TEAM)
            .expect("delete the team — rejected outright before views.team_id cascaded");

        let remaining: Vec<String> = view_ids(db).await;
        assert_eq!(
            remaining, vec!["view_workspace".to_string()],
            "deleting a team must delete exactly the views scoped to it. The \
             team-scoped view surviving means team_id is not cascading; the \
             workspace-level view disappearing means the cascade is on the wrong \
             column."
        );
    }
}

dual_backend_test! {
    /// Deleting a user deletes their notification preferences.
    ///
    /// `notification_preferences.user_id` cascaded on Postgres and was NO ACTION
    /// on SQLite, so a user who had ever opened the notification settings page
    /// could be deleted in production and not on SQLite.
    ///
    /// The user deleted here is a second one that owns nothing. `users` is the
    /// target of NO ACTION keys from `workspaces.owner_user_id` and
    /// `workspace_users.user_id` among others — both of which the seeded owner
    /// holds — and either would reject the delete before the cascade under test
    /// was reached.
    async fn deleting_a_user_deletes_their_notification_preferences(db) {
        const OTHER_USER: &str = "usr_prefs_other";

        seed_user(db, USER, "prefs-owner@example.test")
            .await
            .expect("seed the workspace owner whose preferences must survive");
        seed_workspace(db, WORKSPACE, USER)
            .await
            .expect("seed the workspace both preference rows are scoped to");
        seed_user(db, OTHER_USER, "prefs-other@example.test")
            .await
            .expect("seed the user with no workspace of their own, who is deleted below");

        seed_preferences(db, "pref_owner", USER).await;
        seed_preferences(db, "pref_other", OTHER_USER).await;

        db_execute!(db, "DELETE FROM users WHERE user_id = $1", OTHER_USER)
            .expect("delete the user — rejected before notification_preferences cascaded");

        let remaining: Vec<String> = preference_ids(db).await;
        assert_eq!(
            remaining, vec!["pref_owner".to_string()],
            "deleting a user must delete exactly their own preference row. The \
             deleted user's row surviving means user_id is not cascading; the \
             other user's row disappearing means the cascade is unscoped."
        );
    }
}

dual_backend_test! {
    /// Deleting a workspace deletes the notification preferences within it.
    ///
    /// `notification_preferences.workspace_id` cascaded on Postgres and was NO
    /// ACTION on SQLite.
    async fn deleting_a_workspace_deletes_its_notification_preferences(db) {
        seed_user(db, USER, "prefs-ws@example.test")
            .await
            .expect("seed the workspace owner the preference row belongs to");
        seed_workspace(db, WORKSPACE, USER)
            .await
            .expect("seed the workspace being deleted");

        seed_preferences(db, "pref_ws", USER).await;
        release_workspace_memberships(db).await;

        db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", WORKSPACE)
            .expect("delete the workspace — rejected before notification_preferences cascaded");

        assert!(
            preference_ids(db).await.is_empty(),
            "deleting a workspace must delete the notification preferences scoped \
             to it; a surviving row means workspace_id is not cascading"
        );
    }
}

dual_backend_test! {
    /// Deleting a workspace deletes the attachments within it.
    ///
    /// `attachments.workspace_id` cascaded on Postgres and was NO ACTION on
    /// SQLite, so a workspace holding any attachment could be deleted in
    /// production and not on SQLite.
    async fn deleting_a_workspace_deletes_its_attachments(db) {
        seed_user(db, USER, "attach@example.test")
            .await
            .expect("seed the user recorded as having uploaded the attachment");
        seed_workspace(db, WORKSPACE, USER)
            .await
            .expect("seed the workspace being deleted");

        db_execute!(
            db,
            "INSERT INTO attachments \
                 (attachment_id, workspace_id, filename, content_type, size_bytes, \
                  storage_path, uploaded_by) \
             VALUES ($1, $2, 'note.txt', 'text/plain', $3, 'ws/note.txt', $4)",
            "att_ws", WORKSPACE, 12_i64, USER
        )
        .expect("insert the attachment the workspace delete must take with it");

        release_workspace_memberships(db).await;

        db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", WORKSPACE)
            .expect("delete the workspace — rejected before attachments cascaded");

        let surviving: i64 = db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM attachments")
            .expect("count the attachments left after the workspace delete");
        assert_eq!(
            surviving, 0,
            "deleting a workspace must delete the attachments scoped to it; a \
             surviving row means workspace_id is not cascading"
        );
    }
}

dual_backend_test! {
    /// Linking an existing attachment to an issue records the junction row as
    /// its payload, on both dialects.
    ///
    /// `attach_to_issue` re-reads the row it just inserted, on the transaction,
    /// through a `CAST(created_at AS TEXT)` SELECT — and `issue_attachments.
    /// created_at` is `TIMESTAMPTZ` on Postgres and TEXT on SQLite while the row
    /// type declares `String` for both. Without the cast the Postgres decode
    /// fails and the whole attach fails with it, which is a defect confined to
    /// the arm every other test in the workspace never executes. The SQLite half
    /// of this pair would pass either way; that asymmetry is the reason it is
    /// here rather than only in `sync_log_service`'s own tests.
    ///
    /// Asserted on the decoded value, not the stored bytes: Postgres parses JSONB
    /// and re-serialises it, so `{"a":1}` reads back as `{"a": 1}`.
    async fn a_link_to_an_existing_attachment_carries_the_junction_row(db) {
        seed_tenancy(db).await;
        let issue = create_issue(db, "Holds an attachment").await;

        db_execute!(
            db,
            "INSERT INTO attachments \
                 (attachment_id, workspace_id, filename, content_type, size_bytes, \
                  storage_path, uploaded_by) \
             VALUES ($1, $2, 'diagram.png', 'image/png', $3, 'ws/diagram.png', $4)",
            "att_link", WORKSPACE, 4096_i64, USER
        )
        .expect("insert the attachment the link points at");

        // Taken after the issue exists, so the entries below are the attach's
        // and nothing else — which also states its entry count.
        let watermark = sync_watermark(db).await;

        trakkt_auth::attachment_service::attach_to_issue(
            db, WORKSPACE, &issue.issue_id, "att_link", None,
        )
        .await
        .expect("link the existing attachment to the issue");

        let entries = entries_above(db, watermark).await;
        assert_eq!(
            triples(&entries),
            vec![(
                entity_types::ISSUE_ATTACHMENT.to_owned(),
                format!("{}:att_link", issue.issue_id),
                "insert".to_owned(),
            )],
            "an attach writes exactly one sync entry, naming the link it made"
        );

        let data = entries[0].data.as_deref().unwrap_or_else(|| {
            panic!(
                "sync entry {} ({} {}) has no payload — `cache/apply.rs` drops a \
                 data-less insert before its entity match, so the link would reach no \
                 client at all",
                entries[0].sync_id, entries[0].entity_type, entries[0].entity_id
            )
        });
        let link: trakkt_types::models::IssueAttachment = serde_json::from_str(data)
            .unwrap_or_else(|e| panic!("the stored payload is not an IssueAttachment: {e} — {data}"));

        assert_eq!(link.issue_id, issue.issue_id);
        assert_eq!(link.attachment_id, "att_link");
        assert!(
            !link.created_at.is_empty(),
            "the payload is built from the re-read, so the DB-assigned created_at \
             has to survive the cast on both dialects; got {link:?}"
        );
    }
}

/// Insert a notification-preferences row for `user_id` in [`WORKSPACE`].
async fn seed_preferences(db: &DbPool, preference_id: &str, user_id: &str) {
    db_execute!(
        db,
        "INSERT INTO notification_preferences (preference_id, user_id, workspace_id) \
         VALUES ($1, $2, $3)",
        preference_id, user_id, WORKSPACE
    )
    .unwrap_or_else(|e| panic!("insert the notification preferences {preference_id}: {e}"));
}

/// Every surviving `views.view_id`, ordered so assertions can compare directly.
async fn view_ids(db: &DbPool) -> Vec<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        view_id: String,
    }
    db_fetch_all!(db, Row, "SELECT view_id FROM views ORDER BY view_id")
        .expect("read the views left after the delete")
        .into_iter()
        .map(|row| row.view_id)
        .collect()
}

/// Every surviving `notification_preferences.preference_id`, ordered.
async fn preference_ids(db: &DbPool) -> Vec<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        preference_id: String,
    }
    db_fetch_all!(
        db,
        Row,
        "SELECT preference_id FROM notification_preferences ORDER BY preference_id"
    )
    .expect("read the notification preferences left after the delete")
    .into_iter()
    .map(|row| row.preference_id)
    .collect()
}

// ─── Deleting an issue takes its dependent rows with it ──────────────────────
//
// TRA-9990's two keys, `issue_activities.issue_id` and `github_links.issue_id`.
// Both were category A — absent from SQLite entirely — so the failure mode
// before this migration is an orphan rather than a rejection: the issue
// disappears, and its activity feed and GitHub links stay behind naming an id
// no row has. Nothing errors, on either backend. That is why the assertions
// below count surviving rows instead of expecting a rejected delete.
//
// The structural check above already guards that these two keys exist and
// cascade. This body is the runtime counterpart: it fails if the constraint is
// declared and the rows nevertheless survive.

/// How many rows of `table` still name `issue_id`.
///
/// `table` is interpolated rather than bound because a table name cannot be a
/// bind parameter; every caller passes a literal from this file.
async fn rows_naming_issue(db: &DbPool, table: &str, issue_id: &str) -> i64 {
    db_fetch_scalar!(
        db,
        i64,
        &format!("SELECT COUNT(*) FROM {table} WHERE issue_id = $1"),
        issue_id
    )
    .unwrap_or_else(|e| panic!("count the {table} rows still naming issue {issue_id}: {e}"))
}

dual_backend_test! {
    /// Deleting an issue deletes its activity rows and its GitHub links.
    ///
    /// Both cascades are asserted in one body because they are one ticket and
    /// one delete: splitting them would seed the same issue twice to assert two
    /// halves of the same statement. The counts are checked before the delete as
    /// well as after — without that, a seed that silently failed would leave
    /// both afterwards-counts at zero and the test would pass having proved
    /// nothing.
    async fn deleting_an_issue_deletes_its_activities_and_github_links(db) {
        seed_tenancy(db).await;
        let issue = create_issue(db, "An issue carrying an activity and a GitHub link").await;
        // Bound as `&str` rather than moved: `db_execute!` binds by value, and
        // the same id is needed by four statements and four counts below.
        let issue_id = issue.issue_id.as_str();

        db_execute!(
            db,
            "INSERT INTO issue_activities \
                 (activity_id, issue_id, workspace_id, actor_id, action_type, field) \
             VALUES ($1, $2, $3, $4, 'updated', 'title')",
            "act_cascade", issue_id, WORKSPACE, USER
        )
        .expect("insert the activity row the issue delete must take with it");

        // github_links.installation_id is NOT NULL and now references
        // github_installations, which in turn references github_apps — so the
        // link cannot be seeded without the chain above it.
        db_execute!(
            db,
            "INSERT INTO github_apps \
                 (github_app_id, app_id, app_name, client_id, client_secret_encrypted, \
                  private_key_encrypted, webhook_secret_encrypted) \
             VALUES ($1, $2, 'Trakkt Dialect Suite', 'client-id', 'enc', 'enc', 'enc')",
            "gha_cascade", 4242_i64
        )
        .expect("seed the GitHub App the installation hangs off");

        db_execute!(
            db,
            "INSERT INTO github_installations \
                 (installation_id, workspace_id, github_app_id, github_installation_id, \
                  account_login, account_type) \
             VALUES ($1, $2, $3, $4, 'octocat', 'User')",
            "ghi_cascade", WORKSPACE, "gha_cascade", 99_i64
        )
        .expect("seed the installation the link hangs off");

        db_execute!(
            db,
            "INSERT INTO github_links \
                 (link_id, workspace_id, issue_id, installation_id, link_type, \
                  repo_full_name, ref_identifier, url) \
             VALUES ($1, $2, $3, $4, 'pull_request', 'octocat/trakkt', '7', \
                     'https://github.test/octocat/trakkt/pull/7')",
            "ghl_cascade", WORKSPACE, issue_id, "ghi_cascade"
        )
        .expect("insert the GitHub link the issue delete must take with it");

        assert_eq!(
            rows_naming_issue(db, "issue_activities", issue_id).await, 1,
            "the activity row must exist before the delete, or the assertion after \
             it proves nothing"
        );
        assert_eq!(
            rows_naming_issue(db, "github_links", issue_id).await, 1,
            "the GitHub link must exist before the delete, or the assertion after \
             it proves nothing"
        );

        db_execute!(db, "DELETE FROM issues WHERE issue_id = $1", issue_id)
            .expect("delete the issue the activity row and the GitHub link belong to");

        assert_eq!(
            rows_naming_issue(db, "issue_activities", issue_id).await, 0,
            "deleting an issue must delete its activity rows. A surviving row is an \
             orphan naming an issue_id no row has — which is what SQLite did before \
             issue_activities.issue_id existed at all, silently and without error."
        );
        assert_eq!(
            rows_naming_issue(db, "github_links", issue_id).await, 0,
            "deleting an issue must delete its GitHub links. A surviving row is an \
             orphan naming an issue_id no row has — which is what SQLite did before \
             github_links.issue_id existed at all, silently and without error."
        );
    }
}

// ─── A row with no primary key ───────────────────────────────────────────────
//
// TRA-10000. Three TEXT primary keys carried no NOT NULL on SQLite, so a row
// with no primary key was insertable — and several, since NULLs do not compare
// equal, which the UNIQUE-looking primary key does nothing to prevent.
//
// Each body inserts a row that is valid in every other respect, so that NOT NULL
// is the only constraint that can reject it. The assertion checks the message
// says so: a row rejected by a foreign key instead would otherwise report the
// same pass, and these tables gained foreign keys in the same migration.

/// Assert `outcome` is a NOT NULL rejection naming `table`.`column`.
///
/// Takes the whole `Result` rather than the error, because `DbQueryResult` has
/// no `Debug` and `expect_err` needs one — and because the success arm deserves
/// the message it gets here rather than a derived one.
///
/// Postgres says `null value in column "id" of relation "feedback" violates
/// not-null constraint`; SQLite says `NOT NULL constraint failed: feedback.id`.
/// Both name the table and the column and both contain "null", which is all
/// this needs — and neither says "foreign key", which is the wrong-reason pass
/// worth ruling out explicitly.
fn assert_rejected_for_null_primary_key(
    outcome: Result<trakkt_core::db::DbQueryResult, sqlx::Error>,
    table: &str,
    column: &str,
) {
    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!(
            "inserting a {table} row with a NULL {column} was accepted. {column} is \
             a TEXT PRIMARY KEY, which SQLite gives no implicit NOT NULL, so the \
             table now holds a row with no primary key — and will take more, since \
             NULLs do not compare equal."
        ),
    };
    let message = error.to_string().to_lowercase();

    assert!(
        !message.contains("foreign key"),
        "the insert into {table} was rejected by a foreign key, not by NOT NULL on \
         {column}, so it says nothing about the primary key: {error}"
    );
    assert!(
        message.contains("null") && message.contains(column) && message.contains(table),
        "the insert into {table} must be rejected by NOT NULL on {column}, and the \
         rejection must name them; got: {error}"
    );
}

dual_backend_test! {
    /// A feedback row with no id is rejected.
    async fn feedback_rejects_a_null_primary_key(db) {
        seed_user(db, USER, "feedback-null@example.test")
            .await
            .expect("seed the user the feedback row names");
        seed_workspace(db, WORKSPACE, USER)
            .await
            .expect("seed the workspace the feedback row names");

        let outcome = db_execute!(
            db,
            "INSERT INTO feedback (id, user_id, workspace_id, feedback_type, description) \
             VALUES (NULL, $1, $2, 'bug', 'a report with no primary key')",
            USER, WORKSPACE
        );

        assert_rejected_for_null_primary_key(outcome, "feedback", "id");
    }
}

dual_backend_test! {
    /// An issue_activities row with no activity_id is rejected.
    async fn issue_activities_rejects_a_null_primary_key(db) {
        seed_tenancy(db).await;
        let issue = create_issue(db, "An issue for the activity row to reference").await;

        let outcome = db_execute!(
            db,
            "INSERT INTO issue_activities (activity_id, issue_id, workspace_id, action_type) \
             VALUES (NULL, $1, $2, 'created')",
            issue.issue_id, WORKSPACE
        );

        assert_rejected_for_null_primary_key(outcome, "issue_activities", "activity_id");
    }
}

dual_backend_test! {
    /// A notification_preferences row with no preference_id is rejected.
    async fn notification_preferences_rejects_a_null_primary_key(db) {
        seed_user(db, USER, "prefs-null@example.test")
            .await
            .expect("seed the user the preference row names");
        seed_workspace(db, WORKSPACE, USER)
            .await
            .expect("seed the workspace the preference row names");

        let outcome = db_execute!(
            db,
            "INSERT INTO notification_preferences (preference_id, user_id, workspace_id) \
             VALUES (NULL, $1, $2)",
            USER, WORKSPACE
        );

        assert_rejected_for_null_primary_key(outcome, "notification_preferences", "preference_id");
    }
}

// ─── Deleting a project reports the rows its cascade removes ─────────────────
//
// TRA-9971. `DELETE FROM projects` empties `project_members`,
// `project_milestones` and `project_updates` through `ON DELETE CASCADE` and
// clears `issues.project_id` through `ON DELETE SET NULL`, all without the code
// learning what it destroyed. `sync_delta` replays entity-scoped actions, so an
// entity that never receives one is never evicted from a client's IndexedDB
// cache: a browser that did not perform the delete kept the project's members,
// milestones and posted updates forever.
//
// Run on both backends because the cascade is not identical on them.
// `issues.milestone_id` has an `ON DELETE SET NULL` foreign key on Postgres and
// no foreign key at all on SQLite — the twelfth of the twelve keys
// `migrations-sqlite/20260803100000_dual_backend_fk_parity.sql` measured, and
// the one it deliberately left outstanding. The entry *count* is what must not
// vary by dialect, and only running both proves it does not.

/// A second workspace member, so the project has more than one membership row
/// to cascade and the two entity ids have to be distinguished.
const SECOND_USER: &str = "usr_dialect_second";

/// One `sync_log` row as the cascade assertions read it.
#[derive(sqlx::FromRow)]
struct CascadeEntry {
    sync_id: i64,
    entity_type: String,
    entity_id: String,
    action: String,
    data: Option<String>,
}

/// The highest `sync_id` in the log, or 0 when it is empty.
///
/// Every assertion below is made against the rows above this watermark. Seeding
/// goes through the real services, which write entries of their own — a
/// whole-table read would drown the delete's output in its fixture's.
async fn sync_watermark(db: &DbPool) -> i64 {
    db_fetch_scalar!(db, i64, "SELECT COALESCE(MAX(sync_id), 0) FROM sync_log")
        .expect("read the current high-water mark of the sync log")
}

/// Every `sync_log` row written above `watermark`, oldest first.
async fn entries_above(db: &DbPool, watermark: i64) -> Vec<CascadeEntry> {
    db_fetch_all!(
        db,
        CascadeEntry,
        "SELECT sync_id, entity_type, entity_id, action, CAST(data AS TEXT) AS data \
         FROM sync_log WHERE sync_id > $1 ORDER BY sync_id ASC",
        watermark
    )
    .expect("read the sync entries the project delete wrote")
}

/// `(entity_type, entity_id, action)` for each row, sorted.
///
/// Sorted because only the *set* is specified. The entries are written in loop
/// order within each cascaded table, and no query below orders its ids, so the
/// order two members arrive in is the database's business and not a thing to
/// assert. Sorting also makes a duplicated entry — the same issue reached
/// through both of the detach predicates — visible rather than absorbed.
fn triples(entries: &[CascadeEntry]) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = entries
        .iter()
        .map(|e| {
            (
                e.entity_type.clone(),
                e.entity_id.clone(),
                e.action.clone(),
            )
        })
        .collect();
    out.sort();
    out
}

/// Create an issue attached to `project_id` and/or `milestone_id`.
///
/// `create_issue` above hardcodes both to `None`; this cascade needs issues that
/// hold one, the other, or both, so the detach predicates can be told apart.
async fn create_attached_issue(
    db: &DbPool,
    title: &str,
    project_id: Option<&str>,
    milestone_id: Option<&str>,
) -> Issue {
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
            project_id: project_id.map(str::to_owned),
            milestone_id: milestone_id.map(str::to_owned),
            estimate: None,
        },
        None,
    )
    .await
    .expect("create the issue the project cascade detaches")
}

/// A project with `name`, plus one member, one milestone and one posted update.
///
/// Seeded through the real services so every row is exactly what production
/// writes — including the `sync_log` entries, which is what makes the cached
/// rows this test is about genuinely reachable.
///
/// Returns `(project_id, milestone_id, update_id)`.
async fn seed_project_with_children(
    db: &DbPool,
    name: &str,
    member: &str,
) -> (String, String, String) {
    let project = trakkt_auth::project_service::create_project(
        db,
        &trakkt_auth::project_service::CreateProjectParams {
            workspace_id: WORKSPACE,
            name,
            description: None,
            icon: None,
            color: None,
            lead_id: None,
            start_date: None,
            target_date: None,
        },
        None,
    )
    .await
    .expect("create the project the cascade assertions are made against");

    trakkt_auth::project_service::add_project_member(
        db,
        &project.project_id,
        member,
        "member",
        WORKSPACE,
        None,
    )
    .await
    .expect("add the member the cascade must report the removal of");

    let milestone = trakkt_auth::project_service::create_milestone(
        db,
        &project.project_id,
        &format!("{name} milestone"),
        None,
        None,
        None,
        WORKSPACE,
    )
    .await
    .expect("create the milestone the cascade must report the removal of");

    let update = trakkt_auth::project_service::create_project_update(
        db,
        &project.project_id,
        USER,
        "on_track",
        Some("posted before the delete"),
        None,
        WORKSPACE,
    )
    .await
    .expect("post the update the cascade must report the removal of");

    (project.project_id, milestone.milestone_id, update.update_id)
}

dual_backend_test! {
    /// Every row a project delete cascades away gets a sync entry naming it, and
    /// every issue it merely detaches gets an update carrying the new state.
    ///
    /// The control project is not decoration. Without it the assertion could not
    /// tell "the cascade reported exactly its own rows" from "the cascade
    /// reported every project row in the workspace", and the second is a
    /// considerably worse bug than the one being fixed.
    async fn deleting_a_project_reports_every_row_its_cascade_touches(db) {
        seed_tenancy(db).await;
        seed_user(db, SECOND_USER, "dialect-second@example.test")
            .await
            .expect("seed the project's second member");

        let (doomed, doomed_milestone, doomed_update) =
            seed_project_with_children(db, "Doomed", USER).await;
        // A second membership, so an implementation reporting only the first row
        // of each table is caught.
        trakkt_auth::project_service::add_project_member(
            db, &doomed, SECOND_USER, "member", WORKSPACE, None,
        )
        .await
        .expect("add the project's second member");

        let (survivor, survivor_milestone, survivor_update) =
            seed_project_with_children(db, "Survivor", SECOND_USER).await;

        // The three shapes of detachment, plus one issue that is neither.
        let own = create_attached_issue(db, "In the project", Some(&doomed), None).await;
        let both =
            create_attached_issue(db, "In the project, on its milestone", Some(&doomed), Some(&doomed_milestone))
                .await;
        // Not in the project, but holding one of its milestones. Nothing in
        // `create_issue` or `update_issue` requires a milestone to belong to the
        // issue's own project, so this is reachable — and it is the case a
        // `project_id = $1` read alone would miss.
        let milestone_only =
            create_attached_issue(db, "On its milestone only", None, Some(&doomed_milestone)).await;
        let untouched =
            create_attached_issue(db, "In the other project", Some(&survivor), None).await;

        let watermark = sync_watermark(db).await;

        trakkt_auth::project_service::delete_project(db, &doomed, None)
            .await
            .expect("delete the project in the browser that owns the tab");

        let entries = entries_above(db, watermark).await;

        let mut expected: Vec<(String, String, String)> = vec![
            (entity_types::PROJECT.into(), doomed.clone(), "delete".into()),
            (entity_types::PROJECT_MEMBER.into(), format!("{doomed}:{USER}"), "delete".into()),
            (entity_types::PROJECT_MEMBER.into(), format!("{doomed}:{SECOND_USER}"), "delete".into()),
            (entity_types::PROJECT_MILESTONE.into(), doomed_milestone.clone(), "delete".into()),
            (entity_types::PROJECT_UPDATE.into(), doomed_update.clone(), "delete".into()),
            (entity_types::ISSUE.into(), own.issue_id.clone(), "update".into()),
            (entity_types::ISSUE.into(), both.issue_id.clone(), "update".into()),
            (entity_types::ISSUE.into(), milestone_only.issue_id.clone(), "update".into()),
        ];
        expected.sort();

        assert_eq!(
            triples(&entries),
            expected,
            "the delete must report itself and every row it cascaded away, once each. \
             A missing member, milestone or update entry is a row that stays in every \
             other client's IndexedDB through every reconnect — that is TRA-9971. An \
             extra entry naming the surviving project's rows ({survivor}, \
             {survivor_milestone}, {survivor_update}) would evict rows that still exist."
        );

        // One transaction, stated the only way the log can state it: the ids form
        // a contiguous run. A gap would mean some other transaction's entry
        // interleaved, which for a single-threaded test means the cascade was
        // committed in more than one piece.
        let ids: Vec<i64> = entries.iter().map(|e| e.sync_id).collect();
        let first = *ids.first().expect("the delete must write at least one entry");
        assert_eq!(
            ids,
            (first..first + ids.len() as i64).collect::<Vec<i64>>(),
            "the cascade's entries must be a contiguous block of sync_ids — they are \
             written on one transaction, so nothing can be allocated between them"
        );

        // The detached issues carry their new state. A delete entry, or an update
        // with no payload, would be worse than nothing: `cache/apply.rs` drops a
        // data-less insert/update before it reaches IndexedDB, and a delete would
        // evict an issue the server still has.
        for entry in entries.iter().filter(|e| e.entity_type == entity_types::ISSUE) {
            let raw = entry
                .data
                .as_deref()
                .unwrap_or_else(|| panic!(
                    "the ISSUE update for {} must carry a payload — a data-less \
                     update is discarded by the client before it reaches the cache",
                    entry.entity_id
                ));
            let payload: serde_json::Value = serde_json::from_str(raw)
                .unwrap_or_else(|e| panic!("parsing the ISSUE payload {raw:?}: {e}"));
            assert_eq!(
                payload.get("project_id"),
                Some(&serde_json::Value::Null),
                "the payload for issue {} must show the project cleared; a payload \
                 read before the DELETE would still name {doomed}, and the client \
                 would put the issue straight back under a project it was just told \
                 to remove",
                entry.entity_id
            );
        }

        // Not vacuous: the untouched issue keeps its project, so "reported the
        // right issues" and "reported every issue" are distinguishable.
        let still_attached: Option<String> = db_fetch_scalar!(
            db,
            Option<String>,
            "SELECT project_id FROM issues WHERE issue_id = $1",
            &untouched.issue_id
        )
        .expect("read the untouched issue's project back");
        assert_eq!(
            still_attached.as_deref(),
            Some(survivor.as_str()),
            "the other project's issue must keep its project, or the assertions \
             above would pass against a delete that detached everything"
        );
    }
}

// ─── A rejected cascade entry rolls the whole cascade back ───────────────────

/// How many rows of `table` still belong to `project_id`.
///
/// `table` is interpolated rather than bound because a table name cannot be a
/// bind parameter; both callers pass a literal from this file.
async fn rows_naming_project(db: &DbPool, table: &str, project_id: &str) -> i64 {
    db_fetch_scalar!(
        db,
        i64,
        &format!("SELECT COUNT(*) FROM {table} WHERE project_id = $1"),
        project_id
    )
    .unwrap_or_else(|e| panic!("count the {table} rows still naming project {project_id}: {e}"))
}

/// `(members, milestones, updates)` still belonging to `project_id`.
async fn child_counts(db: &DbPool, project_id: &str) -> (i64, i64, i64) {
    (
        rows_naming_project(db, "project_members", project_id).await,
        rows_naming_project(db, "project_milestones", project_id).await,
        rows_naming_project(db, "project_updates", project_id).await,
    )
}

dual_backend_test! {
    /// A rejected sync entry rolls the whole cascade back — probed once per
    /// entity type the cascade writes.
    ///
    /// The trigger is narrowed to one `entity_type` per pass, which is the whole
    /// point. `reject_sync_log_inserts` fails every insert, so it stops the
    /// cascade at its first entry and each pass would prove the same single
    /// thing: that the PROJECT entry is written inside the transaction. It would
    /// be green against an implementation that wrote nothing else at all —
    /// exactly the bug. Narrowing lets the entries before the probed type
    /// through, so each pass is a statement about the loop that writes *its*
    /// type and no other.
    ///
    /// One body rather than five, and one database rather than five, because
    /// every pass rolls back: the project and its children are exactly as they
    /// were when the next pass starts, which is what the assertions re-establish
    /// each time round.
    async fn a_rejected_entry_of_any_type_rolls_the_whole_project_cascade_back(db) {
        seed_tenancy(db).await;
        let (project, milestone, _update) =
            seed_project_with_children(db, "Rolled back", USER).await;
        let issue = create_attached_issue(db, "Detached and then not", Some(&project), None).await;

        let counts_before = child_counts(db, &project).await;
        assert_eq!(
            counts_before, (1, 1, 1),
            "the fixture must have one row in each cascaded table, or a rollback \
             assertion cannot tell a preserved row from a table that was empty \
             all along"
        );

        for probed in [
            entity_types::PROJECT,
            entity_types::PROJECT_MEMBER,
            entity_types::PROJECT_MILESTONE,
            entity_types::PROJECT_UPDATE,
            entity_types::ISSUE,
        ] {
            let watermark = sync_watermark(db).await;
            reject_sync_log_inserts_of_type(db, probed).await;

            // Destructured rather than `expect_err`, so the success case can name
            // the entity type this pass is probing. `expect_err` takes a fixed
            // string, and "the delete succeeded" is the same sentence for all
            // five passes — which is precisely the confusion the narrowed trigger
            // exists to remove.
            let Err(err) = trakkt_auth::project_service::delete_project(db, &project, None).await
            else {
                panic!(
                    "the trigger installed for this pass rejects every {probed} sync \
                     entry, and the delete succeeded anyway — so no {probed} entry was \
                     written at all. Every rollback assertion below would be vacuous \
                     for {probed}, and a client would never be told to evict it."
                );
            };
            assert!(
                err.to_string().contains("sync_log insert rejected"),
                "the {probed} pass must fail on the rejected sync entry rather than \
                 on something else; got: {err}"
            );

            clear_sync_log_rejection(db).await;

            let survived: i64 = db_fetch_scalar!(
                db,
                i64,
                "SELECT COUNT(*) FROM projects WHERE project_id = $1",
                &project
            )
            .expect("count the project after the rejected delete");
            assert_eq!(
                survived, 1,
                "rejecting the {probed} entry must roll the DELETE back. A project \
                 removed with no entry to replay its removal stays on every other \
                 client forever, and no later delta can repair it — the row it would \
                 have to re-read is gone."
            );

            assert_eq!(
                child_counts(db, &project).await,
                counts_before,
                "rejecting the {probed} entry must roll the cascade back too — the \
                 member, milestone and update rows are destroyed by foreign keys the \
                 same statement fires, so they unwind with it or not at all"
            );

            let attached: Option<String> = db_fetch_scalar!(
                db,
                Option<String>,
                "SELECT project_id FROM issues WHERE issue_id = $1",
                &issue.issue_id
            )
            .expect("read the issue's project after the rejected delete");
            assert_eq!(
                attached.as_deref(),
                Some(project.as_str()),
                "rejecting the {probed} entry must restore the issue's project too; \
                 an issue detached by a rolled-back delete would show as unassigned \
                 with nothing on the wire to explain it"
            );

            let orphans = entries_above(db, watermark).await;
            assert!(
                orphans.is_empty(),
                "rejecting the {probed} entry must leave no partial entry behind — \
                 the entries written before it are in the same transaction and unwind \
                 with it; found {:?}",
                triples(&orphans)
            );
        }

        // The passes above all failed. The same call with no trigger installed
        // must succeed, or every assertion here would hold against a
        // `delete_project` that could never delete anything.
        trakkt_auth::project_service::delete_project(db, &project, None)
            .await
            .expect("with no trigger installed the same delete must succeed");
        assert_eq!(
            child_counts(db, &project).await,
            (0, 0, 0),
            "the successful delete must actually empty the cascaded tables"
        );
        let milestone_gone: i64 = db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM project_milestones WHERE milestone_id = $1",
            &milestone
        )
        .expect("count the milestone after the successful delete");
        assert_eq!(milestone_gone, 0, "the milestone must be gone with its project");
    }
}

// ─── Notification state changes reach exactly their recipient ────────────────
//
// TRA-9974. The notification state-change entry points open a transaction,
// select the rows their predicate matches, UPDATE them and record one `sync_log`
// entry per row, all before the commit. Everything dialect-shaped in that path is
// reached only from here: the `read` / `deleted_at` predicates are built from
// `sql_compat::bool_true`, `bool_false` and `now()`; the id list goes through
// `in_clause_placeholders`, so the bind indices of the `IN` clause are computed
// rather than written out; the read-back runs `NOTIFICATION_SELECT` on the open
// transaction, whose `CAST(n.deleted_at AS TEXT)` exists because Postgres will
// not decode a TIMESTAMP as TEXT; and the entry lands in `sync_log.data`, which
// is JSONB on Postgres and TEXT on SQLite.
//
// The rest of the workspace's coverage for these functions is in
// `sync_log_service`'s SQLite-only test module, so none of the above had ever
// been executed against Postgres. This is a regression guard, not a gap being
// closed — the assertions passed on both backends when first written.

/// The notification the state changes are made against, and the title every
/// payload has to carry back.
const NOTIFIED_ISSUE_TITLE: &str = "The issue the inbox names";

/// The four types the seeded notifications use, one each, so the four entries a
/// phase writes are four distinct rows rather than one row counted four times.
const NOTIFICATION_TYPES: [&str; 4] = [
    trakkt_auth::notification_service::TYPE_ASSIGNED,
    trakkt_auth::notification_service::TYPE_COMMENTED,
    trakkt_auth::notification_service::TYPE_STATUS_CHANGED,
    trakkt_auth::notification_service::TYPE_PRIORITY_CHANGED,
];

/// One `sync_log` row as the notification assertions read it.
///
/// `visibility_user_id` is the column `CascadeEntry` has no need of and this
/// body exists for: it is what keeps a notification out of every other member's
/// delta.
#[derive(sqlx::FromRow)]
struct NotificationEntry {
    entity_type: String,
    entity_id: String,
    action: String,
    visibility_user_id: Option<String>,
    data: Option<String>,
}

/// Create one notification per [`NOTIFICATION_TYPES`] entry for [`USER`],
/// returning their ids in table order.
///
/// Through the real service, so the rows carry whatever `create_notification`
/// actually writes; the ids come back from the table because it returns `()`.
async fn seed_notifications(db: &DbPool, issue_id: &str) -> Vec<String> {
    for notification_type in NOTIFICATION_TYPES {
        trakkt_auth::notification_service::create_notification(
            db,
            WORKSPACE,
            USER,
            issue_id,
            notification_type,
            Some(SECOND_USER),
            None,
            ActionSource::User,
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("seed the {notification_type} notification: {e}"));
    }

    #[derive(sqlx::FromRow)]
    struct Row {
        notification_id: String,
    }
    let ids: Vec<String> = db_fetch_all!(
        db,
        Row,
        "SELECT notification_id FROM notifications WHERE user_id = $1 ORDER BY notification_id",
        USER
    )
    .expect("read back the ids of the seeded notifications")
    .into_iter()
    .map(|row| row.notification_id)
    .collect();

    assert_eq!(
        ids.len(),
        NOTIFICATION_TYPES.len(),
        "the fixture must seed one notification per type, or a phase asserting \
         four entries is asserting something else"
    );
    ids
}

/// The `sync_log` rows written above `watermark`, checked for the shape all the
/// state-change entry points share, and returned as `(entity_id, payload)`
/// ordered by entity id.
///
/// The payload is decoded rather than compared as bytes. Postgres parses JSONB
/// on the way in and re-serialises it on the way out, so a payload written as
/// `{"read":true}` reads back as `{"read": true}` while SQLite hands back the
/// TEXT it was given — a byte comparison would fail on Postgres with nothing
/// wrong.
async fn notification_updates_above(
    db: &DbPool,
    watermark: i64,
    phase: &str,
) -> Vec<(String, serde_json::Value)> {
    let entries: Vec<NotificationEntry> = db_fetch_all!(
        db,
        NotificationEntry,
        "SELECT entity_type, entity_id, action, visibility_user_id, \
                CAST(data AS TEXT) AS data \
         FROM sync_log WHERE sync_id > $1 ORDER BY entity_id ASC",
        watermark
    )
    .unwrap_or_else(|e| panic!("read the sync entries {phase} wrote: {e}"));

    let mut decoded = Vec::new();
    for entry in entries {
        assert_eq!(
            (entry.entity_type.as_str(), entry.action.as_str()),
            (entity_types::NOTIFICATION, "update"),
            "{phase} must write NOTIFICATION `update` entries and nothing else. \
             `delete` is what issue_service's cascade writes when it physically \
             destroys a notification, and a client that cannot tell the two \
             apart evicts a row that was only hidden. Got {:?} for {}",
            (&entry.entity_type, &entry.action),
            entry.entity_id
        );
        assert_eq!(
            entry.visibility_user_id.as_deref(),
            Some(USER),
            "{phase} must address every entry to the recipient. A NULL here is \
             workspace-visible — the TRA-9920 leak — and every other member \
             replays one user's inbox. Entry for {}",
            entry.entity_id
        );

        let raw = entry.data.as_deref().unwrap_or_else(|| {
            panic!(
                "{phase} must persist a payload for {}: cache/apply.rs discards \
                 a data-less update before it reaches IndexedDB, so the entry \
                 would arrive and change nothing",
                entry.entity_id
            )
        });
        let payload: serde_json::Value = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("parsing the {phase} payload {raw:?}: {e}"));

        assert_eq!(
            payload.get("issue_title").and_then(serde_json::Value::as_str),
            Some(NOTIFIED_ISSUE_TITLE),
            "{phase}'s payload must carry the joined issue title. The read-back \
             runs on the open transaction; a SELECT that lost \
             NOTIFICATION_SELECT's LEFT JOINs would hand the client an issue id \
             it has nothing to render. Got {payload}"
        );

        decoded.push((entry.entity_id, payload));
    }
    decoded
}

/// A `bool` field of a payload, or a panic naming the field and the phase.
fn payload_bool(payload: &serde_json::Value, field: &str, phase: &str) -> bool {
    payload
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| {
            panic!("{phase}'s payload must carry a boolean `{field}`; got {payload}")
        })
}

dual_backend_test! {
    /// Notification state changes, each writing one recipient-scoped `update`
    /// per row, with the payload the client applies.
    ///
    /// Four `bulk_*` phases over the same four notifications — mark read, mark
    /// unread, dismiss, restore — for sixteen entries, then two `mark_as_read`
    /// calls on one of them. Each phase reads only the rows above its own
    /// watermark, so a phase that wrote none is caught rather than covered by
    /// its predecessor's.
    ///
    /// The `read` and `deleted_at` assertions are the point of running the
    /// phases in that order: `read` has to move `false → true → false` and
    /// `deleted_at` `NULL → stamped → NULL`, so an implementation that wrote a
    /// constant payload, or read the row back before its UPDATE, disagrees with
    /// one of the four.
    async fn notification_state_changes_log_one_recipient_scoped_update_each(db) {
        seed_tenancy(db).await;
        seed_second_workspace_member(db, SECOND_USER).await;
        let issue = create_issue(db, NOTIFIED_ISSUE_TITLE).await;
        let ids = seed_notifications(db, &issue.issue_id).await;

        // (phase name, the call, the `read` the payload must show, whether
        // `deleted_at` must be stamped). Driven from a list because the four
        // differ only in those three things, and a copy per phase is four places
        // for the shape assertions to drift apart.
        let mut all_entity_ids: Vec<String> = Vec::new();

        for (phase, expected_read, expect_deleted) in [
            ("bulk_mark_as_read", true, false),
            ("bulk_mark_as_unread", false, false),
            ("bulk_delete_notifications", false, true),
            ("bulk_restore_notifications", false, false),
        ] {
            let watermark = sync_watermark(db).await;

            let outcome = match phase {
                "bulk_mark_as_read" => {
                    trakkt_auth::notification_service::bulk_mark_as_read(db, &ids, USER, None).await
                }
                "bulk_mark_as_unread" => {
                    trakkt_auth::notification_service::bulk_mark_as_unread(db, &ids, USER, None)
                        .await
                }
                "bulk_delete_notifications" => {
                    trakkt_auth::notification_service::bulk_delete_notifications(
                        db, &ids, USER, None,
                    )
                    .await
                }
                _ => {
                    trakkt_auth::notification_service::bulk_restore_notifications(
                        db, &ids, USER, None,
                    )
                    .await
                }
            };
            outcome.unwrap_or_else(|e| panic!("{phase} over the four seeded notifications: {e}"));

            let entries = notification_updates_above(db, watermark, phase).await;

            let touched: Vec<String> =
                entries.iter().map(|(id, _)| id.clone()).collect();
            assert_eq!(
                touched, ids,
                "{phase} must write exactly one entry per notification it \
                 changed — no more, or a client applies an update twice; no \
                 fewer, or a tab that reconnects never learns the row moved"
            );

            for (entity_id, payload) in &entries {
                assert_eq!(
                    payload_bool(payload, "read", phase), expected_read,
                    "{phase}'s payload for {entity_id} must show the read state \
                     it just wrote. A payload read before the UPDATE carries the \
                     old value, and the client is told the row is unchanged"
                );
                assert_eq!(
                    payload.get("deleted_at").map(serde_json::Value::is_null),
                    Some(!expect_deleted),
                    "{phase}'s payload for {entity_id} must show deleted_at \
                     {}. The inbox filters on that field, so a stale one leaves \
                     a dismissed notification on screen. Got {payload}",
                    if expect_deleted { "stamped" } else { "cleared" }
                );
                assert_eq!(
                    payload.get("user_id").and_then(serde_json::Value::as_str),
                    Some(USER),
                    "{phase}'s payload for {entity_id} must name its recipient — \
                     it is what the visibility backfill reads, and what a client \
                     checks the row against. Got {payload}"
                );
            }

            all_entity_ids.extend(touched);
        }

        assert_eq!(
            all_entity_ids.len(),
            NOTIFICATION_TYPES.len() * 4,
            "the four phases must write sixteen entries between them; a phase \
             whose predicate excluded every row writes none, and the per-phase \
             assertions above would each be vacuous"
        );

        // `mark_as_read` is the odd one out and its docs say so: it carries no
        // read-state predicate, only `deleted_at IS NULL`, which setting `read`
        // leaves standing. So re-reading an already-read notification writes a
        // second entry, where every `bulk_*` phase above would have written
        // none. That is the claim `affected_notification_ids`' doc comment makes
        // about both backends, asserted here so it stays true.
        for attempt in ["the first read", "the same read again"] {
            let watermark = sync_watermark(db).await;
            trakkt_auth::notification_service::mark_as_read(db, &ids[0], USER, None)
                .await
                .unwrap_or_else(|e| panic!("mark_as_read on {attempt}: {e}"));

            let entries = notification_updates_above(db, watermark, attempt).await;
            let touched: Vec<String> = entries.iter().map(|(id, _)| id.clone()).collect();
            assert_eq!(
                touched, vec![ids[0].clone()],
                "{attempt} must write exactly one entry, for the one \
                 notification named. A predicate on `read` here would make the \
                 second attempt write none, and the doc comment on \
                 `affected_notification_ids` would be describing the wrong \
                 function"
            );
            assert!(
                payload_bool(&entries[0].1, "read", attempt),
                "{attempt} must leave the payload showing `read`: {:?}",
                entries[0].1
            );
        }

        // The persisted column is only half of it. `get_entries_since` is what a
        // reconnecting client actually calls, and it is a separate query with a
        // dialect arm of its own — a correct `visibility_user_id` filtered by a
        // wrong WHERE still leaks.
        let second_members_delta: Vec<String> = get_entries_since(db, WORKSPACE, SECOND_USER, 0, 10_000)
            .await
            .expect("read the delta a reconnecting second member would be sent")
            .into_iter()
            .filter(|action| action.entity_type == entity_types::NOTIFICATION)
            .map(|action| action.entity_id)
            .collect();
        assert!(
            second_members_delta.is_empty(),
            "the other member's delta must carry none of the recipient's \
             notifications, in any action: {second_members_delta:?}"
        );

        // Not vacuous: the recipient's own delta does carry them, so the
        // assertion above is about audience and not about the entries being
        // absent outright. Filtered to `Update` because create_notification
        // wrote an `Insert` for each of these same ids — an unfiltered check
        // would hold even if the four phases had written nothing at all.
        let recipients_updates: Vec<String> = get_entries_since(db, WORKSPACE, USER, 0, 10_000)
            .await
            .expect("read the delta the recipient would be sent")
            .into_iter()
            .filter(|action| {
                action.entity_type == entity_types::NOTIFICATION
                    && matches!(action.action, SyncActionType::Update)
            })
            .map(|action| action.entity_id)
            .collect();
        for id in &ids {
            assert!(
                recipients_updates.contains(id),
                "the recipient's own delta must replay the update for {id}; \
                 scoping that is too narrow leaves their other tabs stale \
                 through every reconnect: {recipients_updates:?}"
            );
        }
    }
}

// ─── Every favorite goes with its target ─────────────────────────────────────
//
// `favorites.target_id` is polymorphic TEXT with no foreign key in either
// dialect, so no cascade — declared or otherwise — removes a favorite when the
// thing it pins is deleted. TRA-10025 made each parent's delete path do it
// instead, which is only sound while every parent is enumerated. These tests are
// what hold that enumeration to its word.
//
// Run on both backends because the *reason* the two dialects behave alike here
// is worth re-checking rather than assuming: the second-order case below leans
// on `views.team_id ON DELETE CASCADE`, which Postgres has always had and SQLite
// only gained in `migrations-sqlite/20260803100000_dual_backend_fk_parity.sql`.

/// The user who pins things alongside [`USER`], so no assertion below can pass
/// against an implementation that handles one owner and stops.
const PINNING_USER: &str = "usr_dialect_pinner";

/// A favorite's owner, as the `sync_log` reports it.
#[derive(sqlx::FromRow)]
struct FavoriteEntry {
    entity_id: String,
    action: String,
    visibility_user_id: Option<String>,
}

/// How many favorites still point at `(target, target_id)`.
async fn favorites_naming(db: &DbPool, target: FavoriteTarget, target_id: &str) -> i64 {
    db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM favorites WHERE target_type = $1 AND target_id = $2",
        target.as_str(),
        target_id
    )
    .expect("count the favorites still naming the deleted target")
}

/// Every FAVORITE entry written above `watermark`, sorted by owner.
async fn favorite_entries_above(
    db: &DbPool,
    watermark: i64,
) -> Vec<(String, String, Option<String>)> {
    let rows: Vec<FavoriteEntry> = db_fetch_all!(
        db,
        FavoriteEntry,
        "SELECT entity_id, action, visibility_user_id FROM sync_log \
         WHERE sync_id > $1 AND entity_type = $2 ORDER BY sync_id ASC",
        watermark,
        entity_types::FAVORITE
    )
    .expect("read the FAVORITE entries the delete wrote");

    let mut out: Vec<(String, String, Option<String>)> = rows
        .into_iter()
        .map(|r| (r.entity_id, r.action, r.visibility_user_id))
        .collect();
    out.sort();
    out
}

/// Pin `(target, target_id)` for `user_id` and return the new `favorite_id`.
async fn pin(db: &DbPool, user_id: &str, target: FavoriteTarget, target_id: &str) -> String {
    trakkt_auth::favorite_service::add_favorite(db, user_id, WORKSPACE, target, target_id, None)
        .await
        .expect("pin the target the delete must unpin")
        .favorite_id
}

/// Create one instance of `target`, named `label` so two are distinguishable.
///
/// Exhaustive over [`FavoriteTarget`] on purpose — see
/// `every_favorite_target_is_deleted_with_its_target`.
async fn create_favoritable(db: &DbPool, target: FavoriteTarget, label: &str) -> String {
    match target {
        FavoriteTarget::Issue => create_issue(db, label).await.issue_id,
        FavoriteTarget::Project => {
            trakkt_auth::project_service::create_project(
                db,
                &trakkt_auth::project_service::CreateProjectParams {
                    workspace_id: WORKSPACE,
                    name: label,
                    description: None,
                    icon: None,
                    color: None,
                    lead_id: None,
                    start_date: None,
                    target_date: None,
                },
                None,
            )
            .await
            .expect("create the project to be pinned")
            .project_id
        }
        FavoriteTarget::Team => {
            // A team of its own, never `TEAM`: `delete_team` refuses to remove
            // the last team in a workspace, so the seeded one has to survive to
            // let this one go.
            trakkt_auth::team_service::create_team(
                db,
                &trakkt_auth::team_service::CreateTeamParams {
                    workspace_id: WORKSPACE,
                    name: label,
                    key: team_key_for(label),
                    description: None,
                    icon: None,
                    creator_id: None,
                },
                None,
            )
            .await
            .expect("create the team to be pinned")
            .team_id
        }
        FavoriteTarget::View => {
            // `team_id: None` — a workspace-level view. A team-scoped one would
            // be swept away by the Team case's `delete_team` instead of by
            // `delete_view`, which is a different cascade with its own test
            // below.
            trakkt_auth::view_service::create_view(
                db,
                &trakkt_auth::view_service::CreateViewParams {
                    workspace_id: WORKSPACE,
                    user_id: USER,
                    name: label,
                    icon: None,
                    filters: "{}",
                    display_options: "{}",
                    is_shared: true,
                    team_id: None,
                    position: 0,
                },
                None,
            )
            .await
            .expect("create the view to be pinned")
            .view_id
        }
    }
}

/// Delete `target_id` through the real service function for its type.
///
/// Exhaustive over [`FavoriteTarget`] on purpose — see
/// `every_favorite_target_is_deleted_with_its_target`. Going through the service
/// rather than a `DELETE` statement is the point: a raw statement would prove
/// only that SQL removes rows, and it is the service layer that owes the
/// favorites their removal and their `sync_log` entries.
async fn delete_favoritable(db: &DbPool, target: FavoriteTarget, target_id: &str) {
    match target {
        FavoriteTarget::Issue => {
            let number: i32 = db_fetch_scalar!(
                db,
                i32,
                "SELECT number FROM issues WHERE issue_id = $1",
                target_id
            )
            .expect("read the issue's team-scoped number, which is how it is deleted");
            trakkt_auth::issue_service::delete_issue(db, WORKSPACE, TEAM_KEY, number, None)
                .await
                .expect("delete the pinned issue");
        }
        FavoriteTarget::Project => {
            trakkt_auth::project_service::delete_project(db, target_id, None)
                .await
                .expect("delete the pinned project");
        }
        FavoriteTarget::Team => {
            trakkt_auth::team_service::delete_team(db, target_id, WORKSPACE, None, None, None)
                .await
                .expect("delete the pinned team");
        }
        FavoriteTarget::View => {
            trakkt_auth::view_service::delete_view(db, target_id, WORKSPACE, None)
                .await
                .expect("delete the pinned view");
        }
    }
}

/// A distinct 2-5 character uppercase team key per label.
fn team_key_for(label: &str) -> &'static str {
    match label {
        "doomed" => "DOOM",
        "survivor" => "SURV",
        other => panic!("no team key allocated for {other:?}"),
    }
}

dual_backend_test! {
    /// Deleting a favorited entity leaves no favorite behind — for *every* type
    /// a favorite can name.
    ///
    /// # Why this test is shaped like this
    ///
    /// `favorites.target_id` carries no foreign key, so nothing in the schema
    /// removes these rows; each parent's delete path has to, and "each parent"
    /// is a set that grows. A test naming one type would have passed throughout
    /// the bug it exists to catch — before TRA-10025 exactly one of the four
    /// delete paths removed favorites (`delete_team`, and even that wrote no
    /// `sync_log` entry), so a projects-only test and a teams-only test would
    /// have disagreed about whether the codebase was correct.
    ///
    /// So the loop is over [`FavoriteTarget::ALL`] and the dispatch is an
    /// exhaustive `match` in [`create_favoritable`] and [`delete_favoritable`].
    /// Adding a variant to `FavoriteTarget` does not compile until both arms are
    /// written, and the arm has to name a real service function, because
    /// everything asserted below is asserted after that function has run. That
    /// is the mechanism: not a comment asking the next person to remember, but
    /// a build failure telling them what is missing.
    ///
    /// Three things are checked per type, and each kills a different wrong fix:
    ///
    /// 1. no favorite still names the deleted target — the defect itself;
    /// 2. a survivor of the same type keeps its favorites — so "delete
    ///    everything" cannot pass;
    /// 3. one FAVORITE `delete` entry per removed row, scoped to that row's own
    ///    owner — so a server-side delete cannot go unannounced (the row would
    ///    sit in the owner's IndexedDB through every reconnect, which is
    ///    TRA-9971/TRA-9957), and cannot be announced to the wrong people (a
    ///    favorite is private; `SyncAudience::User` is what keeps it that way).
    async fn every_favorite_target_is_deleted_with_its_target(db) {
        seed_tenancy(db).await;
        seed_user(db, PINNING_USER, "dialect-pinner@example.test")
            .await
            .expect("seed the second user who pins the same targets");
        db_execute!(
            db,
            "INSERT INTO workspace_users (workspace_id, user_id) VALUES ($1, $2)",
            WORKSPACE,
            PINNING_USER
        )
        .expect("make the second pinner a member, as add_favorite's callers are");

        for target in FavoriteTarget::ALL {
            let target = *target;

            let doomed = create_favoritable(db, target, "doomed").await;
            let survivor = create_favoritable(db, target, "survivor").await;

            // Two owners on the doomed target: one entry that happens to name
            // the right user is indistinguishable from a per-row scoping until
            // there are two rows with two different owners.
            let doomed_by_owner = pin(db, USER, target, &doomed).await;
            let doomed_by_pinner = pin(db, PINNING_USER, target, &doomed).await;
            let survivor_fav = pin(db, USER, target, &survivor).await;

            let watermark = sync_watermark(db).await;

            delete_favoritable(db, target, &doomed).await;

            assert_eq!(
                favorites_naming(db, target, &doomed).await,
                0,
                "deleting the {target} left a favorite pointing at {doomed}. The row \
                 is cached and `sync_bootstrap` streams it, so it comes back after \
                 every SyncReset — the server still has it. `{target}` needs an arm \
                 in the parent's delete path calling \
                 `favorite_service::doomed_favorites_tx`."
            );

            assert_eq!(
                favorites_naming(db, target, &survivor).await,
                1,
                "the surviving {target} {survivor} lost its favorite {survivor_fav}. \
                 The cascade must remove the favorites naming the deleted row and no \
                 others; without this the assertion above would pass against \
                 `DELETE FROM favorites`."
            );

            let mut expected = vec![
                (doomed_by_owner.clone(), "delete".to_string(), Some(USER.to_string())),
                (doomed_by_pinner.clone(), "delete".to_string(), Some(PINNING_USER.to_string())),
            ];
            expected.sort();

            assert_eq!(
                favorite_entries_above(db, watermark).await,
                expected,
                "deleting the {target} must write one FAVORITE delete entry per \
                 removed row, each scoped to that row's own owner. No entry leaves \
                 the favorite in that owner's IndexedDB forever with no later delta \
                 able to evict it. A NULL visibility_user_id publishes who pinned \
                 what to the whole workspace, which is the leak \
                 `SyncAudience::User` exists to prevent."
            );
        }
    }
}

dual_backend_test! {
    /// Deleting a team takes the favorites of the views it cascades away, not
    /// just the favorite of the team itself.
    ///
    /// This is the second-order case, and it is the one a per-type fix misses.
    /// `views.team_id` is `ON DELETE CASCADE` in both dialects, so `DELETE FROM
    /// teams` destroys every view scoped to that team without `delete_view` ever
    /// running — so whatever `delete_view` does about favorites is never reached,
    /// and a favorite pinning such a view is stranded exactly as if the view had
    /// been deleted directly.
    ///
    /// `every_favorite_target_is_deleted_with_its_target` cannot catch this: its
    /// View case deliberately uses a workspace-level view so that the two
    /// cascades stay separable. This is the test for the other half.
    async fn deleting_a_team_unpins_the_views_it_cascades_away(db) {
        seed_tenancy(db).await;

        let doomed_team = trakkt_auth::team_service::create_team(
            db,
            &trakkt_auth::team_service::CreateTeamParams {
                workspace_id: WORKSPACE,
                name: "doomed",
                key: team_key_for("doomed"),
                description: None,
                icon: None,
                creator_id: None,
            },
            None,
        )
        .await
        .expect("create the team whose views the delete must cascade")
        .team_id;

        let scoped_view = trakkt_auth::view_service::create_view(
            db,
            &trakkt_auth::view_service::CreateViewParams {
                workspace_id: WORKSPACE,
                user_id: USER,
                name: "scoped to the doomed team",
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared: true,
                team_id: Some(&doomed_team),
                position: 0,
            },
            None,
        )
        .await
        .expect("create the team-scoped view the team delete cascades away")
        .view_id;

        // A workspace-level view, which the team delete must leave alone —
        // otherwise "unpinned the right view" and "unpinned every view" are the
        // same result.
        let workspace_view = trakkt_auth::view_service::create_view(
            db,
            &trakkt_auth::view_service::CreateViewParams {
                workspace_id: WORKSPACE,
                user_id: USER,
                name: "not scoped to any team",
                icon: None,
                filters: "{}",
                display_options: "{}",
                is_shared: true,
                team_id: None,
                position: 0,
            },
            None,
        )
        .await
        .expect("create the workspace-level view the team delete must not touch")
        .view_id;

        let scoped_fav = pin(db, USER, FavoriteTarget::View, &scoped_view).await;
        pin(db, USER, FavoriteTarget::View, &workspace_view).await;

        let watermark = sync_watermark(db).await;

        trakkt_auth::team_service::delete_team(db, &doomed_team, WORKSPACE, None, None, None)
            .await
            .expect("delete the team the view is scoped to");

        // The premise: the view really is gone, cascaded by the schema.
        assert_eq!(
            db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM views WHERE view_id = $1", &scoped_view)
                .expect("read the cascaded view back"),
            0,
            "the team-scoped view must be gone — if it is not, this test is not \
             exercising the cascade it claims to and every assertion below is \
             vacuous"
        );

        assert_eq!(
            favorites_naming(db, FavoriteTarget::View, &scoped_view).await,
            0,
            "the favorite pinning the cascaded view {scoped_view} survived its \
             view. `delete_view` never ran — `DELETE FROM teams` took the view \
             through `views.team_id ON DELETE CASCADE` — so `delete_team` is what \
             owes this favorite its removal."
        );

        assert_eq!(
            favorites_naming(db, FavoriteTarget::View, &workspace_view).await,
            1,
            "the workspace-level view {workspace_view} is not scoped to the deleted \
             team, so its favorite must survive. Without this the assertion above \
             would pass against a cascade that unpinned every view in the workspace."
        );

        assert!(
            favorite_entries_above(db, watermark).await.contains(&(
                scoped_fav.clone(),
                "delete".to_string(),
                Some(USER.to_string())
            )),
            "the cascaded view's favorite needs a FAVORITE delete entry scoped to \
             its owner, or it stays in their IndexedDB through every reconnect \
             while the server no longer has it"
        );
    }
}

dual_backend_test! {
    /// A FAVORITE entry that cannot be written rolls the whole delete back.
    ///
    /// The favorites are removed inside the caller's transaction, so the rows and
    /// the entries that evict them from their owners' caches commit together or
    /// not at all. If they could come apart, the favorite would be gone from the
    /// server and still in the owner's IndexedDB, with no later delta able to
    /// repair it — which is the whole defect, one table over.
    ///
    /// Narrowed with `reject_sync_log_inserts_of_type` rather than the blanket
    /// trigger: `delete_project` writes a PROJECT entry, then one per cascaded
    /// member, milestone and update, and only then the FAVORITE entries. A
    /// blanket trigger would abort the PROJECT entry and the assertion would pass
    /// without the code under test ever being reached.
    async fn a_rejected_favorite_entry_rolls_the_project_delete_back(db) {
        seed_tenancy(db).await;

        let project = create_favoritable(db, FavoriteTarget::Project, "doomed").await;
        let favorite = pin(db, USER, FavoriteTarget::Project, &project).await;

        reject_sync_log_inserts_of_type(db, entity_types::FAVORITE).await;

        let err = trakkt_auth::project_service::delete_project(db, &project, None)
            .await
            .expect_err("the delete must fail when its FAVORITE entry cannot be written");

        clear_sync_log_rejection(db).await;

        assert!(
            err.to_string().contains("sync_log insert rejected"),
            "the delete must fail for the reason the trigger gives, not some other \
             error that would make this test pass for the wrong reason: {err}"
        );

        assert_eq!(
            favorites_naming(db, FavoriteTarget::Project, &project).await,
            1,
            "the favorite {favorite} must be back — a favorite removed without the \
             entry that announces it is exactly the state this transaction exists \
             to make unreachable"
        );

        assert_eq!(
            db_fetch_scalar!(
                db,
                i64,
                "SELECT COUNT(*) FROM projects WHERE project_id = $1",
                &project
            )
            .expect("read the project back"),
            1,
            "the project must be back too: the favorites are removed on the same \
             transaction, so a rollback that spared them and not the project would \
             mean they are not"
        );
    }
}
