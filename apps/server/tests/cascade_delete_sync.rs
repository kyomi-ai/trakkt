// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a client is left holding after another browser deletes an issue or a
//! project.
//!
//! `apps/server/tests/postgres_dialect.rs` asserts the `sync_log` rows the two
//! deletes write. Neither it nor `crates/trakkt-auth`'s own tests can assert the
//! thing TRA-9957 and TRA-9971 are actually about, because the code that decides
//! what survives in a client's cache lives in `trakkt-ui` and `trakkt-auth` does
//! not — and must not — depend on it. `trakkt-server` depends on both, so this is
//! the one place a Rust test can run the server's real sync output through the
//! client's real apply path.
//!
//! What runs here is the production code on both sides:
//! `issue_service::delete_issue` and `project_service::delete_project` write the
//! entries, `sync_log_service::get_entries_since` reads them back exactly as
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
/// A second member, so the per-user half of the delta has someone to exclude.
const OTHER_USER: &str = "usr_cascade_other";
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

/// A workspace with two members, one team and the default statuses.
///
/// Two on purpose. Notifications are the one cascaded type here that is not
/// workspace-visible — `notification_service::create_notification` writes them
/// with `SyncAudience::User`, so `get_entries_since` hands each member only
/// their own. A single-member fixture cannot tell a correctly scoped delete
/// entry apart from one published to the whole workspace.
async fn seeded_workspace() -> DbPool {
    let db = test_pool().await.expect("migrated in-memory pool");

    seed_user(&db, USER, "cascade@example.test")
        .await
        .expect("seed user");
    seed_user(&db, OTHER_USER, "cascade-other@example.test")
        .await
        .expect("seed the second member");
    seed_workspace(&db, WORKSPACE, USER)
        .await
        .expect("seed workspace");
    // `seed_workspace` enrols only the owner, and `workspace_users` is what
    // decides who a change is broadcast to. `role` and `active` are left to the
    // schema's defaults, as `sync_log_service`'s own two-member fixture does.
    trakkt_core::db_execute!(
        &db,
        "INSERT INTO workspace_users (workspace_id, user_id) VALUES ($1, $2)",
        WORKSPACE,
        OTHER_USER
    )
    .expect("enrol the second member in the workspace");
    seed_team(&db, TEAM, WORKSPACE, TEAM_KEY)
        .await
        .expect("seed team");
    trakkt_auth::status_service::seed_default_statuses(&db, WORKSPACE)
        .await
        .expect("seed default statuses");

    db
}

/// Notify `user_id` about `issue_id` through the real service, returning the id
/// of the row it wrote.
///
/// `create_notification` returns `()`, so the id is read back from the table.
/// `(user_id, issue_id)` identifies it: no fixture here gives one member two
/// notifications on one issue.
async fn seed_notification(db: &DbPool, user_id: &str, issue_id: &str) -> String {
    trakkt_auth::notification_service::create_notification(
        db,
        WORKSPACE,
        user_id,
        issue_id,
        "status_changed",
        None,
        None,
        ActionSource::User,
        None,
        None,
    )
    .await
    .expect("create the notification a cascading delete will have to evict");

    trakkt_core::db_fetch_scalar!(
        db,
        String,
        "SELECT notification_id FROM notifications WHERE user_id = $1 AND issue_id = $2",
        user_id,
        issue_id
    )
    .expect("read back the id of the notification just created")
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
async fn a_client_replaying_the_delta_keeps_nothing_the_deleted_issue_took_with_it() {
    let db = seeded_workspace().await;

    let doomed = seed_issue(&db, "Deleted in another browser").await;
    let survivor = seed_issue(&db, "Still here").await;
    let doomed_comments = seed_comments(&db, &doomed.issue_id, 3).await;
    let survivor_comments = seed_comments(&db, &survivor.issue_id, 2).await;

    // Notifications on the doomed issue for both members, and one on the
    // survivor. Until TRA-9989 gave `notifications.issue_id` an ON DELETE
    // CASCADE these rows made the DELETE below fail on the foreign key outright,
    // which is why this fixture used to keep the table empty.
    let doomed_notification = seed_notification(&db, USER, &doomed.issue_id).await;
    let other_members_notification = seed_notification(&db, OTHER_USER, &doomed.issue_id).await;
    let survivor_notification = seed_notification(&db, USER, &survivor.issue_id).await;

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

    assert!(
        !holds(entity_types::NOTIFICATION, &doomed_notification),
        "the inbox entry for the deleted issue must not survive the replay — it \
         renders a title, number and team key that are joined from a row the \
         cascade destroyed"
    );
    assert!(
        holds(entity_types::NOTIFICATION, &survivor_notification),
        "the notification on the untouched issue must still be cached, or this \
         assertion would pass against a replay that evicted every notification"
    );
    // The other member's notification is theirs alone. `get_entries_since`
    // filters on `visibility_user_id`, so neither its insert nor its delete may
    // appear in this user's delta at all — a delete entry recorded with
    // `SyncAudience::Workspace` would put the id here.
    assert!(
        !actions
            .iter()
            .any(|a| a.entity_id == other_members_notification),
        "the other member's notification id must never reach this user's delta, \
         in either direction"
    );

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

// ─── The same question for a project delete (TRA-9971) ───────────────────────

/// Everything a project delete has to take with it, seeded through the real
/// services so each row arrives in the cache the way production puts it there.
struct SeededProject {
    project_id: String,
    /// The `entity_id` the client cached the membership under —
    /// `project_members` has no surrogate id, so it is the composite key
    /// `project_service::project_member_entity_id` builds.
    member_entity_id: String,
    milestone_id: String,
    update_id: String,
}

async fn seed_project(db: &DbPool, name: &str) -> SeededProject {
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
    .expect("create the project whose cascade the replay is made against");

    trakkt_auth::project_service::add_project_member(
        db,
        &project.project_id,
        USER,
        "member",
        WORKSPACE,
        None,
    )
    .await
    .expect("add the member whose cached row the delete must evict");

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
    .expect("create the milestone whose cached row the delete must evict");

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
    .expect("post the update whose cached row the delete must evict");

    SeededProject {
        member_entity_id: format!("{}:{USER}", project.project_id),
        project_id: project.project_id,
        milestone_id: milestone.milestone_id,
        update_id: update.update_id,
    }
}

/// Move an existing issue into a project, through the real update path.
async fn assign_to_project(db: &DbPool, number: i32, project_id: &str) {
    trakkt_auth::issue_service::update_issue(
        db,
        WORKSPACE,
        TEAM_KEY,
        number,
        &trakkt_types::models::IssueUpdate {
            project_id: Some(Some(project_id.to_owned())),
            ..Default::default()
        },
        Some(USER),
        ActionSource::User,
        None,
        None,
    )
    .await
    .expect("assign the issue to the project the delete will detach it from");
}

#[tokio::test]
async fn a_client_replaying_the_delta_keeps_nothing_the_deleted_project_took_with_it() {
    let db = seeded_workspace().await;

    let doomed = seed_project(&db, "Deleted in another browser").await;
    let survivor = seed_project(&db, "Still here").await;

    let detached = seed_issue(&db, "Was in the deleted project").await;
    let untouched = seed_issue(&db, "Was in the other project").await;
    assign_to_project(&db, detached.number, &doomed.project_id).await;
    assign_to_project(&db, untouched.number, &survivor.project_id).await;

    trakkt_auth::project_service::delete_project(&db, &doomed.project_id, None)
        .await
        .expect("delete the project in the browser that owns the tab");

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

    drop(writer);
    let sink = MapSink::default();
    let cache = sink.rows();
    run_writer(sink, ops).await;

    let cache = cache.borrow();
    let holds = |entity_type: &str, entity_id: &str| {
        cache.contains_key(&(entity_type.to_owned(), entity_id.to_owned()))
    };

    assert!(
        !holds(entity_types::PROJECT, &doomed.project_id),
        "the deleted project must not survive the replay"
    );
    assert!(
        !holds(entity_types::PROJECT_MEMBER, &doomed.member_entity_id),
        "the membership of the deleted project must not survive the replay. \
         `project_members` is emptied by a foreign key that reports nothing, so \
         without a delete entry naming this id nothing ever evicts the row — it \
         outlives every reconnect. That is TRA-9971."
    );
    assert!(
        !holds(entity_types::PROJECT_MILESTONE, &doomed.milestone_id),
        "the milestone of the deleted project must not survive the replay"
    );
    assert!(
        !holds(entity_types::PROJECT_UPDATE, &doomed.update_id),
        "the posted update of the deleted project must not survive the replay"
    );

    // Not vacuous, and the reason the fixture seeds two projects: the same
    // replay leaves the other project's rows in place, so "nothing cached" and
    // "the right things evicted" are distinguishable outcomes. These three
    // assertions are also what proves the rows were being written at all — a
    // client that cached no membership would pass every assertion above.
    assert!(
        holds(entity_types::PROJECT, &survivor.project_id),
        "the surviving project must still be cached"
    );
    assert!(
        holds(entity_types::PROJECT_MEMBER, &survivor.member_entity_id),
        "the surviving project's membership must still be cached, or the eviction \
         assertion above would pass against a replay that cached no membership at all"
    );
    assert!(
        holds(entity_types::PROJECT_MILESTONE, &survivor.milestone_id),
        "the surviving project's milestone must still be cached"
    );
    assert!(
        holds(entity_types::PROJECT_UPDATE, &survivor.update_id),
        "the surviving project's posted update must still be cached"
    );

    // The detached issue is the asymmetric half. It still exists, so it must
    // still be cached — but showing no project, which only an update carrying
    // the post-cascade payload can achieve. A delete entry would have removed it
    // outright; an update built before the DELETE would leave it pointing at the
    // project the same replay just evicted.
    let cached_issue = cache
        .get(&(entity_types::ISSUE.to_owned(), detached.issue_id.clone()))
        .unwrap_or_else(|| {
            panic!(
                "the detached issue must still be cached — `ON DELETE SET NULL` \
                 leaves it in the database, so a delete entry would drop a row the \
                 server still has"
            )
        });
    let cached_issue: serde_json::Value = serde_json::from_str(cached_issue)
        .unwrap_or_else(|e| panic!("parsing the cached issue row {cached_issue:?}: {e}"));
    assert_eq!(
        cached_issue.get("project_id"),
        Some(&serde_json::Value::Null),
        "the cached issue must show no project. Its cached row is what the issue \
         list renders from, so a stale project_id here is an issue filed under a \
         project the client has been told to delete."
    );

    assert!(
        holds(entity_types::ISSUE, &untouched.issue_id),
        "the other project's issue must still be cached"
    );

    // The in-memory half of the same replay, which is what the screen renders
    // from. Both issues survive; only the project list loses one.
    let project_ids: Vec<String> = store
        .projects()
        .get()
        .into_iter()
        .map(|p| p.project_id)
        .collect();
    assert_eq!(
        project_ids,
        vec![survivor.project_id.clone()],
        "the reactive store must be left holding only the surviving project"
    );
}
