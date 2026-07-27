// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background archive sweep — finds completed/cancelled issues older than
//! the configured archive threshold and marks them archived.

use trakkt_core::sql_compat;
use trakkt_core::{db_fetch_all, DbPool};
use trakkt_types::sync::{entity_types, SyncActionType};

use crate::sync_log_service;
use crate::team_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct TeamRef {
    team_id: String,
    workspace_id: String,
}

#[derive(sqlx::FromRow)]
struct IssueIdRow {
    issue_id: String,
}

// ─── Service function ───────────────────────────────────────────────────────

/// Run the auto-archive sweep for all teams in all workspaces.
///
/// For each team with `auto_archive_days` > 0, finds issues where:
/// - status category is 'completed' or 'cancelled'
/// - updated_at is older than the configured days
/// - archived_at is NULL (not already archived)
///
/// Sets `archived_at = NOW()` and writes a sync_log Delete entry so clients
/// remove it from their local store.
///
/// Returns the total number of issues archived.
///
/// # One transaction per issue
///
/// The unit that has to be atomic is one issue: its `archived_at` stamp and the
/// Delete entry telling clients to drop it. An issue archived with no entry
/// disappears from the server and stays on every client forever, so those two
/// commit together.
///
/// The sweep as a whole is deliberately *not* one transaction, which is the
/// opposite call from `release_service::create_release` and for the opposite
/// reason. A release is one fact whose parts mean nothing apart; a sweep is a
/// batch of independent decisions about unrelated issues in unrelated
/// workspaces. Nothing about archiving issue A depends on issue B, so one
/// unreadable row undoing an hour of archiving across every workspace would
/// destroy work for no correctness gain. Wrapping the sweep would also hold a
/// write transaction — on SQLite, the process's only connection — open across
/// the whole run, blocking every request the server is serving, and would make
/// the yield below actively harmful rather than merely pointless.
///
/// Partial progress is safe here because the selection predicate is
/// `archived_at IS NULL`: whatever this run did not reach is simply found again
/// by the next one. So a failure aborts the sweep and is returned to the caller
/// — nothing is swallowed — while the issues already archived stay archived,
/// each with its own entry.
pub async fn run_archive_sweep(
    db: &DbPool,
    ws_manager: &WebSocketManager,
) -> trakkt_core::Result<u64> {
    let teams: Vec<TeamRef> = db_fetch_all!(
        db,
        TeamRef,
        "SELECT team_id, workspace_id FROM teams"
    )?;

    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let update_sql = format!("UPDATE issues SET archived_at = {now_expr} WHERE issue_id = $1");

    let mut total_archived: u64 = 0;

    for team in &teams {
        let archive_days = match team_service::get_team_archive_days(db, &team.team_id, &team.workspace_id).await? {
            Some(days) => days,
            None => continue,
        };

        let ago_expr = sql_compat::ago_days(is_pg, "i.updated_at", "$2");
        let select_sql = format!(
            "SELECT i.issue_id FROM issues i \
             JOIN statuses s ON s.status_id = i.status_id \
             WHERE i.team_id = $1 \
               AND i.archived_at IS NULL \
               AND s.category IN ('completed', 'cancelled') \
               AND {ago_expr}"
        );

        let issues: Vec<IssueIdRow> = db_fetch_all!(
            db,
            IssueIdRow,
            &select_sql,
            &team.team_id,
            i64::from(archive_days)
        )?;

        if issues.is_empty() {
            continue;
        }

        // Process in chunks of 100, yielding between chunks.
        for (idx, issue) in issues.iter().enumerate() {
            // One issue, one transaction: the stamp and the entry that reports
            // it, and nothing else. `commit_and_deliver` takes it by value, so
            // the broadcast — which reads `workspace_users` off the pool the
            // transaction is holding — cannot run before the commit.
            let mut tx = db.begin().await?;

            trakkt_core::tx_execute!(&mut tx, &update_sql, &issue.issue_id)?;

            sync_log_service::commit_and_deliver(
                tx,
                entity_types::ISSUE,
                &issue.issue_id,
                &team.workspace_id,
                sync_log_service::SyncAudience::Workspace,
                SyncActionType::Delete,
                None,
                Some(ws_manager),
            )
            .await?;

            total_archived += 1;

            // Outside the transaction by construction: the yield is between
            // issues, and holding the SQLite connection across an await that
            // hands control to the rest of the server is the deadlock this
            // whole ordering exists to avoid.
            if (idx + 1) % 100 == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    Ok(total_archived)
}
