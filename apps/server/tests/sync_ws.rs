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
use trakkt_core::test_helpers::{seed_team, seed_user, seed_workspace, test_pool};
use trakkt_core::DbPool;
use trakkt_server::routes::websocket::{handle_sync_bootstrap, handle_sync_delta};
use trakkt_types::sync::{entity_types, SyncActionType, SyncResponse};

const USER: &str = "usr_sync";
const WORKSPACE: &str = "ws_sync";
const TEAM: &str = "team_sync";

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
    let tx = conn.tx.clone();
    let flag = Arc::clone(&conn.catching_up);
    let db = db.clone();
    tokio::spawn(async move { handle_sync_bootstrap(&tx, &flag, &db, USER, WORKSPACE).await })
}

/// Spawn `handle_sync_delta` against `conn`'s sender.
fn spawn_delta(db: &DbPool, conn: &TestConnection, last_sync_id: i64) -> JoinHandle<()> {
    let tx = conn.tx.clone();
    let flag = Arc::clone(&conn.catching_up);
    let db = db.clone();
    tokio::spawn(
        async move { handle_sync_delta(&tx, &flag, &db, USER, WORKSPACE, last_sync_id).await },
    )
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
        None,
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
