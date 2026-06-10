// SPDX-License-Identifier: AGPL-3.0-or-later

//! Star service — manage issue stars (the `issue_stars` table).
//!
//! Users can star issues to pin them as personal bookmarks. Stars are
//! per-user, ephemeral preferences (like watchers). They appear as a
//! "Starred" preset view in the workspace sidebar.

use trakkt_core::DbPool;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Row type for fetching starred issue IDs.
#[derive(sqlx::FromRow)]
struct StarredIssueIdRow {
    issue_id: String,
}

// ─── Service functions ──────────────────────────────────────────────────────

/// Star an issue for the given user.
///
/// Uses `ON CONFLICT DO NOTHING` so calling this multiple times is safe.
pub async fn star_issue(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let sql = if is_pg {
        "INSERT INTO issue_stars (issue_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    } else {
        "INSERT OR IGNORE INTO issue_stars (issue_id, user_id) VALUES ($1, $2)"
    };
    trakkt_core::db_execute!(db, sql, issue_id, user_id)?;
    Ok(())
}

/// Remove the star from an issue for the given user.
pub async fn unstar_issue(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "DELETE FROM issue_stars WHERE issue_id = $1 AND user_id = $2",
        issue_id,
        user_id
    )?;
    Ok(())
}

/// List all issue IDs that the user has starred within a given workspace.
///
/// Joins with `issues` to scope results to the workspace (users may star
/// issues across workspaces if they switch contexts).
pub async fn list_starred_issue_ids(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<String>> {
    let rows: Vec<StarredIssueIdRow> = trakkt_core::db_fetch_all!(
        db,
        StarredIssueIdRow,
        "SELECT s.issue_id \
         FROM issue_stars s \
         JOIN issues i ON s.issue_id = i.issue_id \
         WHERE s.user_id = $1 AND i.workspace_id = $2",
        user_id,
        workspace_id
    )?;
    Ok(rows.into_iter().map(|r| r.issue_id).collect())
}

/// Check whether the user has starred a specific issue.
pub async fn is_starred(
    db: &DbPool,
    issue_id: &str,
    user_id: &str,
) -> trakkt_core::Result<bool> {
    let count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM issue_stars WHERE issue_id = $1 AND user_id = $2",
        issue_id,
        user_id
    )?;
    Ok(count > 0)
}
