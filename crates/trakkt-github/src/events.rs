// SPDX-License-Identifier: AGPL-3.0-or-later

//! Event processing for GitHub webhook events.
//!
//! Processes `pull_request` and `push` events by extracting issue references
//! from PR titles, bodies, branch names, and commit messages, then creating
//! or updating GitHub links in the database.

use std::collections::HashSet;

use trakkt_core::DbPool;

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
            }
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
}
