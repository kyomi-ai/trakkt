// SPDX-License-Identifier: AGPL-3.0-or-later

//! A bounded join for tests that wait on a spawned task.
//!
//! # Why this is not in `channel`
//!
//! [`recv_soon`](super::channel::recv_soon) is the receive-side half of the
//! same idea and this is the send-side half, so the two belong next to each
//! other — but not in one module. `channel` is scoped to a single type: every
//! item in it takes an `mpsc::Receiver` and bounds the wait for something to
//! arrive on it. [`join_soon`] takes a [`JoinHandle`] and names no channel type
//! at all, and the park it reports need not involve a channel: a task waiting
//! on the SQLite pool's only connection (`max_connections(1)`, see
//! [`crate::db::DbPool::connect`]) is joined just as indefinitely as one parked
//! on a send. Putting it in `channel` would make that module's name wrong about
//! its own contents, and would leave a caller that only wants the join
//! importing a channel module to get it.
//!
//! # Why a bound
//!
//! A test that joins a spawned task is asserting that the task returns. A bare
//! `handle.await` does not report that assertion failing: a task parked on a
//! bounded `send` that nobody will drain never completes, so the test hangs
//! instead of failing. What survives is less than nothing useful, but it is
//! not nothing: libtest does name the hung test after a minute
//! (`test … has been running for over 60 seconds`), so which test is stuck is
//! recoverable from the log. What is never reported is the *assertion* — no
//! message, no line, no statement of what was expected, and nothing to
//! separate a task parked forever from one that is merely slow. The hang is at
//! least bounded in CI — every
//! job in `.github/workflows/` carries a `timeout-minutes`, all sixteen of them
//! (8 in `ci.yml`, 4 in `release.yml`, 2 in `docs.yml`, 1 each in
//! `claude-code-review.yml` and `realtime-e2e.yml`) — but the whole budget is
//! spent before the report arrives, and the report points at infrastructure.
//!
//! TRA-10007 is the confirmed case, not a hypothetical one. Its test —
//! `sync_bootstrap_flags_the_connection_for_the_whole_stream` in
//! `apps/server/src/routes/websocket.rs` — drives `handle_sync_bootstrap`
//! through a capacity-1 channel it deliberately keeps full, which is how it
//! catches the handler mid-flight. TRA-9960 mutated `stream_bootstrap` to send
//! its `SyncComplete` twice: the second send found no slot, the test had
//! already stopped draining, and the join never returned. The whole
//! `cargo test` run had to be killed by hand. Reproduced again while writing
//! this module, against the test as it then stood: `cargo test` printed
//! `running 1 test`, then the sixty-second notice naming that test, then
//! nothing at all. A `timeout 180s` wrapper is what ended it, exit 124. No
//! assertion was ever reported, which is the whole of the loss.
//!
//! A bounded join is the backstop, not the diagnosis. A test that knows how
//! many frames it expects should drain to close and assert the count, which
//! names the defect outright; [`join_soon`] is what covers the parks it did not
//! anticipate.
//!
//! # Why ten seconds
//!
//! [`JOIN_BOUND`] matches [`RECEIVE_BOUND`](super::channel::RECEIVE_BOUND), and
//! the ceiling argument carries over unchanged from that module's docs: what a
//! bound costs is what one genuinely stuck wait costs the suite — ten seconds,
//! against the whole-job timeout that is the alternative.
//!
//! The floor does not carry over, and this is the part worth reading before
//! reusing the number somewhere else. `recv_soon`'s floor is scheduling
//! latency, because a passing run spends none of the bound: the message it
//! waits for is already queued by the time it is called. A join is often
//! reached the same way — after the drain, the close or the dropped baton that
//! lets the task finish — but several callers deliberately join *concurrently*
//! with the drain that releases the task, because sequentially a stuck drain
//! keeps the join from ever being reached. At those the bound has to cover the
//! task's whole remaining run, not its last poll.
//!
//! So the number that matters is the longest of those runs, and it was
//! measured rather than assumed. The largest is `collect_stream_frames` in
//! `apps/server/src/routes/websocket.rs` streaming a 12,000-entry delta:
//! **738ms for 12,001 frames** with the whole 32-test `routes::websocket` suite
//! running in parallel around it (603ms for the other 12,000-entry test; every
//! remaining call in that run was under 4ms). Ten seconds against 738ms is
//! roughly 13x headroom on that hardware, which absorbs a CI runner several
//! times slower than a dev box and still fails long before the job timeout.
//! Nothing here touches the network, Postgres or a real socket, so there is no
//! source of variance that scales differently from the work itself.

use std::time::Duration;

use tokio::task::JoinHandle;

/// How long [`join_soon`] waits before declaring the task will never finish.
///
/// See the module docs for why this value, and for the measurement of the
/// longest task any caller currently joins.
pub const JOIN_BOUND: Duration = Duration::from_secs(10);

/// Join a spawned task, or panic naming the work it failed to finish.
///
/// Replaces `handle.await.expect(…)`, which reports a task that panicked but
/// waits forever on one that is parked. Both of those are test failures and
/// both are reported here.
///
/// A task that panicked is re-raised with [`std::panic::resume_unwind`] rather
/// than reported through its [`JoinError`](tokio::task::JoinError), so the
/// assertion the task actually failed reaches the test output instead of being
/// flattened into "task panicked". Cancellation is reported separately: a task
/// that was aborted did not fail an assertion, and saying so keeps the two
/// apart.
///
/// `what` names the task and the work it was doing, not the handle — it is read
/// by someone looking at a failure with no other context, so "the bootstrap of
/// the empty workspace" earns its place where "the task" does not.
pub async fn join_soon<T>(handle: JoinHandle<T>, what: &str) -> T {
    match tokio::time::timeout(JOIN_BOUND, handle).await {
        Ok(Ok(value)) => value,
        // `resume_unwind` re-raises the task's own payload, so the panic that
        // surfaces here is the one the task wrote.
        Ok(Err(joined)) if joined.is_panic() => std::panic::resume_unwind(joined.into_panic()),
        Ok(Err(joined)) => panic!("{what} was cancelled before it finished: {joined}"),
        Err(_) => panic!(
            "{what} had not finished within {JOIN_BOUND:?}: the task is still running, \
             so it is parked on something that is never going to happen"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_finished_task_hands_back_its_value() {
        let task = tokio::spawn(async { "frame" });

        assert_eq!(
            join_soon(task, "a task that returns at once").await,
            "frame"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_that_never_finishes_panics_naming_it() {
        // Pending forever, and never aborted, so the join is blocked on "not
        // done yet" rather than on cancellation — the deadlock case.
        let task = tokio::spawn(std::future::pending::<()>());

        let panic = tokio::spawn(async move {
            join_soon(task, "a bootstrap that never returns").await;
        })
        .await
        .expect_err("joining a task that never finishes must panic");

        let message = panic
            .into_panic()
            .downcast::<String>()
            .expect("the panic payload is the formatted message");
        assert!(
            message.contains("a bootstrap that never returns"),
            "the panic must name the task that hung, got: {message}"
        );
    }

    #[tokio::test]
    async fn a_panicking_task_keeps_its_own_message() {
        // `watermark` is a binding and not an inline literal, and that is what
        // decides the payload type. `panic!` whose arguments are written out as
        // literals is const-folded to a `&'static str`; interpolating a binding
        // leaves a runtime argument, so the payload is a `String`. Measured
        // under edition 2024, all three forms: `panic!("... {}, not 99", 0)`
        // fails the downcast below, `let watermark = 0;` passes it, and adding
        // `std::hint::black_box` around the `0` changes nothing — which is why
        // it is not here.
        //
        // `String` is the shape this test needs, because every message the
        // helper itself formats interpolates a runtime `what` and so is also a
        // `String`. Comparing like with like is what makes the assertion able
        // to tell the task's payload from the join's. If a future compiler ever
        // folds the binding too, the `expect` below fails loudly rather than
        // the test passing on the wrong payload.
        let watermark = 0;
        let task = tokio::spawn(async move { panic!("the watermark was {watermark}, not 99") });

        let panic = tokio::spawn(async move {
            join_soon(task, "a task that fails an assertion").await;
        })
        .await
        .expect_err("joining a task that panicked must panic");

        let message = panic
            .into_panic()
            .downcast::<String>()
            .expect("the panic payload is the task's own formatted message");
        assert!(
            message.contains("the watermark was 0, not 99"),
            "the task's own panic must survive the join, got: {message}"
        );
    }

    #[tokio::test]
    async fn a_cancelled_task_is_reported_as_cancelled() {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();

        let panic = tokio::spawn(async move {
            join_soon(task, "a bootstrap whose task was aborted").await;
        })
        .await
        .expect_err("joining an aborted task must panic");

        let message = panic
            .into_panic()
            .downcast::<String>()
            .expect("the panic payload is the formatted message");
        assert!(
            message.contains("was cancelled"),
            "a cancelled task must be distinguished from one that panicked, got: {message}"
        );
    }
}
