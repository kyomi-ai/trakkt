// SPDX-License-Identifier: AGPL-3.0-or-later

//! A bounded receive for tests that wait on a delivery channel.
//!
//! # Why a bound
//!
//! A test that awaits a frame is asserting that the frame gets sent. A bare
//! `rx.recv().await` does not report that assertion failing: on a channel
//! nothing will ever write to it blocks until the process is killed, so the
//! test hangs instead of failing. CI then reports only that the job ran out of
//! time, naming no test and no assertion. TRA-9974 confirmed this against the
//! real case rather than in the abstract — with the `sync_log` write removed
//! from `notification_service::mark_as_read`, the unbounded form left those
//! tests running indefinitely instead of failing.
//!
//! The regression these receives exist to catch produces exactly that shape. A
//! delivery moved back inside an open transaction reaches
//! `WebSocketManager::broadcast_raw_to_workspace`, which resolves its
//! recipients with a `db_fetch_all!` on the pool; SQLite runs at
//! `max_connections(1)` (see [`crate::db::DbPool::connect`]), so the open
//! transaction holds the only connection and that query waits out sqlx's
//! acquire timeout. The error is logged and swallowed — live delivery is
//! best-effort — and the function returns having queued nothing. Nothing on the
//! calling side fails, and no frame is ever sent, so a waiting test is left
//! blocked on a channel that has gone permanently quiet.
//!
//! # Why ten seconds
//!
//! [`RECEIVE_BOUND`] is a deadlock guard, not a budget for a slow arrival, and
//! on a passing run it costs nothing measurable. A test awaits the call that
//! produces the frame before it awaits the frame, so by the time it reaches its
//! receive the message is already queued and `recv()` returns without yielding
//! — the workspace broadcast path makes this structural, since
//! `WebSocketManager::deliver_to_local_user` enqueues with `try_send` and never
//! awaits at all.
//!
//! The bound is therefore only ever consumed by a message that is not coming.
//! Its floor is how long a loaded CI runner could plausibly leave a ready task
//! unscheduled, which ten seconds exceeds by orders of magnitude, so it cannot
//! flake. Its ceiling is what one genuinely stuck receive costs the suite: ten
//! seconds, against the whole-job timeout that is the alternative. TRA-9974
//! picked this value for the three notification-sync receives and nothing about
//! the other sites argues for a different one — every caller is in-process,
//! against in-memory SQLite, over a local `mpsc`.

use std::time::Duration;

use tokio::sync::mpsc;

/// How long [`recv_soon`] waits before declaring the message will never arrive.
///
/// See the module docs for why this value, and why a passing test never spends
/// any of it.
pub const RECEIVE_BOUND: Duration = Duration::from_secs(10);

/// Receive the next message, or panic naming what was being waited for.
///
/// Replaces `rx.recv().await.expect(…)`, which reports a closed channel but
/// waits forever on an open one nobody will write to. Both of those are test
/// failures and both are reported here.
///
/// `what` names the delivery under test, not the channel — it is read by
/// someone looking at a failure with no other context, so "the read state
/// reaching the second tab" earns its place where "a frame" does not.
pub async fn recv_soon<T>(rx: &mut mpsc::Receiver<T>, what: &str) -> T {
    match tokio::time::timeout(RECEIVE_BOUND, rx.recv()).await {
        Ok(Some(message)) => message,
        Ok(None) => panic!(
            "the channel closed before {what} arrived: every sender was dropped, \
             so it never will"
        ),
        Err(_) => panic!(
            "nothing arrived within {RECEIVE_BOUND:?} while waiting for {what}: \
             the sender is still open, so this is a delivery that never happened"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_queued_message_is_returned() {
        let (tx, mut rx) = mpsc::channel::<&str>(1);
        tx.send("frame").await.expect("queue one message");

        assert_eq!(recv_soon(&mut rx, "the queued message").await, "frame");
    }

    #[tokio::test(start_paused = true)]
    async fn a_message_that_never_arrives_panics_naming_it() {
        // The sender is held open for the whole call, so `recv()` is blocked on
        // "nothing sent yet" rather than on closure — the deadlock case.
        let (_tx, mut rx) = mpsc::channel::<&str>(1);

        let panic = tokio::spawn(async move {
            recv_soon(&mut rx, "a frame no one sends").await;
        })
        .await
        .expect_err("waiting for a message that is never sent must panic");

        let message = panic
            .into_panic()
            .downcast::<String>()
            .expect("the panic payload is the formatted message");
        assert!(
            message.contains("a frame no one sends"),
            "the panic must name what was awaited, got: {message}"
        );
    }

    #[tokio::test]
    async fn a_closed_channel_panics_immediately() {
        let (tx, mut rx) = mpsc::channel::<&str>(1);
        drop(tx);

        let panic = tokio::spawn(async move {
            recv_soon(&mut rx, "a frame from a dropped sender").await;
        })
        .await
        .expect_err("receiving from a closed channel must panic");

        let message = panic
            .into_panic()
            .downcast::<String>()
            .expect("the panic payload is the formatted message");
        assert!(
            message.contains("the channel closed"),
            "a closed channel must be distinguished from a timeout, got: {message}"
        );
    }
}
