// SPDX-License-Identifier: AGPL-3.0-or-later

//! Event processing for GitHub webhook events.
//!
//! Processes `pull_request` and `push` events by extracting issue references
//! from PR titles, bodies, branch names, and commit messages, then creating
//! or updating GitHub links in the database.

use std::collections::HashSet;

use trakkt_auth::activity_service::ActivityRecorder;
use trakkt_auth::websocket::WebSocketManager;
use trakkt_core::DbPool;
use trakkt_types::enums::ActionSource;

use crate::patterns::{self, IssueRef};
use crate::schema::{self, CreateLinkParams, GitHubInstallation};

// ===========================================================================
// Pull Request Processing
// ===========================================================================

/// Process a `pull_request` webhook event.
///
/// Extracts issue references from the PR title, body, and branch name,
/// validates them against the database, and upserts GitHub links. For
/// "edited" actions, also removes links to issues no longer referenced.
pub async fn process_pull_request(
    db: &DbPool,
    installation: &GitHubInstallation,
    action: &str,
    payload: &serde_json::Value,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    let pr = match payload.get("pull_request") {
        Some(pr) => pr,
        None => {
            tracing::warn!("pull_request event missing 'pull_request' field");
            return Ok(());
        }
    };

    // ── Extract PR fields ─────────────────────────────────────────────────

    let title = match pr.get("title").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            tracing::warn!("pull_request event missing title");
            return Ok(());
        }
    };

    let body = pr.get("body").and_then(|v| v.as_str()).map(|s| s.to_string());
    let branch_name = match pr.pointer("/head/ref").and_then(|v| v.as_str()) {
        Some(b) => b.to_string(),
        None => {
            tracing::warn!("pull_request event missing head.ref");
            return Ok(());
        }
    };

    let pr_number = match pr.get("number").and_then(|v| v.as_i64()) {
        Some(n) => n,
        None => {
            tracing::warn!("pull_request event missing number");
            return Ok(());
        }
    };

    let html_url = match pr.get("html_url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => {
            tracing::warn!("pull_request event missing html_url");
            return Ok(());
        }
    };

    let author = pr.pointer("/user/login").and_then(|v| v.as_str()).map(|s| s.to_string());
    let base_branch = pr.pointer("/base/ref").and_then(|v| v.as_str()).map(|s| s.to_string());
    // GitHub omits these fields or sets them to null for some event types — false is the correct default.
    let merged = pr.get("merged").and_then(|v| v.as_bool()).unwrap_or(false);
    let draft = pr.get("draft").and_then(|v| v.as_bool()).unwrap_or(false);
    let node_id = pr.get("node_id").and_then(|v| v.as_str()).map(|s| s.to_string());

    let repo = match payload.pointer("/repository/full_name").and_then(|v| v.as_str()) {
        Some(r) => r.to_string(),
        None => {
            tracing::warn!("pull_request event missing repository.full_name");
            return Ok(());
        }
    };

    // ── Determine PR state ────────────────────────────────────────────────

    let pr_state = determine_pr_state(action, merged, draft);

    // ── Collect text and extract refs ─────────────────────────────────────

    let mut text = title.clone();
    if let Some(ref b) = body {
        text.push(' ');
        text.push_str(b);
    }

    let text_refs = patterns::extract_issue_refs(&text);
    let branch_refs = patterns::extract_issue_refs(&branch_name);

    // Merge and deduplicate
    let mut seen = HashSet::new();
    let mut all_refs = Vec::new();
    for r in text_refs.into_iter().chain(branch_refs) {
        if seen.insert(r.clone()) {
            all_refs.push(r);
        }
    }

    // Extract close intent refs from the text (not the branch name)
    let close_refs = patterns::extract_close_intent_refs(&text);
    let close_set: HashSet<IssueRef> = close_refs.into_iter().collect();

    // ── Validate refs and upsert links ────────────────────────────────────

    let mut valid_issue_ids: Vec<String> = Vec::new();

    for issue_ref in &all_refs {
        let issue = match validate_issue_ref(db, &installation.workspace_id, issue_ref).await? {
            Some(i) => i,
            None => continue,
        };

        schema::upsert_link(
            db,
            &CreateLinkParams {
                workspace_id: &installation.workspace_id,
                issue_id: &issue.issue_id,
                installation_id: &installation.installation_id,
                link_type: "pull_request",
                github_id: Some(pr_number),
                github_node_id: node_id.as_deref(),
                repo_full_name: &repo,
                ref_identifier: &pr_number.to_string(),
                title: Some(&title),
                state: Some(&pr_state),
                url: &html_url,
                author_login: author.as_deref(),
                close_intent: close_set.contains(issue_ref),
            },
        )
        .await?;

        valid_issue_ids.push(issue.issue_id.clone());
    }

    // ── For "edited" action, remove stale links ───────────────────────────

    if action == "edited" {
        let deleted = schema::delete_links_not_matching_issues(
            db,
            &installation.workspace_id,
            "pull_request",
            &repo,
            &pr_number.to_string(),
            &valid_issue_ids,
        )
        .await?;

        if deleted > 0 {
            tracing::info!(
                pr_number = pr_number,
                repo = %repo,
                deleted = deleted,
                "Removed stale PR links after edit"
            );
        }
    }

    // ── Record PR activities (best-effort) ────────────────────────────────

    if let Some(activity_type) = pr_activity_type(action, merged) {
        // PR payloads carry no author email, so we cannot resolve a Trakkt
        // user; the author login is surfaced via metadata instead.
        let meta = serde_json::json!({
            "pr_number": pr_number,
            "pr_title": title,
            "state": pr_state,
            "author_login": author,
            "url": html_url,
            "base_branch": base_branch,
        });

        let recorder = ActivityRecorder::new_with_optional_actor(
            db,
            &installation.workspace_id,
            None,
            ActionSource::Github,
            Some("GitHub".to_string()),
            ws_manager,
        );

        let mut recorded: HashSet<&str> = HashSet::new();
        for issue_id in &valid_issue_ids {
            if !recorded.insert(issue_id.as_str()) {
                continue;
            }
            if let Err(e) = recorder.record(issue_id, activity_type, Some(&meta)).await {
                tracing::warn!(
                    error = %e,
                    issue_id = %issue_id,
                    activity_type = %activity_type,
                    "Failed to record pull_request activity"
                );
            }
        }
    }

    tracing::info!(
        pr_number = pr_number,
        repo = %repo,
        state = %pr_state,
        links = valid_issue_ids.len(),
        "Processed pull_request event"
    );

    Ok(())
}

// ===========================================================================
// Push Processing
// ===========================================================================

/// Process a `push` webhook event.
///
/// Extracts issue references from the branch name and each commit message,
/// validates them against the database, and upserts GitHub links.
pub async fn process_push(
    db: &DbPool,
    installation: &GitHubInstallation,
    payload: &serde_json::Value,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<()> {
    // ── Extract branch name ───────────────────────────────────────────────

    let git_ref = match payload.get("ref").and_then(|v| v.as_str()) {
        Some(r) => r.to_string(),
        None => {
            tracing::warn!("push event missing 'ref' field");
            return Ok(());
        }
    };

    let branch_name = git_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(&git_ref)
        .to_string();

    let repo = match payload.pointer("/repository/full_name").and_then(|v| v.as_str()) {
        Some(r) => r.to_string(),
        None => {
            tracing::warn!("push event missing repository.full_name");
            return Ok(());
        }
    };

    // ── Process branch refs ───────────────────────────────────────────────

    let branch_refs = patterns::extract_issue_refs(&branch_name);
    let branch_url = format!("https://github.com/{repo}/tree/{branch_name}");
    let mut total_branch_links: usize = 0;
    let mut branch_issue_ids: Vec<String> = Vec::new();

    for issue_ref in &branch_refs {
        if let Some(issue) =
            validate_issue_ref(db, &installation.workspace_id, issue_ref).await?
        {
            schema::upsert_link(
                db,
                &CreateLinkParams {
                    workspace_id: &installation.workspace_id,
                    issue_id: &issue.issue_id,
                    installation_id: &installation.installation_id,
                    link_type: "branch",
                    github_id: None,
                    github_node_id: None,
                    repo_full_name: &repo,
                    ref_identifier: &branch_name,
                    title: None,
                    state: None,
                    url: &branch_url,
                    author_login: None,
                    close_intent: false,
                },
            )
            .await?;
            total_branch_links += 1;
            branch_issue_ids.push(issue.issue_id);
        }
    }

    // ── Record branch_created activities (best-effort) ────────────────────
    //
    // Only when this push actually created the branch (`created == true`).
    if payload.get("created").and_then(|v| v.as_bool()).unwrap_or(false)
        && !branch_issue_ids.is_empty()
    {
        // The pusher (GitHub actor) login — no email is available, so the
        // actor stays unresolved and the login is surfaced via metadata.
        let pusher_login = payload
            .pointer("/sender/login")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let meta = serde_json::json!({
            "branch": branch_name,
            "url": branch_url,
            "author_login": pusher_login,
        });

        let recorder = ActivityRecorder::new_with_optional_actor(
            db,
            &installation.workspace_id,
            None,
            ActionSource::Github,
            Some("GitHub".to_string()),
            ws_manager,
        );

        let mut recorded: HashSet<&str> = HashSet::new();
        for issue_id in &branch_issue_ids {
            if !recorded.insert(issue_id.as_str()) {
                continue;
            }
            if let Err(e) = recorder.record(issue_id, "branch_created", Some(&meta)).await {
                tracing::warn!(
                    error = %e,
                    issue_id = %issue_id,
                    "Failed to record branch_created activity"
                );
            }
        }
    }

    // ── Process commits ───────────────────────────────────────────────────

    let commits = match payload.get("commits").and_then(|v| v.as_array()) {
        Some(c) => c,
        None => {
            tracing::debug!("push event has no commits array");
            return Ok(());
        }
    };

    let mut total_commit_links: usize = 0;
    // (issue_id, commit) pairs in push order, used to aggregate one
    // `commit_pushed` activity per issue after the link loop.
    let mut issue_commits: Vec<(String, CommitInfo)> = Vec::new();

    for commit in commits {
        let message = match commit.get("message").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => continue,
        };

        let sha = match commit.get("id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let url = match commit.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => continue,
        };

        let author_login = commit
            .pointer("/author/username")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let author_email = commit
            .pointer("/author/email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let commit_refs = patterns::extract_issue_refs(message);
        let close_refs = patterns::extract_close_intent_refs(message);
        let close_set: HashSet<IssueRef> = close_refs.into_iter().collect();

        // Use the first line of the commit message as the link title
        let commit_title = message.lines().next().unwrap_or(message);

        for issue_ref in &commit_refs {
            if let Some(issue) =
                validate_issue_ref(db, &installation.workspace_id, issue_ref).await?
            {
                schema::upsert_link(
                    db,
                    &CreateLinkParams {
                        workspace_id: &installation.workspace_id,
                        issue_id: &issue.issue_id,
                        installation_id: &installation.installation_id,
                        link_type: "commit",
                        github_id: None,
                        github_node_id: None,
                        repo_full_name: &repo,
                        ref_identifier: &sha,
                        title: Some(commit_title),
                        state: None,
                        url: &url,
                        author_login: author_login.as_deref(),
                        close_intent: close_set.contains(issue_ref),
                    },
                )
                .await?;

                total_commit_links += 1;
                issue_commits.push((
                    issue.issue_id,
                    CommitInfo {
                        sha: sha.clone(),
                        message: commit_title.to_string(),
                        url: url.clone(),
                        author_login: author_login.clone(),
                        author_email: author_email.clone(),
                    },
                ));
            }
        }
    }

    // ── Record commit_pushed activities (best-effort, one per issue) ──────

    let grouped = group_commit_activities(issue_commits);
    for (issue_id, agg) in grouped {
        let actor = resolve_author_actor(db, agg.head.author_email.as_deref()).await;
        let meta = serde_json::json!({
            "commit_sha": agg.head.sha,
            "commit_message": agg.head.message,
            "branch": branch_name,
            "author_login": agg.head.author_login,
            "url": agg.head.url,
            "commit_count": agg.count,
        });

        let recorder = ActivityRecorder::new_with_optional_actor(
            db,
            &installation.workspace_id,
            actor.as_deref(),
            ActionSource::Github,
            Some("GitHub".to_string()),
            ws_manager,
        );

        if let Err(e) = recorder.record(&issue_id, "commit_pushed", Some(&meta)).await {
            tracing::warn!(
                error = %e,
                issue_id = %issue_id,
                "Failed to record commit_pushed activity"
            );
        }
    }

    tracing::info!(
        repo = %repo,
        branch = %branch_name,
        commits = commits.len(),
        branch_links = total_branch_links,
        commit_links = total_commit_links,
        "Processed push event"
    );

    Ok(())
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Per-commit fields needed to record a `commit_pushed` activity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitInfo {
    sha: String,
    /// First line of the commit message.
    message: String,
    url: String,
    author_login: Option<String>,
    author_email: Option<String>,
}

/// Aggregated commit activity for a single issue: a count plus the head
/// (last-pushed) commit, which represents the group in the activity feed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitActivityAgg {
    count: usize,
    head: CommitInfo,
}

/// Group `(issue_id, commit)` pairs into one aggregate per issue.
///
/// Within each issue the commits are visited in push order, so the last one
/// seen becomes the `head` and the total is the `count`. Issues are returned
/// sorted by id for deterministic output (and testability).
fn group_commit_activities(
    issue_commits: Vec<(String, CommitInfo)>,
) -> Vec<(String, CommitActivityAgg)> {
    use std::collections::BTreeMap;

    let mut grouped: BTreeMap<String, CommitActivityAgg> = BTreeMap::new();
    for (issue_id, commit) in issue_commits {
        grouped
            .entry(issue_id)
            .and_modify(|agg| {
                agg.count += 1;
                agg.head = commit.clone();
            })
            .or_insert(CommitActivityAgg {
                count: 1,
                head: commit,
            });
    }
    grouped.into_iter().collect()
}

/// Map a pull_request action + merged flag to an activity type, if any.
///
/// Returns `None` for actions that should not produce an activity entry
/// (e.g. `synchronize`, `edited`, `converted_to_draft`).
fn pr_activity_type(action: &str, merged: bool) -> Option<&'static str> {
    match action {
        "opened" | "ready_for_review" | "reopened" => Some("pr_opened"),
        "closed" if merged => Some("pr_merged"),
        "closed" => Some("pr_closed"),
        _ => None,
    }
}

/// Resolve a GitHub author email to a Trakkt user id, if one matches.
async fn resolve_author_actor(db: &DbPool, email: Option<&str>) -> Option<String> {
    let email = email?;
    match trakkt_auth::user_service::get_user_by_email(db, email).await {
        Ok(Some(u)) => Some(u.user_id),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to resolve GitHub author email to user");
            None
        }
    }
}

/// Determine the PR state string from the webhook action and PR flags.
fn determine_pr_state(action: &str, merged: bool, draft: bool) -> String {
    if action == "closed" && merged {
        "merged".to_string()
    } else if action == "closed" {
        "closed".to_string()
    } else if draft {
        "draft".to_string()
    } else {
        "open".to_string()
    }
}

/// A validated issue reference — just the ID we need for link creation.
struct ValidatedIssue {
    issue_id: String,
}

/// Validate an issue reference against the database.
///
/// Checks that both the team and issue exist in the given workspace.
/// Returns `None` (and logs a debug message) if either doesn't exist.
async fn validate_issue_ref(
    db: &DbPool,
    workspace_id: &str,
    issue_ref: &IssueRef,
) -> trakkt_core::Result<Option<ValidatedIssue>> {
    let team = trakkt_auth::team_service::get_team_by_key(db, workspace_id, &issue_ref.team_key)
        .await?;

    if team.is_none() {
        tracing::debug!(
            team_key = %issue_ref.team_key,
            "Team does not exist in workspace — skipping ref"
        );
        return Ok(None);
    }

    let issue = trakkt_auth::issue_service::get_issue(
        db,
        workspace_id,
        &issue_ref.team_key,
        issue_ref.number,
    )
    .await?;

    match issue {
        Some(i) => Ok(Some(ValidatedIssue {
            issue_id: i.issue_id,
        })),
        None => {
            tracing::debug!(
                issue_ref = %format!("{}-{}", issue_ref.team_key, issue_ref.number),
                "Issue does not exist — skipping ref"
            );
            Ok(None)
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_state_merged() {
        assert_eq!(determine_pr_state("closed", true, false), "merged");
    }

    #[test]
    fn pr_state_closed_not_merged() {
        assert_eq!(determine_pr_state("closed", false, false), "closed");
    }

    #[test]
    fn pr_state_draft() {
        assert_eq!(determine_pr_state("opened", false, true), "draft");
    }

    #[test]
    fn pr_state_open() {
        assert_eq!(determine_pr_state("opened", false, false), "open");
    }

    #[test]
    fn pr_state_reopened_draft() {
        // Draft takes precedence when not closed
        assert_eq!(determine_pr_state("reopened", false, true), "draft");
    }

    #[test]
    fn pr_state_closed_merged_overrides_draft() {
        // Merged takes precedence over draft when action is "closed"
        assert_eq!(determine_pr_state("closed", true, true), "merged");
    }

    #[test]
    fn pr_state_synchronize() {
        assert_eq!(determine_pr_state("synchronize", false, false), "open");
    }

    #[test]
    fn pr_state_ready_for_review() {
        // When a PR is marked ready, draft=false
        assert_eq!(determine_pr_state("ready_for_review", false, false), "open");
    }

    #[test]
    fn pr_state_converted_to_draft() {
        assert_eq!(
            determine_pr_state("converted_to_draft", false, true),
            "draft"
        );
    }

    // ── pr_activity_type ──────────────────────────────────────────────────

    #[test]
    fn pr_activity_opened() {
        assert_eq!(pr_activity_type("opened", false), Some("pr_opened"));
    }

    #[test]
    fn pr_activity_ready_for_review() {
        assert_eq!(pr_activity_type("ready_for_review", false), Some("pr_opened"));
    }

    #[test]
    fn pr_activity_reopened() {
        assert_eq!(pr_activity_type("reopened", false), Some("pr_opened"));
    }

    #[test]
    fn pr_activity_closed_merged() {
        assert_eq!(pr_activity_type("closed", true), Some("pr_merged"));
    }

    #[test]
    fn pr_activity_closed_not_merged() {
        assert_eq!(pr_activity_type("closed", false), Some("pr_closed"));
    }

    #[test]
    fn pr_activity_synchronize_is_none() {
        assert_eq!(pr_activity_type("synchronize", false), None);
    }

    #[test]
    fn pr_activity_edited_is_none() {
        assert_eq!(pr_activity_type("edited", false), None);
    }

    #[test]
    fn pr_activity_converted_to_draft_is_none() {
        assert_eq!(pr_activity_type("converted_to_draft", false), None);
    }

    // ── group_commit_activities ───────────────────────────────────────────

    fn commit(sha: &str) -> CommitInfo {
        CommitInfo {
            sha: sha.to_string(),
            message: format!("message for {sha}"),
            url: format!("https://github.com/o/r/commit/{sha}"),
            author_login: Some("octocat".to_string()),
            author_email: Some("octocat@example.com".to_string()),
        }
    }

    #[test]
    fn group_commits_single() {
        let grouped = group_commit_activities(vec![("ISS-1".to_string(), commit("aaa"))]);
        assert_eq!(grouped.len(), 1);
        let (issue_id, agg) = &grouped[0];
        assert_eq!(issue_id, "ISS-1");
        assert_eq!(agg.count, 1);
        assert_eq!(agg.head.sha, "aaa");
    }

    #[test]
    fn group_commits_multiple_same_issue_uses_head() {
        let grouped = group_commit_activities(vec![
            ("ISS-1".to_string(), commit("aaa")),
            ("ISS-1".to_string(), commit("bbb")),
            ("ISS-1".to_string(), commit("ccc")),
        ]);
        assert_eq!(grouped.len(), 1);
        let (_, agg) = &grouped[0];
        assert_eq!(agg.count, 3);
        // Head is the last commit pushed for the issue.
        assert_eq!(agg.head.sha, "ccc");
    }

    #[test]
    fn group_commits_across_issues() {
        let grouped = group_commit_activities(vec![
            ("ISS-2".to_string(), commit("aaa")),
            ("ISS-1".to_string(), commit("bbb")),
            ("ISS-2".to_string(), commit("ccc")),
        ]);
        // Sorted by issue id for determinism.
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, "ISS-1");
        assert_eq!(grouped[0].1.count, 1);
        assert_eq!(grouped[0].1.head.sha, "bbb");
        assert_eq!(grouped[1].0, "ISS-2");
        assert_eq!(grouped[1].1.count, 2);
        assert_eq!(grouped[1].1.head.sha, "ccc");
    }

    #[test]
    fn group_commits_empty() {
        let grouped = group_commit_activities(vec![]);
        assert!(grouped.is_empty());
    }
}
