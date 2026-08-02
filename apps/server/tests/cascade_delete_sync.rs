// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a client is left holding after another browser deletes an issue.
//!
//! `crates/trakkt-auth`'s own tests assert the `sync_log` rows `delete_issue`
//! writes. They cannot assert the thing TRA-9957 is actually about, because the
//! code that decides what survives in a client's cache lives in `trakkt-ui` and
//! `trakkt-auth` does not — and must not — depend on it. `trakkt-server` depends
//! on both, so this is the one place a Rust test can run the server's real sync
//! output through the client's real apply path.
//!
//! What runs here is the production code on both sides:
//! `issue_service::delete_issue` writes the entries,
//! `sync_log_service::get_entries_since` reads them back exactly as
//! `handle_sync_delta` does, and `cache::apply::enqueue_cache_writes` plus
//! `cache::idb_writer::run_writer` turn them into cache operations.
//!
//! What is *not* the real thing is IndexedDB itself, which is a browser API with
//! no native implementation. `run_writer` drives an [`IdbSink`], and the sink
//! here is a map keyed by `(entity_type, entity_id)`. That is a
//! workspace-free simplification of the real key: `entity_key` in
//! `crates/trakkt-ui/src/cache/db.rs:95` is the 3-part
//! `entity_type\0workspace_id\0entity_id`. The simplification is sound only
//! because this fixture is single-workspace, so no two rows can collide on the
//! shortened key — a multi-workspace fixture would need the third component.
//!
//! So this proves the op stream and its effect on a keyed store; it does not
//! prove the browser's storage layer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use leptos::prelude::*;

use trakkt_auth::sync_log_service::get_entries_since;
use trakkt_core::test_helpers::{seed_team, seed_user, seed_workspace, test_pool};
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;
use trakkt_types::models::CreateIssueParams;
use trakkt_types::sync::entity_types;
use trakkt_ui::cache::apply::{apply_action_to_memory, enqueue_cache_writes};
use trakkt_ui::cache::idb_writer::{channel, run_writer, IdbSink, SinkError};
use trakkt_ui::cache::store::SyncStore;

const USER: &str = "usr_cascade";
const WORKSPACE: &str = "ws_cascade";
const TEAM: &str = "team_cascade";
const TEAM_KEY: &str = "CAS";

/// Rows a cache holds, keyed the way the IndexedDB sink keys them.
type CacheRows = Rc<RefCell<HashMap<(String, String), String>>>;

/// An [`IdbSink`] that keeps the rows in a map instead of IndexedDB.
///
/// Applies the same three operations the browser sink does, so replaying an
/// action stream through it leaves the set of keys a real cache would hold.
#[derive(Default)]
struct MapSink {
    rows: CacheRows,
}

impl MapSink {
    fn rows(&self) -> CacheRows {
        Rc::clone(&self.rows)
    }
}

impl IdbSink for MapSink {
    async fn upsert(
        &self,
        entity_type: &str,
        entity_id: &str,
        json: &str,
        _ts: &str,
    ) -> Result<(), SinkError> {
        self.rows.borrow_mut().insert(
            (entity_type.to_owned(), entity_id.to_owned()),
            json.to_owned(),
        );
        Ok(())
    }

    async fn delete(&self, entity_type: &str, entity_id: &str) -> Result<(), SinkError> {
        self.rows
            .borrow_mut()
            .remove(&(entity_type.to_owned(), entity_id.to_owned()));
        Ok(())
    }

    async fn delete_all_of_type(&self, entity_type: &str) -> Result<(), SinkError> {
        self.rows.borrow_mut().retain(|(et, _), _| et != entity_type);
        Ok(())
    }

    async fn set_cursor(&self, _cursor: &str) -> Result<(), SinkError> {
        Ok(())
    }

    async fn set_schema_hash(&self) -> Result<(), SinkError> {
        Ok(())
    }
}

/// A workspace with one member, one team and the default statuses.
///
/// One member on purpose: `create_comment` notifies an issue's watchers, and
/// `notifications.issue_id` has no `ON DELETE` clause in either dialect, so a
/// notification row would make the DELETE fail on the foreign key rather than
/// cascade. With a single user the self-notification suppression keeps the table
/// empty, which is asserted below rather than assumed.
async fn seeded_workspace() -> DbPool {
    let db = test_pool().await.expect("migrated in-memory pool");

    seed_user(&db, USER, "cascade@example.test")
        .await
        .expect("seed user");
    seed_workspace(&db, WORKSPACE, USER)
        .await
        .expect("seed workspace");
    seed_team(&db, TEAM, WORKSPACE, TEAM_KEY)
        .await
        .expect("seed team");
    trakkt_auth::status_service::seed_default_statuses(&db, WORKSPACE)
        .await
        .expect("seed default statuses");

    db
}

/// Create an issue with a description, so it produces an `issue_content` cache
/// row as well as an `issue` one.
async fn seed_issue(db: &DbPool, title: &str) -> trakkt_types::models::Issue {
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
    .expect("create issue")
}

async fn seed_comments(db: &DbPool, issue_id: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for n in 0..count {
        let comment = trakkt_auth::comment_service::create_comment(
            db,
            issue_id,
            USER,
            &format!("comment {n}"),
            None,
            ActionSource::User,
            None,
            None,
        )
        .await
        .expect("create comment");
        ids.push(comment.comment_id);
    }
    ids
}

#[tokio::test]
async fn a_client_replaying_the_delta_keeps_neither_the_issue_nor_its_comments() {
    let db = seeded_workspace().await;

    let doomed = seed_issue(&db, "Deleted in another browser").await;
    let survivor = seed_issue(&db, "Still here").await;
    let doomed_comments = seed_comments(&db, &doomed.issue_id, 3).await;
    let survivor_comments = seed_comments(&db, &survivor.issue_id, 2).await;

    // The fixture's own precondition: with a notification present the DELETE
    // below would fail on the foreign key instead of cascading, and this test
    // would be proving something else entirely.
    let notifications: i64 = trakkt_core::db_fetch_scalar!(
        &db,
        i64,
        "SELECT COUNT(*) FROM notifications WHERE issue_id = $1",
        &doomed.issue_id
    )
    .expect("count notifications on the issue about to be deleted");
    assert_eq!(
        notifications, 0,
        "the seed must not leave a notification on the doomed issue — \
         notifications.issue_id does not cascade, so one would abort the DELETE"
    );

    trakkt_auth::issue_service::delete_issue(&db, WORKSPACE, TEAM_KEY, doomed.number, None)
        .await
        .expect("delete the issue in the browser that owns the tab");

    // Everything a fresh client would be sent: the whole log, in order, read
    // through the same function `handle_sync_delta` uses.
    let actions = get_entries_since(&db, WORKSPACE, USER, 0, 10_000)
        .await
        .expect("read the delta a replaying client would receive");

    let owner = Owner::new();
    owner.set();
    let store = SyncStore::new();
    let (writer, ops) = channel();

    for action in &actions {
        apply_action_to_memory(&store, action);
        enqueue_cache_writes(&writer, action);
    }

    // The writer loop runs until every handle is dropped, so the queue has to be
    // closed before it is drained.
    drop(writer);
    let sink = MapSink::default();
    let cache = sink.rows();
    run_writer(sink, ops).await;

    let cache = cache.borrow();
    let holds = |entity_type: &str, entity_id: &str| {
        cache.contains_key(&(entity_type.to_owned(), entity_id.to_owned()))
    };

    assert!(
        !holds(entity_types::ISSUE, &doomed.issue_id),
        "the deleted issue must not survive the replay"
    );
    assert!(
        !holds(entity_types::ISSUE_CONTENT, &doomed.issue_id),
        "the deleted issue's description is a cache row of its own, and the \
         issue delete evicts it"
    );
    for comment_id in &doomed_comments {
        assert!(
            !holds(entity_types::COMMENT, comment_id),
            "comment {comment_id} belonged to the deleted issue; with no delete \
             action naming it, it stays cached through every reconnect — that is \
             TRA-9957"
        );
    }

    // Not vacuous: the same replay leaves the untouched issue and its comments
    // in the cache, so "nothing cached" and "the right things evicted" are
    // distinguishable outcomes.
    assert!(
        holds(entity_types::ISSUE, &survivor.issue_id),
        "the surviving issue must still be cached, or this test would pass \
         against a replay that cached nothing at all"
    );
    assert!(
        holds(entity_types::ISSUE_CONTENT, &survivor.issue_id),
        "the surviving issue's description is cached too"
    );
    for comment_id in &survivor_comments {
        assert!(
            holds(entity_types::COMMENT, comment_id),
            "comment {comment_id} belongs to the surviving issue and must be \
             cached"
        );
    }

    // The in-memory half of the same replay, which is what the issue list on
    // screen is rendered from.
    let issue_ids: Vec<String> = store
        .issues()
        .get()
        .into_iter()
        .map(|i| i.issue_id)
        .collect();
    assert_eq!(
        issue_ids,
        vec![survivor.issue_id.clone()],
        "the reactive store must be left holding only the surviving issue"
    );
}
