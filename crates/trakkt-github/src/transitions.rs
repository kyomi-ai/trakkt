// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic issue status transitions triggered by GitHub events.
//!
//! When a PR is opened, merged, or closed, and the workspace has matching
//! transition rules enabled, this module:
//! 1. Resolves the target status from the rule's `target_status_category`
//! 2. Updates the linked issue(s) status via `issue_service::update_issue`
//! 3. Posts a bot comment on the GitHub PR (best-effort)
//!
//! Additionally, when a Trakkt issue is manually moved to a "completed" status,
//! `notify_github_links_on_completion` posts a comment on linked PRs.

use trakkt_core::DbPool;

use crate::schema::{self, GitHubInstallation, GitHubTransitionRule};
use crate::GitHubClient;

// ─── Trigger Event Mapping ─────────────────────────────────────────────────

/// Map a link_type + webhook action + PR state to a trigger event string.
///
/// Returns `None` if the action does not correspond to a transition trigger.
fn determine_trigger(action: &str, merged: bool) -> Option<&'static str> {
    match action {
        "opened" | "ready_for_review" => Some("pr_opened"),
        "closed" if merged => Some("pr_merged"),
        "closed" => Some("pr_closed"),
        _ => None,
    }
}

// ─── Rule Matching ─────────────────────────────────────────────────────────

/// Find the most specific enabled transition rule for the given event.
///
/// Prefers rules with `close_intent_required = true` when `has_close_intent`
/// is true, falling back to rules without the requirement.
///
/// Filter logic:
/// - Rules with `close_intent_required = false` are always eligible.
/// - Rules with `close_intent_required = true` are only eligible when the link
///   has `close_intent = true`.
/// - `ORDER BY close_intent_required DESC` ensures the more specific rule wins.
async fn query_matching_rule(
    db: &DbPool,
    workspace_id: &str,
    trigger_event: &str,
    has_close_intent: bool,
) -> trakkt_core::Result<Option<GitHubTransitionRule>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubTransitionRule,
        "SELECT rule_id, workspace_id, trigger_event, close_intent_required, \
                target_status_category, enabled, \
                CAST(created_at AS TEXT) AS created_at \
         FROM github_transition_rules \
         WHERE workspace_id = $1 AND trigger_event = $2 AND enabled = $3 \
               AND (close_intent_required = $4 OR $5 = $3) \
         ORDER BY close_intent_required DESC \
         LIMIT 1",
        workspace_id,
        trigger_event,
        true,
        false,
        has_close_intent
    )?;
    Ok(row)
}

// ─── Token Management ──────────────────────────────────────────────────────

/// Get or refresh an installation access token.
///
/// Checks the cached token's expiry (with 5-minute buffer). If still valid,
/// decrypts and returns it. Otherwise, requests a fresh token from GitHub,
/// encrypts and persists it, then returns the new token.
async fn get_installation_token(
    db: &DbPool,
    github_client: &GitHubClient,
    installation: &GitHubInstallation,
    encryption_key: &[u8; 32],
) -> trakkt_core::Result<String> {
    // Check if the cached token is still valid (with 5-minute buffer).
    if let Some(ref encrypted_token) = installation.access_token_encrypted
        && let Some(ref expires_str) = installation.token_expires_at
        && let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(expires_str)
    {
        let buffer = chrono::Duration::minutes(5);
        if chrono::Utc::now() < expires_at - buffer {
            return trakkt_auth::encryption::decrypt(encrypted_token, encryption_key);
        }
    }

    // Token expired or missing — request a fresh one.
    let github_installation_id = installation.github_installation_id as u64;
    let token_response = github_client
        .request_installation_token(github_installation_id)
        .await?;

    // Encrypt and persist the new token.
    let encrypted = trakkt_auth::encryption::encrypt(&token_response.token, encryption_key)?;
    schema::update_installation_token(
        db,
        &installation.installation_id,
        &encrypted,
        &token_response.expires_at,
    )
    .await?;

    Ok(token_response.token)
}

// ─── Bot Comment Formatting ────────────────────────────────────────────────

/// Format a bot comment for a status transition triggered by a PR event.
fn format_transition_comment(
    trigger: &str,
    issue_team_key: &str,
    issue_number: i32,
    issue_title: &str,
    target_status_name: &str,
    base_url: &str,
) -> String {
    let issue_ref = format!("{issue_team_key}-{issue_number}");
    let issue_url = format!("{base_url}/issues/{issue_team_key}-{issue_number}");

    match trigger {
        "pr_merged" => {
            format!(
                "[{issue_ref}]({issue_url}): *{issue_title}* — marked as **{target_status_name}**"
            )
        }
        "pr_opened" => {
            format!(
                "Linked to [{issue_ref}]({issue_url}): *{issue_title}*\n\
                 Status moved to **{target_status_name}**"
            )
        }
        _ => {
            format!(
                "[{issue_ref}]({issue_url}): *{issue_title}* — moved to **{target_status_name}**"
            )
        }
    }
}

// ─── Main Transition Orchestration ─────────────────────────────────────────

/// Apply automatic status transitions after a PR event is processed.
///
/// Called from the webhook handler after `process_pull_request` has created/
/// updated GitHub links. Finds all linked issues for the PR, checks transition
/// rules, updates statuses, and posts bot comments (best-effort).
pub async fn apply_transition_rules(
    db: &DbPool,
    github_client: &GitHubClient,
    installation: &GitHubInstallation,
    workspace_id: &str,
    action: &str,
    payload: &serde_json::Value,
    encryption_key: &[u8; 32],
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
    base_url: &str,
) -> trakkt_core::Result<()> {
    // Extract PR details from payload.
    let pr = match payload.get("pull_request") {
        Some(pr) => pr,
        None => return Ok(()),
    };

    let pr_number = match pr.get("number").and_then(|v| v.as_i64()) {
        Some(n) => n,
        None => return Ok(()),
    };

    let repo = match payload.pointer("/repository/full_name").and_then(|v| v.as_str()) {
        Some(r) => r.to_string(),
        None => return Ok(()),
    };

    let merged = pr.get("merged").and_then(|v| v.as_bool()).unwrap_or(false);

    // Determine the trigger event.
    let trigger = match determine_trigger(action, merged) {
        Some(t) => t,
        None => return Ok(()),
    };

    // Find all PR links for this PR in this workspace.
    let links = schema::list_pr_links_by_ref(
        db,
        workspace_id,
        &repo,
        &pr_number.to_string(),
    )
    .await?;

    if links.is_empty() {
        return Ok(());
    }

    // Try to get an installation token (needed for bot comments).
    let token = match get_installation_token(db, github_client, installation, encryption_key).await {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!(
                error = %e,
                installation_id = %installation.installation_id,
                "Failed to get installation token for bot comments — transitions will still apply"
            );
            None
        }
    };

    for link in &links {
        // Query matching transition rule for this link.
        let rule = match query_matching_rule(db, workspace_id, trigger, link.close_intent).await? {
            Some(r) => r,
            None => continue,
        };

        // Resolve the target status from the rule's category.
        let target_status = match trakkt_auth::status_service::get_status_by_category(db, workspace_id, &rule.target_status_category).await? {
            Some(s) => s,
            None => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    category = %rule.target_status_category,
                    "No status found in target category for transition rule — skipping"
                );
                continue;
            }
        };

        // Fetch the issue to check current status and get details for the comment.
        let issue = match trakkt_auth::issue_service::get_issue_by_id(db, &link.issue_id).await? {
            Some(i) => i,
            None => {
                tracing::warn!(
                    issue_id = %link.issue_id,
                    "Linked issue not found — skipping transition"
                );
                continue;
            }
        };

        // Skip if issue is already in the target status (no-op).
        if issue.status_id == target_status.status_id {
            tracing::debug!(
                issue_id = %link.issue_id,
                status = %target_status.name,
                "Issue already in target status — skipping"
            );
            continue;
        }

        // Apply the status update via the issue service.
        let updates = trakkt_types::models::IssueUpdate {
            status_id: Some(target_status.status_id.clone()),
            ..Default::default()
        };

        match trakkt_auth::issue_service::update_issue(
            db,
            workspace_id,
            &issue.team_key,
            issue.number,
            &updates,
            None, // No actor user — system-initiated
            trakkt_types::enums::ActionSource::Api,
            None,
            ws_manager,
        )
        .await
        {
            Ok(_) => {
                tracing::info!(
                    issue_key = %issue.team_key,
                    issue_number = issue.number,
                    trigger = %trigger,
                    target_status = %target_status.name,
                    "Applied automatic status transition"
                );
            }
            Err(e) => {
                tracing::warn!(
                    issue_id = %link.issue_id,
                    error = %e,
                    "Failed to apply status transition"
                );
                continue;
            }
        }

        // Post bot comment on the PR (best-effort).
        if let Some(ref token) = token {
            let comment_body = format_transition_comment(
                trigger,
                &issue.team_key,
                issue.number,
                &issue.title,
                &target_status.name,
                base_url,
            );

            if let Err(e) = github_client
                .create_comment(token, &repo, pr_number as u64, &comment_body)
                .await
            {
                tracing::warn!(
                    error = %e,
                    repo = %repo,
                    pr_number = pr_number,
                    "Failed to post bot comment on PR — transition was applied successfully"
                );
            }
        }
    }

    Ok(())
}

// ─── Outbound: Notify GitHub on Issue Completion ───────────────────────────

/// Post a comment on linked GitHub PRs when an issue is manually completed.
///
/// Called from the update_issue API handler when the new status is in the
/// "completed" category. Best-effort — errors are logged but do not fail
/// the API request.
pub async fn notify_github_links_on_completion(
    db: &DbPool,
    github_client: &GitHubClient,
    encryption_key: &[u8; 32],
    issue_id: &str,
    issue_team_key: &str,
    issue_number: i32,
    issue_title: &str,
    status_name: &str,
    base_url: &str,
) -> trakkt_core::Result<()> {
    // Find all PR links with close_intent for this issue.
    let links = schema::list_close_intent_links_for_issue(db, issue_id).await?;

    if links.is_empty() {
        return Ok(());
    }

    let issue_ref = format!("{issue_team_key}-{issue_number}");
    let issue_url = format!("{base_url}/issues/{issue_team_key}-{issue_number}");
    let comment_body = format!(
        "[{issue_ref}]({issue_url}): *{issue_title}* — marked as **{status_name}** in Trakkt"
    );

    for link in &links {
        // Look up the installation for this link.
        let installation = match schema::get_installation_by_id(db, &link.installation_id).await? {
            Some(inst) => inst,
            None => {
                tracing::warn!(
                    installation_id = %link.installation_id,
                    "Installation not found for outbound notification — skipping"
                );
                continue;
            }
        };

        // Skip suspended installations.
        if installation.suspended_at.is_some() {
            tracing::debug!(
                installation_id = %installation.installation_id,
                "Installation is suspended — skipping outbound notification"
            );
            continue;
        }

        // Get an installation token.
        let token = match get_installation_token(db, github_client, &installation, encryption_key).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    installation_id = %installation.installation_id,
                    "Failed to get token for outbound notification — skipping"
                );
                continue;
            }
        };

        // Parse the PR number from ref_identifier.
        let pr_number: u64 = match link.ref_identifier.parse() {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!(
                    ref_identifier = %link.ref_identifier,
                    "Cannot parse ref_identifier as PR number — skipping"
                );
                continue;
            }
        };

        if let Err(e) = github_client
            .create_comment(&token, &link.repo_full_name, pr_number, &comment_body)
            .await
        {
            tracing::warn!(
                error = %e,
                repo = %link.repo_full_name,
                pr_number = pr_number,
                "Failed to post outbound completion comment on PR"
            );
        }
    }

    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determine_trigger_pr_opened() {
        assert_eq!(determine_trigger("opened", false), Some("pr_opened"));
    }

    #[test]
    fn determine_trigger_ready_for_review() {
        assert_eq!(determine_trigger("ready_for_review", false), Some("pr_opened"));
    }

    #[test]
    fn determine_trigger_closed_merged() {
        assert_eq!(determine_trigger("closed", true), Some("pr_merged"));
    }

    #[test]
    fn determine_trigger_closed_not_merged() {
        assert_eq!(determine_trigger("closed", false), Some("pr_closed"));
    }

    #[test]
    fn determine_trigger_other_action() {
        assert_eq!(determine_trigger("synchronize", false), None);
        assert_eq!(determine_trigger("edited", false), None);
        assert_eq!(determine_trigger("reopened", false), None);
    }

    #[test]
    fn format_comment_pr_merged() {
        let comment = format_transition_comment(
            "pr_merged",
            "TRA",
            17,
            "Fix login timeout",
            "Done",
            "https://app.trakkt.dev",
        );
        assert!(comment.contains("[TRA-17]"));
        assert!(comment.contains("https://app.trakkt.dev/issues/TRA-17"));
        assert!(comment.contains("*Fix login timeout*"));
        assert!(comment.contains("**Done**"));
    }

    #[test]
    fn format_comment_pr_opened() {
        let comment = format_transition_comment(
            "pr_opened",
            "ENG",
            42,
            "Add caching layer",
            "In Progress",
            "https://trakkt.local",
        );
        assert!(comment.contains("[ENG-42]"));
        assert!(comment.contains("https://trakkt.local/issues/ENG-42"));
        assert!(comment.contains("*Add caching layer*"));
        assert!(comment.contains("**In Progress**"));
    }

    #[test]
    fn format_comment_pr_closed() {
        let comment = format_transition_comment(
            "pr_closed",
            "BUG",
            3,
            "Some bug",
            "Cancelled",
            "https://example.com",
        );
        assert!(comment.contains("[BUG-3]"));
        assert!(comment.contains("https://example.com/issues/BUG-3"));
        assert!(comment.contains("*Some bug*"));
        assert!(comment.contains("**Cancelled**"));
    }
}
