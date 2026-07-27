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
/// - Records an ISSUE sync entry so clients see the stamp
///
/// # One transaction for the whole release
///
/// Everything above is a single transaction: the `releases` row, every
/// `release_issues` row, every `released_at` stamp, the N ISSUE entries and the
/// one RELEASE entry that reports the release itself.
///
/// This is deliberately all-or-nothing rather than per-issue. A release is one
/// fact about one moment, not a batch of independent jobs — the parts do not
/// mean anything apart. Committing the release without its issue stamps
/// publishes a release whose issues still read as unreleased, and it is
/// unrepairable from the outside: `unreleased_issues` filters on `released_at IS
/// NULL`, so those issues are silently offered up for the *next* release while
/// already sitting in this one. Committing some stamps without the release row
/// is worse still — issues marked released by a release nobody can open. There
/// is no partial result here worth keeping, and the caller's remedy is the same
/// in every case: create the release again.
///
/// So a failed sync entry, on the first issue or the last, unwinds the whole
/// thing. Contrast `archive_service::run_archive_sweep`, which is genuinely a
/// batch of independent jobs and takes the opposite boundary.
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
    let release_id = uuid::Uuid::new_v4().to_string();

    let mut tx = db.begin().await?;

    // Dialect comes from the transaction, not the pool: nothing between here
    // and the commit should have to reach for `db` at all (see `DbTx`).
    let is_pg = tx.is_postgres();
    let now = sql_compat::now(is_pg);

    // Insert the release record.
    let sql = format!(
        "INSERT INTO releases \
            (release_id, workspace_id, team_key, tag_name, previous_tag, \
             title, notes, created_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {now})"
    );
    trakkt_core::tx_execute!(
        &mut tx,
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
        trakkt_core::tx_execute!(
            &mut tx,
            "INSERT INTO release_issues (release_id, issue_id) VALUES ($1, $2) \
             ON CONFLICT (release_id, issue_id) DO NOTHING",
            &release_id,
            issue_id
        )?;
    }

    // Every sync entry below is held here until the commit. Delivering one
    // inside the loop would broadcast a `sync_id` that may still be rolled back,
    // and would read `workspace_users` off the pool while this transaction holds
    // the SQLite connection — a deadlock on the first issue, not a style slip.
    let mut batch = sync_log_service::SyncBatch::new();

    // Stamp released_at on issues that haven't been released yet.
    if !issue_ids.is_empty() {
        let (in_clause, _) =
            trakkt_core::db::in_clause_placeholders(issue_ids.len(), 1);
        let sql = format!(
            "UPDATE issues SET released_at = {now} \
             WHERE issue_id IN {in_clause} AND released_at IS NULL"
        );

        trakkt_core::tx_with!(&mut tx, |e| {
            let mut query = sqlx::query(&sql);
            for id in issue_ids {
                query = query.bind(id);
            }
            query.execute(e).await.map(|r| r.rows_affected())
        })?;

        // One ISSUE entry per issue so clients see the released_at update.
        for issue_id in issue_ids {
            // Read the issue back before the entry is written so the stored row
            // carries the new `released_at`; without a payload the client skips
            // the row on reconnect and the stamp never arrives. The read runs on
            // the transaction: the stamp is not visible anywhere else yet, and
            // on SQLite the pool is not reachable while it is open.
            //
            // Unlike the issue service's own write sites, `issue_ids` comes from
            // the caller, so an id may have been deleted since it was resolved.
            // A row that is not there is not a failure: there is no entity left
            // to report, the UPDATE above simply matched nothing, and the rest
            // of the release is unaffected — so the entry is skipped. That has
            // to stay a different outcome from a sync entry that cannot be
            // written, which aborts the whole release.
            //
            // The two are told apart by the error variant rather than by asking
            // first. A separate existence check would be two round trips with a
            // gap between them, and under Postgres READ COMMITTED a delete
            // committing in that gap passes the check and then fails the read —
            // turning the vanished row back into the aborted release the check
            // existed to prevent. One query has no such window.
            //
            // `NotFound` means the row was absent and nothing else:
            // `issue_sync_payload_tx` constructs it at exactly one place, on
            // `get_issue_by_id_tx` returning `Ok(None)` from a `fetch_optional`.
            // Every failure in that call tree — the detail SELECT, the labels
            // SELECT — arrives as `Error::Sqlx` through the `#[from]` conversion,
            // so a read that broke can never be mistaken for a row that went
            // away, and is returned.
            let payload = match crate::issue_service::issue_sync_payload_tx(&mut tx, issue_id)
                .await
            {
                Ok(payload) => payload,
                Err(trakkt_core::Error::NotFound(_)) => {
                    tracing::warn!(
                        issue_id = %issue_id,
                        "Issue disappeared before its released_at could be synced -- skipping"
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };

            batch
                .record(
                    &mut tx,
                    entity_types::ISSUE,
                    issue_id,
                    workspace_id,
                    sync_log_service::SyncAudience::Workspace,
                    SyncActionType::Update,
                    payload,
                )
                .await?;
        }
    }

    // Re-fetch to get DB-assigned timestamps and issue count. On the
    // transaction: the release and its `release_issues` rows are not visible on
    // the pool yet, so a pool read would find no release at all.
    let sql = format!("{RELEASE_LIST_SELECT} WHERE r.release_id = $1");
    let row = trakkt_core::tx_fetch_one!(&mut tx, ReleaseRow, &sql, &release_id)?;
    let release = row.into_dto();

    // The RELEASE entry last, after every row it describes. It carries the
    // release the caller is about to receive: the same value drives the stored
    // row and the live frame, and a payload-less insert is dropped outright by
    // the client, so an entry without one would leave reconnecting clients with
    // no release at all.
    let payload = sync_log_service::sync_payload(&release, entity_types::RELEASE, &release_id);

    batch
        .record(
            &mut tx,
            entity_types::RELEASE,
            &release_id,
            workspace_id,
            sync_log_service::SyncAudience::Workspace,
            SyncActionType::Insert,
            payload,
        )
        .await?;

    batch.commit_and_deliver(tx, ws_manager).await?;

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
