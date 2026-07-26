// SPDX-License-Identifier: AGPL-3.0-or-later

//! Release service — CRUD operations for the `releases` and `release_issues`
//! tables.
//!
//! Releases are workspace-scoped entities that track which issues shipped in a
//! given git tag. When a release is created with commit SHAs, the service
//! auto-discovers linked issues via the `github_links` table and stamps
//! `released_at` on each linked issue.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::{Release, ReleaseIssue, ReleaseWithIssues};
use trakkt_types::sync::{SyncActionType, entity_types};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Internal row type for deserialising `releases` query results with issue count.
#[derive(sqlx::FromRow)]
struct ReleaseRow {
    release_id: String,
    workspace_id: String,
    team_key: String,
    tag_name: String,
    previous_tag: Option<String>,
    title: Option<String>,
    notes: Option<String>,
    created_by: String,
    created_at: String,
    issue_count: i64,
}

impl ReleaseRow {
    fn into_dto(self) -> Release {
        Release {
            release_id: self.release_id,
            workspace_id: self.workspace_id,
            team_key: self.team_key,
            tag_name: self.tag_name,
            previous_tag: self.previous_tag,
            title: self.title,
            notes: self.notes,
            created_by: self.created_by,
            created_at: self.created_at,
            issue_count: self.issue_count,
        }
    }
}

/// Internal row type for release issue details.
#[derive(sqlx::FromRow)]
struct ReleaseIssueRow {
    issue_id: String,
    team_key: String,
    number: i32,
    title: String,
    status_name: String,
    status_category: String,
}

impl ReleaseIssueRow {
    fn into_dto(self) -> ReleaseIssue {
        ReleaseIssue {
            issue_id: self.issue_id,
            team_key: self.team_key,
            number: self.number,
            title: self.title,
            status_name: self.status_name,
            status_category: self.status_category,
        }
    }
}

/// Internal row type for the release header (without issue count).
#[derive(sqlx::FromRow)]
struct ReleaseHeaderRow {
    release_id: String,
    workspace_id: String,
    team_key: String,
    tag_name: String,
    previous_tag: Option<String>,
    title: Option<String>,
    notes: Option<String>,
    created_by: String,
    created_at: String,
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Base SELECT for release list queries (with issue count).
const RELEASE_LIST_SELECT: &str = "\
    SELECT r.release_id, r.workspace_id, r.team_key, r.tag_name, r.previous_tag, \
           r.title, r.notes, r.created_by, \
           CAST(r.created_at AS TEXT) AS created_at, \
           (SELECT COUNT(*) FROM release_issues ri WHERE ri.release_id = r.release_id) AS issue_count \
    FROM releases r";

/// Base SELECT for a single release header (no count needed, issues fetched separately).
const RELEASE_HEADER_SELECT: &str = "\
    SELECT r.release_id, r.workspace_id, r.team_key, r.tag_name, r.previous_tag, \
           r.title, r.notes, r.created_by, \
           CAST(r.created_at AS TEXT) AS created_at \
    FROM releases r";

// ─── Service functions ──────────────────────────────────────────────────────

/// Create a new release with pre-resolved linked issue IDs.
///
/// The caller (API layer) is responsible for resolving commit SHAs to issue IDs
/// via `trakkt_github::schema::lookup_issues_by_ref`. This avoids a circular
/// dependency between `trakkt-auth` and `trakkt-github`.
///
/// For each issue ID:
/// - Inserts into `release_issues`
/// - Sets `released_at = now()` on issues that haven't been released yet
/// - Writes a sync log entry and broadcasts via WebSocket
pub async fn create_release(
    db: &DbPool,
    workspace_id: &str,
    team_key: &str,
    tag_name: &str,
    previous_tag: Option<&str>,
    title: Option<&str>,
    notes: Option<&str>,
    issue_ids: &[String],
    created_by: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<Release> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let release_id = uuid::Uuid::new_v4().to_string();

    // Insert the release record.
    let sql = format!(
        "INSERT INTO releases \
            (release_id, workspace_id, team_key, tag_name, previous_tag, \
             title, notes, created_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &release_id,
        workspace_id,
        team_key,
        tag_name,
        previous_tag,
        title,
        notes,
        created_by
    )?;

    // Insert into release_issues for each linked issue.
    for issue_id in issue_ids {
        trakkt_core::db_execute!(
            db,
            "INSERT INTO release_issues (release_id, issue_id) VALUES ($1, $2) \
             ON CONFLICT (release_id, issue_id) DO NOTHING",
            &release_id,
            issue_id
        )?;
    }

    // Stamp released_at on issues that haven't been released yet.
    if !issue_ids.is_empty() {
        let (in_clause, _) =
            trakkt_core::db::in_clause_placeholders(issue_ids.len(), 1);
        let sql = format!(
            "UPDATE issues SET released_at = {now} \
             WHERE issue_id IN {in_clause} AND released_at IS NULL"
        );

        trakkt_core::db_with_pool!(db, |p| {
            let mut query = sqlx::query(&sql);
            for id in issue_ids {
                query = query.bind(id);
            }
            query.execute(p).await.map(|r| r.rows_affected())
        })?;

        // Write sync_log + broadcast for each affected issue so clients see
        // the released_at update in real time.
        for issue_id in issue_ids {
            let sync_id = sync_log_service::write_sync_entry(
                db,
                entity_types::ISSUE,
                issue_id,
                workspace_id,
                SyncActionType::Update,
                None,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, issue_id = %issue_id, "Failed to write sync log for issue released_at");
                0
            });

            if let Some(ws) = ws_manager
                && let Ok(Some(full_issue)) =
                    crate::issue_service::get_issue_by_id(db, issue_id).await
            {
                sync_log_service::broadcast_sync_action(
                    ws,
                    workspace_id,
                    entity_types::ISSUE,
                    issue_id,
                    SyncActionType::Update,
                    serde_json::to_value(&full_issue).ok(),
                    sync_id,
                )
                .await;
            }
        }
    }

    // Sync log for the release entity — best-effort.
    let sync_id = sync_log_service::write_sync_entry(
        db,
        entity_types::RELEASE,
        &release_id,
        workspace_id,
        SyncActionType::Insert,
        None,
    )
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, release_id = %release_id, "Failed to write sync log entry for release create");
        0
    });

    // Re-fetch to get DB-assigned timestamps and issue count.
    let sql = format!(
        "{RELEASE_LIST_SELECT} WHERE r.release_id = $1"
    );
    let row = trakkt_core::db_fetch_one!(
        db,
        ReleaseRow,
        &sql,
        &release_id
    )?;
    let release = row.into_dto();

    // WebSocket broadcast — send full release entity data.
    if let Some(ws) = ws_manager {
        sync_log_service::broadcast_sync_action(
            ws,
            workspace_id,
            entity_types::RELEASE,
            &release_id,
            SyncActionType::Insert,
            serde_json::to_value(&release).ok(),
            sync_id,
        )
        .await;
    }

    Ok(release)
}

/// List all releases in a workspace, optionally filtered by team key.
///
/// Returns releases ordered by creation date (newest first) with issue counts.
pub async fn list_releases(
    db: &DbPool,
    workspace_id: &str,
    team_key: Option<&str>,
) -> trakkt_core::Result<Vec<Release>> {
    let rows: Vec<ReleaseRow> = if let Some(tk) = team_key {
        let sql = format!(
            "{RELEASE_LIST_SELECT} WHERE r.workspace_id = $1 AND r.team_key = $2 \
             ORDER BY r.created_at DESC"
        );
        trakkt_core::db_fetch_all!(db, ReleaseRow, &sql, workspace_id, tk)?
    } else {
        let sql = format!(
            "{RELEASE_LIST_SELECT} WHERE r.workspace_id = $1 \
             ORDER BY r.created_at DESC"
        );
        trakkt_core::db_fetch_all!(db, ReleaseRow, &sql, workspace_id)?
    };

    Ok(rows.into_iter().map(ReleaseRow::into_dto).collect())
}

/// Get a single release by ID, including its linked issues with details.
pub async fn get_release(
    db: &DbPool,
    release_id: &str,
) -> trakkt_core::Result<Option<ReleaseWithIssues>> {
    // Fetch the release header.
    let sql = format!("{RELEASE_HEADER_SELECT} WHERE r.release_id = $1");
    let header = trakkt_core::db_fetch_optional!(
        db,
        ReleaseHeaderRow,
        &sql,
        release_id
    )?;

    let header = match header {
        Some(h) => h,
        None => return Ok(None),
    };

    // Fetch linked issues with details.
    let issue_rows: Vec<ReleaseIssueRow> = trakkt_core::db_fetch_all!(
        db,
        ReleaseIssueRow,
        "SELECT i.issue_id, t.key AS team_key, i.number, i.title, \
                s.name AS status_name, s.category AS status_category \
         FROM release_issues ri \
         JOIN issues i ON i.issue_id = ri.issue_id \
         JOIN teams t ON t.team_id = i.team_id \
         JOIN statuses s ON s.status_id = i.status_id \
         WHERE ri.release_id = $1 \
         ORDER BY t.key ASC, i.number ASC",
        release_id
    )?;

    let issues: Vec<ReleaseIssue> = issue_rows
        .into_iter()
        .map(ReleaseIssueRow::into_dto)
        .collect();

    Ok(Some(ReleaseWithIssues {
        release_id: header.release_id,
        workspace_id: header.workspace_id,
        team_key: header.team_key,
        tag_name: header.tag_name,
        previous_tag: header.previous_tag,
        title: header.title,
        notes: header.notes,
        created_by: header.created_by,
        created_at: header.created_at,
        issues,
    }))
}

/// List issues that are completed/cancelled but not yet released.
///
/// Returns issues with full details where `completed_at IS NOT NULL` and
/// `released_at IS NULL`, optionally filtered by team key.
pub async fn unreleased_issues(
    db: &DbPool,
    workspace_id: &str,
    team_key: Option<&str>,
) -> trakkt_core::Result<Vec<ReleaseIssue>> {
    let rows: Vec<ReleaseIssueRow> = if let Some(tk) = team_key {
        trakkt_core::db_fetch_all!(
            db,
            ReleaseIssueRow,
            "SELECT i.issue_id, t.key AS team_key, i.number, i.title, \
                    s.name AS status_name, s.category AS status_category \
             FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             JOIN statuses s ON s.status_id = i.status_id \
             WHERE i.workspace_id = $1 AND t.key = $2 \
                   AND i.completed_at IS NOT NULL AND i.released_at IS NULL \
             ORDER BY i.completed_at DESC",
            workspace_id,
            tk
        )?
    } else {
        trakkt_core::db_fetch_all!(
            db,
            ReleaseIssueRow,
            "SELECT i.issue_id, t.key AS team_key, i.number, i.title, \
                    s.name AS status_name, s.category AS status_category \
             FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             JOIN statuses s ON s.status_id = i.status_id \
             WHERE i.workspace_id = $1 \
                   AND i.completed_at IS NOT NULL AND i.released_at IS NULL \
             ORDER BY i.completed_at DESC",
            workspace_id
        )?
    };

    Ok(rows.into_iter().map(ReleaseIssueRow::into_dto).collect())
}
