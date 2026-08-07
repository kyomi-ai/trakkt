// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync protocol tests that need more than one connection.
//!
//! `websocket.rs`'s own `#[cfg(test)]` module drives the handlers through a
//! bare channel, which is the right instrument for everything that happens
//! *inside* one stream — ordering, paging, watermarks, the catch-up flag. It
//! cannot see the property this file exists for: that a sync response reaches
//! the connection that asked for it and no other. That needs a real
//! `WebSocketManager` with several connections registered under one user, which
//! is what every test below builds.
//!
//! The bug this guards against (TRA-9916) was not a crash. A second browser's
//! `SyncComplete` was fanned out to the whole user, so the first browser
//! advanced its cursor past entities it had never been sent and went silently
//! stale. Nothing observable failed at the time — which is why the assertions
//! here are about who did *not* receive a frame.
//!
//! Everything runs against a migrated in-memory SQLite database and an
//! in-process manager: no Postgres, no Redis, no sockets.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use trakkt_auth::websocket::manager::{CatchUpFlag, WebSocketManager, WsSender};
use trakkt_core::test_helpers::channel::recv_soon;
use trakkt_core::test_helpers::{seed_team, seed_user, seed_workspace, test_pool};
use trakkt_core::DbPool;
use trakkt_server::routes::websocket::{handle_sync_bootstrap, handle_sync_delta};
use trakkt_types::sync::{entity_types, SyncActionType, SyncResponse};

const USER: &str = "usr_sync";
const WORKSPACE: &str = "ws_sync";
const TEAM: &str = "team_sync";

/// A second workspace member who is never put in any team.
///
/// Used only by the TEAM live-frame tests below, where the whole property under
/// test is what a full member of the workspace who is *not* in a team does and
/// does not receive.
const OUTSIDER: &str = "usr_workspace_only";

/// Outbound capacity for the hand-built channels used where backpressure is the
/// point of the test. Connections that come from the manager keep the manager's
/// own capacity instead.
const BACKPRESSURED_CAPACITY: usize = 1;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A workspace with enough content that a bootstrap streams a stream, not a
/// single frame.
///
/// The tenancy rows come from `trakkt_core::test_helpers`; everything with a
/// service behind it goes through that service, so the rows are the ones the
/// product would have written — issue numbering, default status resolution and
/// the `sync_log` appends included.
async fn seeded_workspace() -> DbPool {
    let db = test_pool().await.expect("migrated in-memory pool");

    seed_user(&db, USER, "sync@example.test")
        .await
        .expect("seed user");
    seed_workspace(&db, WORKSPACE, USER)
        .await
        .expect("seed workspace");
    seed_team(&db, TEAM, WORKSPACE, "SYN")
        .await
        .expect("seed team");

    // Membership is what makes the team visible to `list_teams(.., Some(user))`,
    // and therefore to bootstrap.
    trakkt_auth::team_service::add_team_member(&db, TEAM, USER, "lead", WORKSPACE)
        .await
        .expect("add the seeded user to the seeded team");

    // Issues carry a `status_id` foreign key, so statuses have to exist first.
    trakkt_auth::status_service::seed_default_statuses(&db, WORKSPACE)
        .await
        .expect("seed default statuses");

    for (name, color) in [("bug", "#ef4444"), ("chore", "#64748b")] {
        trakkt_auth::label_service::create_label(&db, WORKSPACE, name, color, Some(TEAM), None)
            .await
            .expect("create label");
    }

    for title in ["first issue", "second issue", "third issue"] {
        trakkt_auth::issue_service::create_issue(
            &db,
            &trakkt_types::models::CreateIssueParams {
                workspace_id: WORKSPACE.to_string(),
                team_id: TEAM.to_string(),
                creator_id: USER.to_string(),
                title: title.to_string(),
                description: None,
                priority: 0,
                assignee_id: None,
                due_date: None,
                label_ids: vec![],
                project_id: None,
                milestone_id: None,
                estimate: None,
            },
            None,
        )
        .await
        .expect("create issue");
    }

    db
}

/// One registered connection, with the receiver the socket task would normally
/// own so a test can read what the connection was sent.
struct TestConnection {
    tx: WsSender,
    rx: mpsc::Receiver<String>,
    catching_up: CatchUpFlag,
    /// Kept alive for the connection's lifetime. The manager fires this to tear
    /// down a connection that stops draining; holding it means nothing here
    /// depends on whether it was fired, but dropping it would be a lie about
    /// what a real connection owns.
    _kill: trakkt_auth::websocket::manager::KillSignal,
}

/// Register `count` connections for `user_id` and hand back their receivers.
///
/// `connect` fans a heartbeat out to every connection the user already has, so
/// the earlier connections accumulate one per later registration. Those are
/// drained here: a test asserting "this connection received nothing" must start
/// from an empty queue, and the drain is also the first evidence the wiring
/// works at all.
fn register_connections(
    manager: &WebSocketManager,
    user_id: &str,
    count: usize,
) -> Vec<TestConnection> {
    let mut conns: Vec<TestConnection> = (0..count)
        .map(|i| {
            let handle = manager
                .connect(user_id)
                .unwrap_or_else(|e| panic!("register connection {i}: {e}"));
            TestConnection {
                tx: handle.tx,
                rx: handle.rx,
                catching_up: handle.catching_up,
                _kill: handle.kill,
            }
        })
        .collect();

    for (i, conn) in conns.iter_mut().enumerate() {
        let mut heartbeats = 0;
        while conn.rx.try_recv().is_ok() {
            heartbeats += 1;
        }
        assert_eq!(
            heartbeats,
            count - i,
            "connection {i} should have been sent one heartbeat per registration \
             from its own onwards"
        );
    }

    conns
}

/// Send `probe` to every connection the user has and assert each one reads it
/// back, leaving all queues empty.
///
/// This is the guard against a vacuous pass. "Connection B received zero
/// frames" is trivially true of a connection that was never registered, never
/// readable, or already torn down; running this immediately before the
/// assertion proves none of those is the reason.
async fn assert_all_live(
    manager: &WebSocketManager,
    user_id: &str,
    conns: &mut [TestConnection],
    probe: &str,
) {
    manager.send_to_user_raw(user_id, probe).await;

    for (i, conn) in conns.iter_mut().enumerate() {
        assert_eq!(
            conn.rx.try_recv().ok().as_deref(),
            Some(probe),
            "connection {i} is not live: the manager could not reach it"
        );
        assert!(
            conn.rx.try_recv().is_err(),
            "connection {i} had unexpected traffic queued behind the probe"
        );
    }
}

// ---------------------------------------------------------------------------
// Driving a handler
// ---------------------------------------------------------------------------

fn parse_frame(frame: &str) -> SyncResponse {
    serde_json::from_str(frame).expect("frame deserializes as a SyncResponse")
}

/// Run a spawned sync handler to completion, collecting every frame it writes.
///
/// The handlers `await` their sends on a bounded channel, so calling one inline
/// and draining afterwards deadlocks the moment the channel fills. Draining
/// concurrently is the only way to run one — the same shape `websocket.rs`'s own
/// `collect_stream_frames` uses.
///
/// It differs in how it stops. There, the handler owns the only sender and the
/// drain ends when the channel closes; here the manager holds a clone for as
/// long as the connection is registered, so the channel never closes and the
/// handler task finishing is the signal instead. `biased` keeps queued frames
/// ahead of that signal, and the trailing `try_recv` sweep collects anything
/// buffered in the same poll the handler returned on.
async fn run_and_collect(
    mut handler: JoinHandle<()>,
    rx: &mut mpsc::Receiver<String>,
) -> Vec<SyncResponse> {
    let mut frames = Vec::new();

    loop {
        tokio::select! {
            biased;
            frame = rx.recv() => match frame {
                Some(frame) => frames.push(parse_frame(&frame)),
                None => panic!("the manager still holds a sender, so this channel cannot close"),
            },
            outcome = &mut handler => {
                outcome.expect("sync handler task completes without panicking");
                break;
            }
        }
    }

    while let Ok(frame) = rx.try_recv() {
        frames.push(parse_frame(&frame));
    }

    frames
}

/// Spawn `handle_sync_bootstrap` against `conn`'s sender.
fn spawn_bootstrap(db: &DbPool, conn: &TestConnection) -> JoinHandle<()> {
    spawn_bootstrap_in(db, conn, WORKSPACE)
}

/// Spawn `handle_sync_bootstrap` for a named workspace, which is only
/// interesting for workspaces the fixture did *not* create.
fn spawn_bootstrap_in(
    db: &DbPool,
    conn: &TestConnection,
    workspace_id: &'static str,
) -> JoinHandle<()> {
    let tx = conn.tx.clone();
    let flag = Arc::clone(&conn.catching_up);
    let db = db.clone();
    tokio::spawn(async move { handle_sync_bootstrap(&tx, &flag, &db, USER, workspace_id).await })
}

/// The watermarks carried by the `SyncComplete` frames in `frames`.
fn watermarks(frames: &[SyncResponse]) -> Vec<i64> {
    frames
        .iter()
        .filter_map(|f| match f {
            SyncResponse::SyncComplete { last_sync_id } => Some(*last_sync_id),
            _ => None,
        })
        .collect()
}

/// Spawn `handle_sync_delta` against `conn`'s sender.
fn spawn_delta(db: &DbPool, conn: &TestConnection, last_sync_id: i64) -> JoinHandle<()> {
    spawn_delta_for(db, conn, USER, last_sync_id)
}

/// Spawn `handle_sync_delta` for a named user, which is only interesting where
/// two users' deltas of the same workspace are meant to differ.
fn spawn_delta_for(
    db: &DbPool,
    conn: &TestConnection,
    user_id: &'static str,
    last_sync_id: i64,
) -> JoinHandle<()> {
    let tx = conn.tx.clone();
    let flag = Arc::clone(&conn.catching_up);
    let db = db.clone();
    tokio::spawn(async move {
        handle_sync_delta(&tx, &flag, &db, user_id, WORKSPACE, last_sync_id).await
    })
}

/// The entity types carried by the `SyncAction` frames in `frames`.
fn streamed_entity_types(frames: &[SyncResponse]) -> Vec<&str> {
    frames
        .iter()
        .filter_map(|f| match f {
            SyncResponse::SyncAction(action) => Some(action.entity_type.as_str()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// (a) Bootstrap is addressed to one connection
// ---------------------------------------------------------------------------

/// Two browsers, one account. Bootstrapping the first must be invisible to the
/// second — most of all the trailing `SyncComplete`, which the second browser
/// would otherwise adopt as a watermark covering entities it was never sent.
#[tokio::test]
async fn bootstrap_reaches_only_the_connection_that_requested_it() {
    let db = seeded_workspace().await;
    let manager = WebSocketManager::new(None, db.clone());
    let mut conns = register_connections(&manager, USER, 2);

    assert_all_live(&manager, USER, &mut conns, "probe-before-bootstrap").await;

    let (requesting, idle) = conns.split_at_mut(1);
    let requesting = &mut requesting[0];
    let idle = &mut idle[0];

    let frames = run_and_collect(spawn_bootstrap(&db, requesting), &mut requesting.rx).await;

    // The bootstrap really ran. Asserting on which entity types arrived, rather
    // than a frame count, is what keeps this honest: a handler that streamed
    // only the cheap tables would still clear a threshold.
    let streamed = streamed_entity_types(&frames);
    for expected in [
        entity_types::ISSUE,
        entity_types::LABEL,
        entity_types::STATUS,
        entity_types::TEAM,
    ] {
        assert!(
            streamed.contains(&expected),
            "the requesting connection should have received the seeded {expected} rows, \
             got {streamed:?}"
        );
    }
    assert!(
        matches!(frames.last(), Some(SyncResponse::SyncComplete { .. })),
        "a completed bootstrap ends in SyncComplete, got {:?}",
        frames.last()
    );

    // And none of it went to the other connection. Probing again is what makes
    // this assertion mean something: the probe is the *first* thing the idle
    // connection reads, so it was live and empty for the whole bootstrap.
    manager.send_to_user_raw(USER, "probe-after-bootstrap").await;
    assert_eq!(
        idle.rx.try_recv().ok().as_deref(),
        Some("probe-after-bootstrap"),
        "the idle connection received a frame from a bootstrap it never requested"
    );
    assert!(
        idle.rx.try_recv().is_err(),
        "the idle connection received more than the probe"
    );
}

// ---------------------------------------------------------------------------
// (b) SyncReset is addressed to one connection
// ---------------------------------------------------------------------------

/// A cursor the log no longer holds resets the client that presented it. Fanning
/// that reset out would throw away the other connection's perfectly valid
/// cached state and make it re-download the workspace.
#[tokio::test]
async fn a_pruned_cursor_resets_only_the_connection_that_presented_it() {
    let db = seeded_workspace().await;

    // Model pruning by doing what pruning does: write an entry, hand its id out
    // as a cursor, then delete the row. `is_sync_id_available` looks the id up
    // by equality, so a deleted row is indistinguishable from a pruned one.
    let pruned_cursor = trakkt_auth::sync_log_service::write_sync_entry(
        &db,
        entity_types::ISSUE,
        "iss_pruned",
        WORKSPACE,
        trakkt_auth::sync_log_service::SyncAudience::Workspace,
        SyncActionType::Update,
        None,
    )
    .await
    .expect("write the entry that will be pruned");

    trakkt_core::db_execute!(db, "DELETE FROM sync_log WHERE sync_id = $1", pruned_cursor)
        .expect("prune the entry");

    let manager = WebSocketManager::new(None, db.clone());
    let mut conns = register_connections(&manager, USER, 2);

    assert_all_live(&manager, USER, &mut conns, "probe-before-delta").await;

    let (requesting, idle) = conns.split_at_mut(1);
    let requesting = &mut requesting[0];
    let idle = &mut idle[0];

    let frames = run_and_collect(
        spawn_delta(&db, requesting, pruned_cursor),
        &mut requesting.rx,
    )
    .await;

    assert_eq!(
        frames.len(),
        1,
        "a pruned cursor is answered with a reset and nothing else, got {frames:?}"
    );
    assert!(
        matches!(frames[0], SyncResponse::SyncReset),
        "expected SyncReset, got {:?}",
        frames[0]
    );

    manager.send_to_user_raw(USER, "probe-after-delta").await;
    assert_eq!(
        idle.rx.try_recv().ok().as_deref(),
        Some("probe-after-delta"),
        "the idle connection was reset because another connection presented a pruned cursor"
    );
    assert!(
        idle.rx.try_recv().is_err(),
        "the idle connection received more than the probe"
    );
}

// ---------------------------------------------------------------------------
// (c) A bootstrap survives its connection going away
// ---------------------------------------------------------------------------

/// A browser closed mid-load must not take the handler down with it, and must
/// not leave its catch-up exemption behind.
///
/// `websocket.rs` covers the abort path for deltas
/// (`sync_delta_clears_the_catch_up_flag_when_the_stream_aborts_early`,
/// `sync_delta_abandons_a_multi_page_stream_when_the_client_disconnects`) and
/// covers it for `stream_entities` in isolation
/// (`stream_entities_stops_early_when_the_connection_dies`). Neither reaches
/// `handle_sync_bootstrap` itself, and the exemption is the part that matters:
/// a `CatchUpGuard` leaked on the abort path leaves the connection permanently
/// exempt from the slow-consumer kill, which is a live socket the manager can
/// no longer reclaim. That asymmetry — covered for deltas, uncovered for
/// bootstrap — is the gap this closes.
///
/// What this deliberately does *not* assert is that the handler withholds its
/// trailing `SyncComplete`. It does withhold it, but that is unfalsifiable from
/// here: the channel the watermark would travel on is the one that just closed,
/// so a handler that tried to send it anyway would be indistinguishable. The
/// `return` in `stream_batch!` stops the *work*; the dead channel is what stops
/// the *delivery*. Asserting the absence of a frame that could not have arrived
/// either way would read as coverage without being any.
///
/// The channel is hand-built rather than taken from the manager. A registered
/// connection buffers 1024 frames — far more than this workspace produces — so
/// the handler would finish before a test could close anything, and the abort
/// path would never be reached.
#[tokio::test]
async fn a_connection_closing_mid_bootstrap_releases_its_catch_up_exemption() {
    let db = seeded_workspace().await;

    // Baseline: what an undisturbed bootstrap of this workspace delivers.
    let (full_tx, mut full_rx) = mpsc::channel::<String>(1024);
    let full_flag: CatchUpFlag = Arc::new(AtomicBool::new(false));
    let full_db = db.clone();
    let full = tokio::spawn(async move {
        handle_sync_bootstrap(&full_tx, &full_flag, &full_db, USER, WORKSPACE).await
    });
    let mut complete_frames = Vec::new();
    while let Some(frame) = full_rx.recv().await {
        complete_frames.push(parse_frame(&frame));
    }
    full.await.expect("baseline bootstrap task");
    assert!(
        complete_frames.len() > 2,
        "the baseline bootstrap must be long enough to be interrupted, got {}",
        complete_frames.len()
    );

    // Capacity one means the handler is blocked on a send it cannot complete
    // for all but the first frame or two, so closing the receiver lands squarely
    // mid-stream.
    let (conn_tx, mut conn_rx) = mpsc::channel::<String>(BACKPRESSURED_CAPACITY);
    let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));

    let flag = Arc::clone(&catching_up);
    let handler_db = db.clone();
    let handler = tokio::spawn(async move {
        handle_sync_bootstrap(&conn_tx, &flag, &handler_db, USER, WORKSPACE).await
    });

    // Receiving a frame proves the stream is under way; closing then fails every
    // send after it.
    let first = conn_rx.recv().await.expect("first bootstrap frame");
    assert!(
        matches!(parse_frame(&first), SyncResponse::SyncAction(_)),
        "a bootstrap opens with entities, got {:?}",
        parse_frame(&first)
    );
    conn_rx.close();

    // Returning at all is half the point: a handler that panicked on the failed
    // send, or that parked forever on it, fails here.
    handler
        .await
        .expect("the bootstrap returns cleanly when its connection dies");

    assert!(
        !catching_up.load(Ordering::Acquire),
        "aborting mid-bootstrap must still release the catch-up exemption, or the \
         connection stays exempt from the slow-consumer kill for the rest of its life"
    );

    let mut delivered = vec![parse_frame(&first)];
    while let Some(frame) = conn_rx.recv().await {
        delivered.push(parse_frame(&frame));
    }

    // Non-vacuity: the abort has to have landed in the middle of a stream that
    // had more to give, otherwise the assertions above describe a bootstrap that
    // had already finished.
    assert!(
        delivered.len() < complete_frames.len(),
        "the stream must be abandoned mid-flight: delivered all {} frames of the baseline",
        complete_frames.len()
    );
}

// ---------------------------------------------------------------------------
// (d) A bootstrap that cannot read the workspace sends no watermark
// ---------------------------------------------------------------------------

/// A `SyncComplete` is a promise that everything below the watermark has been
/// delivered. A bootstrap whose reads failed cannot make that promise, and the
/// client it lies to is unrecoverable without deleting its IndexedDB by hand:
/// the rows it never received sit below its stored cursor, so no delta will
/// ever mention them again.
///
/// The failure is a real one — the table one of the eleven reads selects from
/// is taken out of service, so the query errors the way a transient database
/// fault would. Nothing in the handler knows this is a test.
///
/// `favorites` is the table because exactly one bootstrap read touches it.
/// `issues` would tell the more dramatic story, but four of the reads join
/// through it (`list_issues`, `list_teams`, `list_notifications`,
/// `list_comments_for_workspace`), so a bootstrap that aborted after only
/// three of them were fixed would still pass — the failure has to be
/// attributable to a single read for this test to be able to catch a partial
/// fix. Nothing has a foreign key into `favorites`, so it can simply be
/// dropped.
#[tokio::test]
async fn a_failed_entity_read_ends_the_bootstrap_without_a_watermark() {
    let db = seeded_workspace().await;
    let manager = WebSocketManager::new(None, db.clone());
    let mut conns = register_connections(&manager, USER, 1);
    let conn = &mut conns[0];

    // Control: this workspace, this connection and this harness produce a
    // completed bootstrap. Without it, "no frames arrived" is a claim any
    // amount of broken wiring would also satisfy.
    let healthy = run_and_collect(spawn_bootstrap(&db, conn), &mut conn.rx).await;
    assert!(
        matches!(healthy.last(), Some(SyncResponse::SyncComplete { .. })),
        "the control bootstrap must complete, got {:?}",
        healthy.last()
    );
    assert!(
        streamed_entity_types(&healthy).contains(&entity_types::ISSUE),
        "the control bootstrap must stream the seeded issues, got {:?}",
        streamed_entity_types(&healthy)
    );

    trakkt_core::db_execute!(db, "DROP TABLE favorites")
        .expect("take the favorites table out of service");

    let frames = run_and_collect(spawn_bootstrap(&db, conn), &mut conn.rx).await;

    assert!(
        watermarks(&frames).is_empty(),
        "a bootstrap that could not read the workspace must not hand out a \
         watermark, got {frames:?}"
    );
    // The reads all happen before the first frame goes out, so a failed one
    // costs the client nothing to discard: it receives no stream at all.
    assert!(
        frames.is_empty(),
        "a bootstrap that could not read the workspace should stream nothing, \
         got {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// (e) An intact bootstrap still ends with exactly one watermark
// ---------------------------------------------------------------------------

/// The other half of (d): withholding the watermark on failure is only correct
/// if it is still sent, once, on success — and if it is the workspace's real
/// `latest_sync_id` rather than a placeholder.
#[tokio::test]
async fn an_intact_bootstrap_ends_with_exactly_one_watermark() {
    let db = seeded_workspace().await;
    let expected = trakkt_auth::sync_log_service::get_latest_sync_id(&db, WORKSPACE)
        .await
        .expect("read the workspace watermark");
    assert!(
        expected > 0,
        "the fixture writes sync_log entries, so its watermark cannot be the \
         zero a failed read would produce"
    );

    let manager = WebSocketManager::new(None, db.clone());
    let mut conns = register_connections(&manager, USER, 1);
    let conn = &mut conns[0];

    let frames = run_and_collect(spawn_bootstrap(&db, conn), &mut conn.rx).await;

    assert_eq!(
        watermarks(&frames),
        vec![expected],
        "a completed bootstrap sends the workspace's watermark exactly once, \
         got {frames:?}"
    );
    assert!(
        matches!(frames.last(), Some(SyncResponse::SyncComplete { .. })),
        "the watermark closes the stream, got {:?}",
        frames.last()
    );
    // The seeded workspace has a settings row, so it is streamed. This is the
    // contrast the next test needs: an absent snapshot there is an absent row,
    // not an absent code path.
    assert!(
        streamed_entity_types(&frames).contains(&entity_types::WORKSPACE_SETTINGS),
        "a workspace with a row streams its settings snapshot, got {:?}",
        streamed_entity_types(&frames)
    );
}

// ---------------------------------------------------------------------------
// (f) A workspace with no settings row is not a failure
// ---------------------------------------------------------------------------

/// "No settings row" and "the settings read failed" used to be the same value.
/// Now that the second aborts the bootstrap, the first must still complete —
/// otherwise a workspace that simply has no row would be permanently
/// un-bootstrappable.
#[tokio::test]
async fn a_workspace_with_no_settings_row_still_completes() {
    const ABSENT: &str = "ws_never_created";

    let db = seeded_workspace().await;

    // The premise, stated against the service rather than assumed: reading an
    // absent workspace is an answer, not an error.
    let snapshot = trakkt_auth::workspace_service::get_workspace_settings_for_sync(&db, ABSENT)
        .await
        .expect("reading a workspace that does not exist is not a failure");
    assert!(
        snapshot.is_none(),
        "a workspace with no row has no snapshot, got {snapshot:?}"
    );

    let manager = WebSocketManager::new(None, db.clone());
    let mut conns = register_connections(&manager, USER, 1);
    let conn = &mut conns[0];

    let frames = run_and_collect(spawn_bootstrap_in(&db, conn, ABSENT), &mut conn.rx).await;

    assert!(
        !streamed_entity_types(&frames).contains(&entity_types::WORKSPACE_SETTINGS),
        "there is no settings row to stream, got {:?}",
        streamed_entity_types(&frames)
    );
    assert_eq!(
        watermarks(&frames),
        vec![0],
        "an empty workspace still completes, at the watermark it has, got {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// (g) Unreadable workspace settings are a failure, not an absence
// ---------------------------------------------------------------------------

/// `get_workspace_settings_for_sync` has two ways to come back empty-handed,
/// and they used to be the same value: no row, and a `settings` column that is
/// not JSON. Only the first is an answer. Streaming the second as "this
/// workspace has no settings" and then certifying it with a watermark is how a
/// client ends up permanently believing a configured workspace is unconfigured.
///
/// The column is written directly here because no service can produce this row:
/// `update_workspace_settings` serializes a `serde_json::Value`, so everything
/// it writes parses. A row like this comes from outside the service layer —
/// hand-editing, a bad restore, a migration — which is exactly the case a
/// bootstrap has to survive without lying.
#[tokio::test]
async fn unparseable_workspace_settings_end_the_bootstrap_without_a_watermark() {
    let db = seeded_workspace().await;
    let manager = WebSocketManager::new(None, db.clone());
    let mut conns = register_connections(&manager, USER, 1);
    let conn = &mut conns[0];

    // Control: the same workspace, before the column is corrupted.
    let healthy = run_and_collect(spawn_bootstrap(&db, conn), &mut conn.rx).await;
    assert!(
        matches!(healthy.last(), Some(SyncResponse::SyncComplete { .. })),
        "the control bootstrap must complete, got {:?}",
        healthy.last()
    );

    trakkt_core::db_execute!(
        db,
        "UPDATE workspaces SET settings = $1 WHERE workspace_id = $2",
        "{ not json",
        WORKSPACE
    )
    .expect("write a settings column that cannot be parsed");

    // The service reports it as a failure rather than as an absent snapshot.
    assert!(
        trakkt_auth::workspace_service::get_workspace_settings_for_sync(&db, WORKSPACE)
            .await
            .is_err(),
        "settings that cannot be parsed are unknown, not absent"
    );

    let frames = run_and_collect(spawn_bootstrap(&db, conn), &mut conn.rx).await;

    assert!(
        watermarks(&frames).is_empty(),
        "a bootstrap that could not read the workspace's settings must not hand \
         out a watermark, got {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// (h) A team's live frame reaches its members and nobody else (TRA-10039)
// ---------------------------------------------------------------------------
//
// TRA-10013 gave `sync_log_service::ENTRIES_SINCE_SQL` a `team_members`
// predicate, so a TEAM insert or update replays only to that team's current
// members. The live frame was left behind: `create_team` and
// `commit_team_update` hand-rolled a broadcast that resolved recipients from
// `workspace_users`, so a connected non-member was handed a private team's
// name, key, icon and settings the moment it was created or renamed.
//
// The fix is `SyncAudience::Team`, whose delivery arm resolves `team_members` —
// the same table, the same set. These tests exercise that through the real
// `WebSocketManager` and the real services; nothing here is a mock, and the
// frames are read off the same channels a socket task would own.

/// A workspace with two members and no teams.
///
/// `USER` creates the teams below and is therefore their only member;
/// `OUTSIDER` is a full `workspace_users` row — the recipient set the old
/// broadcast resolved — and is never added to any team.
///
/// `seeded_workspace` is the wrong fixture here: it seeds a team `USER` is
/// already in, and every property below is about a team that has exactly one
/// member and one non-member. The teams are created by `create_team` itself,
/// which is one of the two writers on trial.
async fn workspace_with_an_outsider() -> DbPool {
    let db = test_pool().await.expect("migrated in-memory pool");

    seed_user(&db, USER, "sync@example.test")
        .await
        .expect("seed the user who will own the teams");
    seed_user(&db, OUTSIDER, "outsider@example.test")
        .await
        .expect("seed the workspace member who joins no team");
    seed_workspace(&db, WORKSPACE, USER)
        .await
        .expect("seed workspace");
    trakkt_auth::workspace_service::create_workspace_user(&db, WORKSPACE, OUTSIDER, "member")
        .await
        .expect("enrol the outsider in the workspace");

    db
}

/// Create a team through the real service, with `USER` as its sole member and
/// the manager wired up so the live frame is actually delivered.
async fn create_team_owned_by_user(
    db: &DbPool,
    manager: &WebSocketManager,
    name: &str,
    key: &str,
) -> String {
    trakkt_auth::team_service::create_team(
        db,
        &trakkt_auth::team_service::CreateTeamParams {
            workspace_id: WORKSPACE,
            name,
            key,
            description: None,
            icon: None,
            creator_id: Some(USER),
        },
        Some(manager),
    )
    .await
    .unwrap_or_else(|e| panic!("create the team {name} that only USER belongs to: {e}"))
    .team_id
}

/// Everything already queued on `conn`, with a probe proving the queue was read
/// to its end on a connection that was genuinely reachable.
///
/// This is what stops the negative assertions below from passing vacuously.
/// "No frame arrived" is equally true of a connection that was never
/// registered, was torn down, or sits behind a delivery path that is broken for
/// everyone — and a test that only ever asserts an absence cannot tell those
/// apart from the fix working. The probe travels the same
/// `WebSocketManager::deliver` path the frames under test travel, so reading it
/// back rules all of them out, and it is read *after* whatever else is queued,
/// so nothing can still be in flight behind it.
///
/// Every read is `recv_soon`, so a probe that never arrives fails this test
/// naming the connection instead of hanging the suite.
async fn drain_live_frames(
    manager: &WebSocketManager,
    user_id: &str,
    conn: &mut TestConnection,
    probe: &str,
) -> Vec<SyncResponse> {
    manager.send_to_user_raw(user_id, probe).await;

    let waiting_for = format!("the {probe:?} probe closing {user_id}'s live queue");
    let mut frames = Vec::new();
    loop {
        let frame = recv_soon(&mut conn.rx, &waiting_for).await;
        if frame == probe {
            return frames;
        }
        frames.push(parse_frame(&frame));
    }
}

/// One TEAM frame, reduced to everything the live path and the delta path have
/// to agree on.
///
/// The `sync_id` is included deliberately: it ties a live frame to the exact
/// `sync_log` row that replays it, so this is an assertion about one row
/// reaching one user two ways, not about two similar-looking frames. The
/// timestamp is excluded because it legitimately differs — the live frame
/// stamps `Utc::now()`, the replay carries the row's `created_at`.
type TeamFrame = (i64, String, SyncActionType, Option<serde_json::Value>);

/// The TEAM frames in `frames`, in arrival order.
fn team_frames(frames: &[SyncResponse]) -> Vec<TeamFrame> {
    frames
        .iter()
        .filter_map(|frame| match frame {
            SyncResponse::SyncAction(action) if action.entity_type == entity_types::TEAM => Some((
                action.sync_id,
                action.entity_id.clone(),
                action.action.clone(),
                action.data.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// The `(entity_id, action, carries a payload)` shape of `frames`, which is
/// what a failure message can be read at a glance.
fn team_frame_shapes(frames: &[TeamFrame]) -> Vec<(&str, SyncActionType, bool)> {
    frames
        .iter()
        .map(|(_, entity_id, action, data)| (entity_id.as_str(), action.clone(), data.is_some()))
        .collect()
}

/// The team name carried by a TEAM frame's payload, or `None` where it carries
/// no payload.
fn team_name(frame: &TeamFrame) -> Option<&str> {
    frame.3.as_ref()?.get("name")?.as_str()
}

/// A team a private team's name must never appear in the frames of.
const PRIVATE_NAME: &str = "Members Only";

/// The reported bug, on create. A connected workspace member who is not in the
/// team must not be handed its name, key, icon or settings.
///
/// The member's half is asserted in the same test, against the same call, so
/// neither half can be satisfied by a broadcast that simply stopped working:
/// one frame set has to be empty while the other is not.
#[tokio::test]
async fn creating_a_team_delivers_its_live_frame_to_members_only() {
    let db = workspace_with_an_outsider().await;
    let manager = WebSocketManager::new(None, db.clone());
    let mut member = register_connections(&manager, USER, 1);
    let mut outsider = register_connections(&manager, OUTSIDER, 1);

    let team_id = create_team_owned_by_user(&db, &manager, PRIVATE_NAME, "MON").await;

    let member_frames =
        drain_live_frames(&manager, USER, &mut member[0], "probe-member-after-create").await;
    let outsider_frames = drain_live_frames(
        &manager,
        OUTSIDER,
        &mut outsider[0],
        "probe-outsider-after-create",
    )
    .await;

    let delivered = team_frames(&member_frames);
    assert_eq!(
        team_frame_shapes(&delivered),
        vec![
            (team_id.as_str(), SyncActionType::Insert, true),
            (team_id.as_str(), SyncActionType::Update, true),
        ],
        "the creator is the team's only member, so both entries create_team \
         writes have to reach them live and carry the payload the client's \
         upsert_team arm needs; got {member_frames:?}"
    );
    assert!(
        delivered
            .iter()
            .all(|frame| team_name(frame) == Some(PRIVATE_NAME)),
        "the frames the member did receive must carry the real team, or the \
         assertion below is about a payload nobody was ever going to get; \
         got {delivered:?}"
    );

    assert_eq!(
        team_frames(&outsider_frames),
        Vec::<TeamFrame>::new(),
        "a workspace member who is not in the team must receive no TEAM frame \
         for it; got {outsider_frames:?}"
    );
    assert!(
        outsider_frames.is_empty(),
        "create_team writes nothing but TEAM entries, so the outsider's queue \
         should hold nothing at all; got {outsider_frames:?}"
    );
}

/// The same disclosure on update. Every single-statement team mutation — rename,
/// key change, icon set/upload/delete, `update_team_settings` — ends in
/// `commit_team_update`, so pinning the rename pins all of them.
#[tokio::test]
async fn renaming_a_team_delivers_its_live_frame_to_members_only() {
    let db = workspace_with_an_outsider().await;
    let manager = WebSocketManager::new(None, db.clone());
    let team_id = create_team_owned_by_user(&db, &manager, "Before", "MON").await;

    let mut member = register_connections(&manager, USER, 1);
    let mut outsider = register_connections(&manager, OUTSIDER, 1);

    trakkt_auth::team_service::update_team(
        &db,
        &team_id,
        WORKSPACE,
        Some(PRIVATE_NAME.to_owned()),
        None,
        Some(&manager),
    )
    .await
    .expect("rename the team the outsider is not a member of");

    let member_frames =
        drain_live_frames(&manager, USER, &mut member[0], "probe-member-after-rename").await;
    let outsider_frames = drain_live_frames(
        &manager,
        OUTSIDER,
        &mut outsider[0],
        "probe-outsider-after-rename",
    )
    .await;

    let delivered = team_frames(&member_frames);
    assert_eq!(
        team_frame_shapes(&delivered),
        vec![(team_id.as_str(), SyncActionType::Update, true)],
        "the member has to be told about the rename live; got {member_frames:?}"
    );
    assert_eq!(
        delivered.first().and_then(team_name),
        Some(PRIVATE_NAME),
        "and the frame has to carry the new name — the thing the outsider must \
         not be shown; got {delivered:?}"
    );

    assert_eq!(
        team_frames(&outsider_frames),
        Vec::<TeamFrame>::new(),
        "a rename must not disclose the team to a non-member; got {outsider_frames:?}"
    );
    assert!(
        outsider_frames.is_empty(),
        "an update_team writes nothing but its TEAM entry; got {outsider_frames:?}"
    );
}

/// The trap on the other side of the fix, and the one thing here that is easiest
/// to break by "tidying up": a TEAM `Delete` deliberately reaches *everyone*.
///
/// `delete_team` writes its entry after the `DELETE FROM teams` it describes,
/// and `team_members` declares `ON DELETE CASCADE` on `teams(team_id)` — so by
/// delivery time there is no membership row left to resolve. Narrowing this
/// broadcast to the team's members would deliver it to nobody and leave the
/// deleted team in every remaining member's cache permanently.
/// `ENTRIES_SINCE_SQL` exempts `action = 'delete'` from its membership
/// predicate for the same reason, so the workspace-wide audience here is what
/// keeps the two paths agreeing.
///
/// Reaching a non-member is safe because the frame carries a `None` payload:
/// a UUID and nothing about the team. That is asserted rather than assumed.
#[tokio::test]
async fn deleting_a_team_reaches_a_non_member_live() {
    let db = workspace_with_an_outsider().await;
    let manager = WebSocketManager::new(None, db.clone());

    // Two teams: a workspace's last team cannot be deleted.
    let doomed = create_team_owned_by_user(&db, &manager, PRIVATE_NAME, "MON").await;
    create_team_owned_by_user(&db, &manager, "Survivor", "SUR").await;

    let mut member = register_connections(&manager, USER, 1);
    let mut outsider = register_connections(&manager, OUTSIDER, 1);

    trakkt_auth::team_service::delete_team(&db, &doomed, WORKSPACE, None, None, Some(&manager))
        .await
        .expect("delete the team");

    let member_frames =
        drain_live_frames(&manager, USER, &mut member[0], "probe-member-after-delete").await;
    let outsider_frames = drain_live_frames(
        &manager,
        OUTSIDER,
        &mut outsider[0],
        "probe-outsider-after-delete",
    )
    .await;

    let expected = vec![(doomed.as_str(), SyncActionType::Delete, false)];
    assert_eq!(
        team_frame_shapes(&team_frames(&member_frames)),
        expected,
        "the member holds the team, so the delete that evicts it has to reach \
         them; got {member_frames:?}"
    );
    assert_eq!(
        team_frame_shapes(&team_frames(&outsider_frames)),
        expected,
        "the delete has to reach a non-member too: the membership rows that \
         would authorise it cascaded away with the team, so a member-scoped \
         delivery would reach nobody and the team would never leave any cache; \
         got {outsider_frames:?}"
    );
}

/// The invariant the three tests above are instances of: for one team, over its
/// whole life, the set of users a live frame reaches is the set of users whose
/// delta replays that same `sync_log` row.
///
/// Compared by `sync_id` and payload, not by shape, so this is an assertion
/// about one row reaching one user twice — a live frame that agreed only in
/// action and entity id would not satisfy it. Both users are checked after
/// every step, so a live path narrowed too far fails here just as loudly as one
/// left too wide, and the delete's exemption is covered by the same comparison
/// rather than by a rule written twice.
///
/// The delta half runs through `handle_sync_delta`, the production handler,
/// from the watermark that handler last handed the client — so the cursor
/// arithmetic is the client's, not the test's.
#[tokio::test]
async fn the_live_and_delta_team_audiences_agree_through_a_teams_whole_life() {
    let db = workspace_with_an_outsider().await;
    let manager = WebSocketManager::new(None, db.clone());
    let mut member = register_connections(&manager, USER, 1);
    let mut outsider = register_connections(&manager, OUTSIDER, 1);

    // Both users start caught up on an empty log, which is what a client holds
    // straight after a bootstrap of a workspace with nothing in it.
    let mut member_cursor = 0i64;
    let mut outsider_cursor = 0i64;

    /// Drain what the live path delivered to both users, then replay the same
    /// range through the delta handler, and assert each user's two sets match.
    ///
    /// Returns the number of rows the member saw, so the caller can refuse to
    /// pass on a step that delivered nothing to anyone.
    async fn assert_agree(
        db: &DbPool,
        manager: &WebSocketManager,
        member: &mut TestConnection,
        member_cursor: &mut i64,
        outsider: &mut TestConnection,
        outsider_cursor: &mut i64,
        step: &str,
    ) -> usize {
        let member_live = team_frames(
            &drain_live_frames(manager, USER, member, &format!("probe-member-{step}")).await,
        );
        let outsider_live = team_frames(
            &drain_live_frames(
                manager,
                OUTSIDER,
                outsider,
                &format!("probe-outsider-{step}"),
            )
            .await,
        );

        for (user_id, conn, cursor, live) in [
            (USER, member, member_cursor, &member_live),
            (OUTSIDER, outsider, outsider_cursor, &outsider_live),
        ] {
            let frames =
                run_and_collect(spawn_delta_for(db, conn, user_id, *cursor), &mut conn.rx).await;
            assert_eq!(
                team_frames(&frames),
                *live,
                "{step}: what the live path delivered to {user_id} and what \
                 their delta from {cursor} replays must be the same rows"
            );

            let watermark = watermarks(&frames);
            assert_eq!(
                watermark.len(),
                1,
                "{step}: {user_id}'s delta must end in exactly one watermark, \
                 or the cursor this test carries forward is invented; \
                 got {frames:?}"
            );
            *cursor = watermark[0];
        }

        member_live.len()
    }

    let team_id = create_team_owned_by_user(&db, &manager, PRIVATE_NAME, "MON").await;
    let survivor = create_team_owned_by_user(&db, &manager, "Survivor", "SUR").await;
    let seen = assert_agree(
        &db,
        &manager,
        &mut member[0],
        &mut member_cursor,
        &mut outsider[0],
        &mut outsider_cursor,
        "after-two-creates",
    )
    .await;
    assert_eq!(
        seen, 4,
        "each create writes an Insert and a member-add Update, so the member \
         must have seen four TEAM rows — an agreement between two empty sets \
         would otherwise satisfy every assertion in this test"
    );

    trakkt_auth::team_service::update_team(
        &db,
        &team_id,
        WORKSPACE,
        Some("Renamed".to_owned()),
        None,
        Some(&manager),
    )
    .await
    .expect("rename the team");
    let seen = assert_agree(
        &db,
        &manager,
        &mut member[0],
        &mut member_cursor,
        &mut outsider[0],
        &mut outsider_cursor,
        "after-rename",
    )
    .await;
    assert_eq!(seen, 1, "a rename writes exactly one TEAM row");

    trakkt_auth::team_service::update_team_settings(
        &db,
        &team_id,
        WORKSPACE,
        &trakkt_types::models::TeamSettings {
            auto_archive_days: Some(30),
            ..Default::default()
        },
        Some(&manager),
    )
    .await
    .expect("change the team's settings");
    let seen = assert_agree(
        &db,
        &manager,
        &mut member[0],
        &mut member_cursor,
        &mut outsider[0],
        &mut outsider_cursor,
        "after-settings",
    )
    .await;
    assert_eq!(seen, 1, "a settings change writes exactly one TEAM row");

    trakkt_auth::team_service::delete_team(&db, &team_id, WORKSPACE, None, None, Some(&manager))
        .await
        .expect("delete the team");
    let seen = assert_agree(
        &db,
        &manager,
        &mut member[0],
        &mut member_cursor,
        &mut outsider[0],
        &mut outsider_cursor,
        "after-delete",
    )
    .await;
    assert_eq!(seen, 1, "a delete writes exactly one TEAM row");

    // The end state, stated against the real bootstrap read: the outsider holds
    // nothing, and the member holds the team that was not deleted. Without this
    // the test above could be satisfied by a delta path that agreed with a live
    // path which had itself stopped delivering to anyone.
    let outsider_teams: Vec<String> =
        trakkt_auth::team_service::list_teams(&db, WORKSPACE, Some(OUTSIDER))
            .await
            .expect("read the team set a bootstrap would stream the outsider")
            .into_iter()
            .map(|team| team.team_id)
            .collect();
    assert_eq!(
        outsider_teams,
        Vec::<String>::new(),
        "the outsider never joined a team, so a re-bootstrap gives them none"
    );
    let member_teams: Vec<String> =
        trakkt_auth::team_service::list_teams(&db, WORKSPACE, Some(USER))
            .await
            .expect("read the team set a bootstrap would stream the member")
            .into_iter()
            .map(|team| team.team_id)
            .collect();
    assert_eq!(
        member_teams,
        vec![survivor],
        "the member created two teams and deleted one, so a re-bootstrap gives \
         them the survivor"
    );
}
