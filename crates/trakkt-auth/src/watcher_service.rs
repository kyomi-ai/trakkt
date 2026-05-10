// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watcher service — manage issue watchers (the `issue_watchers` table).
//!
//! Users can watch issues to receive notifications about changes. Watching
//! is automatic on issue creation and commenting, but can also be toggled
//! manually from the issue detail page.

use trakkt_core::DbPool;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Row type for fetching watched issue IDs.
#[derive(sqlx::FromRow)]
struct WatchedIssueIdRow {
    issue_id: String,
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Add the user as a watcher of the given issue.
///
/// Uses `ON CONFLICT DO NOTHING` so calling this multiple times is safe.
pub async fn watch_issue(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let sql = if is_pg {
        "INSERT INTO issue_watchers (issue_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    } else {
        "INSERT OR IGNORE INTO issue_watchers (issue_id, user_id) VALUES ($1, $2)"
    };
    trakkt_core::db_execute!(db, sql, issue_id, user_id)?;
    Ok(())
}

/// Remove the user as a watcher of the given issue.
pub async fn unwatch_issue(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "DELETE FROM issue_watchers WHERE issue_id = $1 AND user_id = $2",
        issue_id,
        user_id
    )?;
    Ok(())
}

/// List all issue IDs that the user is watching within a given workspace.
///
/// Joins with `issues` to scope results to the workspace (users may watch
/// issues across workspaces if they switch contexts).
pub async fn list_watched_issue_ids(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<String>> {
    let rows: Vec<WatchedIssueIdRow> = trakkt_core::db_fetch_all!(
        db,
        WatchedIssueIdRow,
        "SELECT iw.issue_id \
         FROM issue_watchers iw \
         JOIN issues i ON iw.issue_id = i.issue_id \
         WHERE iw.user_id = $1 AND i.workspace_id = $2",
        user_id,
        workspace_id
    )?;
    Ok(rows.into_iter().map(|r| r.issue_id).collect())
}

/// Check whether the user is watching a specific issue.
pub async fn is_watching(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
) -> trakkt_core::Result<bool> {
    let count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM issue_watchers WHERE issue_id = $1 AND user_id = $2",
        issue_id,
        user_id
    )?;
    Ok(count > 0)
}
