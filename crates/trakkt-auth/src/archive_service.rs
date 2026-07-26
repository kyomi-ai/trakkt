// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background archive sweep — finds completed/cancelled issues older than
//! the configured archive threshold and marks them archived.

use trakkt_core::sql_compat;
use trakkt_core::{db_execute, db_fetch_all, DbPool};
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
            db_execute!(db, &update_sql, &issue.issue_id)?;

            let sync_id = sync_log_service::write_sync_entry(
                db,
                entity_types::ISSUE,
                &issue.issue_id,
                &team.workspace_id,
                None,
                SyncActionType::Delete,
                None,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    error = %e,
                    issue_id = %issue.issue_id,
                    "Failed to write sync log entry for archive"
                );
                0
            });

            sync_log_service::broadcast_sync_action(
                ws_manager,
                &team.workspace_id,
                entity_types::ISSUE,
                &issue.issue_id,
                SyncActionType::Delete,
                None,
                sync_id,
            )
            .await;

            total_archived += 1;

            if (idx + 1) % 100 == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    Ok(total_archived)
}
