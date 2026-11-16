// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic issue status transitions triggered by GitHub events.
//!
//! A PR event moves a linked issue only when all of these hold, in order:
//! 1. The event is an automation step (draft, opened/ready, merged, closed).
//! 2. The link's [`LinkKind`] is allowed to fire that step ([`behavior`]).
//! 3. A rule exists for the step: the issue's team override, else the
//!    workspace default. A team override with no target means "no action".
//! 4. The move is forward, and the issue is not completed or cancelled
//!    ([`decide`]). GitHub never reopens finished work or moves work backwards.
//! 5. For merge and close, no other closing PR on the issue is still open.
//!
//! Then it resolves the target status, updates the issue, and (if the
//! workspace has linkback on) posts a bot comment on the PR.
//!
//! Additionally, when a Trakkt issue is manually moved to a "completed" status,
//! `notify_github_links_on_completion` posts a comment on linked PRs.
//! The model is Linear's GitHub integration; deviations are documented in
//! docs/ARCHITECTURE.md ("GitHub link intent").

use trakkt_core::DbPool;

use crate::GitHubClient;
use crate::patterns::LinkKind;
use crate::schema::{self, GitHubInstallation, GitHubTransitionRule};

// ─── Trigger Event Mapping ─────────────────────────────────────────────────

/// Map a webhook action and the PR's flags to an automation step.
///
/// Returns `None` for actions that are not a step (`synchronize`, `edited`,
/// ...). Automation fires on lifecycle events, not on edits.
fn determine_trigger(action: &str, merged: bool, draft: bool) -> Option<&'static str> {
    match action {
        // A reopened PR starts the issue like an opened one; forward-only
        // keeps that from undoing anything.
        "opened" | "reopened" if draft => Some("pr_draft"),
        "opened" | "reopened" | "ready_for_review" => Some("pr_opened"),
        "converted_to_draft" => Some("pr_draft"),
        "closed" if merged => Some("pr_merged"),
        "closed" => Some("pr_closed"),
        _ => None,
    }
}

/// What a link of a given kind is allowed to do. This is the single place the
/// per-kind behaviour lives; changing what a kind does is changing one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KindBehavior {
    /// May fire the draft and opened/ready steps.
    starts: bool,
    /// May fire the merged and closed steps.
    completes: bool,
}

const fn behavior(kind: LinkKind) -> KindBehavior {
    match kind {
        // Shown on the issue and the PR, nothing more.
        LinkKind::Related => KindBehavior {
            starts: false,
            completes: false,
        },
        // Proposed deviation from Linear, pending owner confirmation: a link
        // from the branch name or PR title starts the issue but does not
        // complete it. Setting `completes` to true follows Linear.
        LinkKind::BranchTitle => KindBehavior {
            starts: true,
            completes: false,
        },
        LinkKind::Contributing => KindBehavior {
            starts: true,
            completes: false,
        },
        LinkKind::Closing => KindBehavior {
            starts: true,
            completes: true,
        },
    }
}

fn kind_may_fire(kind: LinkKind, trigger: &str) -> bool {
    let b = behavior(kind);
    match trigger {
        "pr_draft" | "pr_opened" => b.starts,
        "pr_merged" | "pr_closed" => b.completes,
        _ => false,
    }
}

/// Where a status category sits on the way from idea to finished.
/// Completed and cancelled are both an end.
fn category_rank(category: &str) -> Option<u8> {
    match category {
        "backlog" => Some(0),
        "unstarted" => Some(1),
        "started" => Some(2),
        "completed" | "cancelled" => Some(3),
        _ => None,
    }
}

/// Whether a status category means the issue's work is over.
///
/// GitHub events never move an issue out of one of these. Reopening finished or
/// abandoned work is a decision for a person, not a side effect of a PR that
/// happens to name the ticket.
fn is_closed_category(category: &str) -> bool {
    matches!(category, "completed" | "cancelled")
}

/// The pure part of the decision: given a link kind, a step, the rule's target
/// and the issue's current category, which category (if any) to move to.
fn decide<'a>(
    kind: LinkKind,
    trigger: &str,
    current_category: &str,
    target_category: &'a str,
) -> Option<&'a str> {
    if !kind_may_fire(kind, trigger) || is_closed_category(current_category) {
        return None;
    }
    // Forward only. The same category never moves either, so In Review is
    // never pulled back to In Progress by a later event.
    match (
        category_rank(current_category),
        category_rank(target_category),
    ) {
        (Some(current), Some(target)) if target > current => Some(target_category),
        _ => None,
    }
}

// ─── Rule Matching ─────────────────────────────────────────────────────────

/// The target the issue's team has configured for a step.
enum ResolvedRule {
    /// Move to this status category.
    Target(String),
    /// The step does nothing for this issue.
    NoAction,
}

/// Find the rule for a step: the issue's team override, else the enabled
/// workspace default. `None` means nothing is configured, so nothing happens.
async fn resolve_rule(
    db: &DbPool,
    workspace_id: &str,
    team_id: &str,
    trigger_event: &str,
) -> trakkt_core::Result<Option<ResolvedRule>> {
    if let Some(team_rule) = schema::get_team_transition_rule(db, team_id, trigger_event).await? {
        return Ok(Some(match team_rule.target_status_category {
            Some(category) => ResolvedRule::Target(category),
            None => ResolvedRule::NoAction,
        }));
    }
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubTransitionRule,
        "SELECT rule_id, workspace_id, trigger_event, close_intent_required, \
                target_status_category, enabled, \
                CAST(created_at AS TEXT) AS created_at \
         FROM github_transition_rules \
         WHERE workspace_id = $1 AND trigger_event = $2 AND enabled = $3 \
         ORDER BY close_intent_required DESC \
         LIMIT 1",
        workspace_id,
        trigger_event,
        true
    )?;
    Ok(row.map(|r| ResolvedRule::Target(r.target_status_category)))
}

/// Whether another closing PR on the issue is still open or in draft.
///
/// The merge status applies when the *last* such PR merges, so an open sibling
/// holds it back.
fn has_other_open_closing_pr(links: &[schema::GitHubLink], this: &schema::GitHubLink) -> bool {
    links.iter().any(|other| {
        !is_same_pr(other, this)
            && matches!(other.state.as_deref(), Some("open" | "draft"))
            && other.kind().is_ok_and(|k| behavior(k).completes)
    })
}

/// Whether a different closing PR on the issue already merged.
fn has_other_merged_closing_pr(links: &[schema::GitHubLink], this: &schema::GitHubLink) -> bool {
    links.iter().any(|other| {
        !is_same_pr(other, this)
            && other.state.as_deref() == Some("merged")
            && other.kind().is_ok_and(|k| behavior(k).completes)
    })
}

fn is_same_pr(a: &schema::GitHubLink, b: &schema::GitHubLink) -> bool {
    a.repo_full_name == b.repo_full_name && a.ref_identifier == b.ref_identifier
}

// ─── Token Management ──────────────────────────────────────────────────────

/// Accept both GitHub's RFC 3339 timestamps and PostgreSQL's text representation.
pub(crate) fn parse_token_expiry(
    value: &str,
) -> chrono::ParseResult<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
}

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
    repository: &str,
) -> trakkt_core::Result<String> {
    let current = schema::get_installation_by_id(db, &installation.installation_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound("GitHub connection no longer exists".into()))?;
    if !current.is_active() {
        return Err(trakkt_core::Error::Forbidden(
            "GitHub connection is inactive".into(),
        ));
    }
    if current.token_generation != installation.token_generation {
        return Err(trakkt_core::Error::Conflict(
            "GitHub connection changed; retry the event".into(),
        ));
    }
    let installation = &current;
    // Check if the cached token is still valid (with 5-minute buffer).
    if let Some(ref encrypted_token) = installation.access_token_encrypted
        && let Some(ref expires_str) = installation.token_expires_at
        && let Ok(expires_at) = parse_token_expiry(expires_str)
    {
        let buffer = chrono::Duration::minutes(5);
        if chrono::Utc::now() < expires_at - buffer {
            return trakkt_auth::encryption::decrypt(encrypted_token, encryption_key);
        }
    }

    // Token expired or missing — request a fresh one.
    let github_installation_id = installation.github_installation_id as u64;
    let outbound = schema::admit_outbound(db, installation, repository).await?;
    let token_response = github_client
        .request_installation_token(github_installation_id)
        .await?;
    outbound.commit().await?;

    // Encrypt and persist the new token.
    let encrypted = trakkt_auth::encryption::encrypt(&token_response.token, encryption_key)?;
    if !schema::cache_installation_token(db, installation, &encrypted, &token_response.expires_at)
        .await?
    {
        return Err(trakkt_core::Error::Conflict(
            "GitHub connection changed during token refresh".into(),
        ));
    }

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
/// updated GitHub links. Finds all linked issues for the PR, decides each one
/// from its link kind and the rules (see the module docs), updates statuses,
/// and posts bot comments (best-effort) for the ones that changed.
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
    let Some(current) = schema::get_installation_by_id(db, &installation.installation_id).await?
    else {
        return Ok(());
    };
    if current.workspace_id != workspace_id
        || !payload
            .pointer("/repository/full_name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|repo| current.allows_repository(repo))
    {
        return Ok(());
    }
    let installation = &current;
    // Extract PR details from payload.
    let pr = match payload.get("pull_request") {
        Some(pr) => pr,
        None => return Ok(()),
    };

    let pr_number = match pr.get("number").and_then(|v| v.as_i64()) {
        Some(n) => n,
        None => return Ok(()),
    };

    let repo = match payload
        .pointer("/repository/full_name")
        .and_then(|v| v.as_str())
    {
        Some(r) => r.to_string(),
        None => return Ok(()),
    };

    let merged = pr.get("merged").and_then(|v| v.as_bool()).unwrap_or(false);
    let draft = pr.get("draft").and_then(|v| v.as_bool()).unwrap_or(false);

    // Determine the automation step.
    let trigger = match determine_trigger(action, merged, draft) {
        Some(t) => t,
        None => return Ok(()),
    };

    // Find all PR links for this PR in this workspace.
    let links =
        schema::list_pr_links_by_ref(db, workspace_id, &repo, &pr_number.to_string()).await?;

    if links.is_empty() {
        return Ok(());
    }

    let linkback = schema::linkback_enabled(db, workspace_id).await?;
    // The installation token is only needed for the linkback comment, so it is
    // fetched on the first status change that wants one, not for every event.
    let mut token: Option<Option<String>> = None;

    for link in &links {
        let kind = link.kind()?;
        if !kind_may_fire(kind, trigger) {
            continue;
        }

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

        // The team's rule, else the workspace default.
        let target_category = match resolve_rule(db, workspace_id, &issue.team_id, trigger).await? {
            Some(ResolvedRule::Target(category)) => category,
            Some(ResolvedRule::NoAction) | None => continue,
        };

        // Completion steps wait for the last open closing PR.
        if matches!(trigger, "pr_merged" | "pr_closed") {
            let siblings = schema::list_pr_links_for_issue(db, &link.issue_id).await?;
            if has_other_open_closing_pr(&siblings, link) {
                tracing::debug!(issue_id = %link.issue_id, trigger = %trigger,
                    "Another closing PR is still open — not completing yet");
                continue;
            }
            // Closing without merging is not a merge. If a sibling already
            // merged, this close has nothing to say about the issue.
            if trigger == "pr_closed" && has_other_merged_closing_pr(&siblings, link) {
                continue;
            }
        }

        // Pure decision: kind, forward-only, completed/cancelled guard.
        let Some(target_category) = decide(kind, trigger, &issue.status_category, &target_category)
        else {
            tracing::debug!(
                issue_id = %link.issue_id,
                status = %issue.status_name,
                trigger = %trigger,
                kind = %kind.as_str(),
                "Leaving the issue alone"
            );
            continue;
        };

        // Resolve the target status from the category.
        let target_status = match trakkt_auth::status_service::get_status_by_category(
            db,
            workspace_id,
            target_category,
        )
        .await?
        {
            Some(s) => s,
            None => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    category = %target_category,
                    "No status found in target category for transition rule — skipping"
                );
                continue;
            }
        };
        if issue.status_id == target_status.status_id {
            continue;
        }

        // Apply the status update via the issue service.
        let updates = trakkt_types::models::IssueUpdate {
            status_id: Some(target_status.status_id.clone()),
            ..Default::default()
        };

        #[cfg(test)]
        schema::mutation_pause::wait(&installation.installation_id).await;
        let admission = schema::ConnectionAdmission {
            installation,
            repository: &repo,
        };
        match trakkt_auth::issue_service::update_issue_with_admission(
            db,
            workspace_id,
            &issue.team_key,
            issue.number,
            &updates,
            None, // No actor user — system-initiated
            trakkt_types::enums::ActionSource::Github,
            Some("GitHub"),
            ws_manager,
            Some(&admission),
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

        // Post bot comment on the PR (best-effort), only for a real change.
        if !linkback {
            continue;
        }
        if token.is_none() {
            token = Some(
                match get_installation_token(db, github_client, installation, encryption_key, &repo)
                    .await
                {
                    Ok(t) => Some(t),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            installation_id = %installation.installation_id,
                            "Failed to get installation token for bot comments — transitions still apply"
                        );
                        None
                    }
                },
            );
        }
        if let Some(Some(ref token)) = token {
            let outbound = match schema::admit_outbound(db, installation, &repo).await {
                Ok(outbound) => outbound,
                Err(_) => return Ok(()),
            };
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
            outbound.commit().await?;
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
    // Find all closing PR links for this issue.
    let links = schema::list_closing_pr_links_for_issue(db, issue_id).await?;

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

        // Skip inactive connections and repositories outside their current access scope.
        if !installation.allows_repository(&link.repo_full_name) {
            tracing::debug!(
                installation_id = %installation.installation_id,
                "Connection is inactive or repository is outside access scope — skipping outbound notification"
            );
            continue;
        }

        // Get an installation token.
        let token = match get_installation_token(
            db,
            github_client,
            &installation,
            encryption_key,
            &link.repo_full_name,
        )
        .await
        {
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

        let outbound = match schema::admit_outbound(db, &installation, &link.repo_full_name).await {
            Ok(outbound) => outbound,
            Err(_) => continue,
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
        outbound.commit().await?;
    }

    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_completed_and_cancelled_are_left_alone() {
        assert!(is_closed_category("completed"));
        assert!(is_closed_category("cancelled"));
        for open in ["backlog", "unstarted", "started"] {
            assert!(!is_closed_category(open), "{open} is still open work");
        }
    }

    #[test]
    fn determine_trigger_maps_every_lifecycle_action() {
        // (action, merged, draft) -> step
        let cases = [
            ("opened", false, false, Some("pr_opened")),
            ("opened", false, true, Some("pr_draft")),
            ("reopened", false, false, Some("pr_opened")),
            ("reopened", false, true, Some("pr_draft")),
            ("ready_for_review", false, false, Some("pr_opened")),
            ("converted_to_draft", false, true, Some("pr_draft")),
            ("closed", true, false, Some("pr_merged")),
            ("closed", false, false, Some("pr_closed")),
            ("closed", false, true, Some("pr_closed")),
            ("synchronize", false, false, None),
            ("edited", false, false, None),
        ];
        for (action, merged, draft, expected) in cases {
            assert_eq!(
                determine_trigger(action, merged, draft),
                expected,
                "{action} merged={merged} draft={draft}"
            );
        }
    }

    /// Every link kind x step x starting category, against the rule defaults
    /// (opened/ready -> started, merged -> completed, closed -> cancelled).
    #[test]
    fn decide_table() {
        use LinkKind::*;
        let categories = ["backlog", "unstarted", "started", "completed", "cancelled"];
        // (kind, step, target) -> the category each starting category moves to
        let table: [(LinkKind, &str, &str, [Option<&str>; 5]); 16] = [
            (
                Closing,
                "pr_opened",
                "started",
                [Some("started"), Some("started"), None, None, None],
            ),
            (
                Closing,
                "pr_merged",
                "completed",
                [
                    Some("completed"),
                    Some("completed"),
                    Some("completed"),
                    None,
                    None,
                ],
            ),
            (
                Closing,
                "pr_closed",
                "cancelled",
                [
                    Some("cancelled"),
                    Some("cancelled"),
                    Some("cancelled"),
                    None,
                    None,
                ],
            ),
            (
                Closing,
                "pr_draft",
                "unstarted",
                [Some("unstarted"), None, None, None, None],
            ),
            (
                Contributing,
                "pr_opened",
                "started",
                [Some("started"), Some("started"), None, None, None],
            ),
            (Contributing, "pr_merged", "completed", [None; 5]),
            (Contributing, "pr_closed", "cancelled", [None; 5]),
            (
                BranchTitle,
                "pr_opened",
                "started",
                [Some("started"), Some("started"), None, None, None],
            ),
            (BranchTitle, "pr_merged", "completed", [None; 5]),
            (BranchTitle, "pr_closed", "cancelled", [None; 5]),
            (
                BranchTitle,
                "pr_draft",
                "unstarted",
                [Some("unstarted"), None, None, None, None],
            ),
            (Related, "pr_opened", "started", [None; 5]),
            (Related, "pr_merged", "completed", [None; 5]),
            (Related, "pr_closed", "cancelled", [None; 5]),
            (Related, "pr_draft", "unstarted", [None; 5]),
            (
                Contributing,
                "pr_draft",
                "unstarted",
                [Some("unstarted"), None, None, None, None],
            ),
        ];
        for (kind, step, target, expected) in table {
            for (current, want) in categories.iter().zip(expected) {
                assert_eq!(
                    decide(kind, step, current, target),
                    want,
                    "{kind:?} {step} from {current}"
                );
            }
        }
    }

    #[test]
    fn decide_never_moves_backwards_or_within_a_category() {
        // In Review and In Progress are both `started`: a later event must not
        // pull an issue back to In Progress.
        assert_eq!(
            decide(LinkKind::Closing, "pr_opened", "started", "started"),
            None
        );
        assert_eq!(
            decide(LinkKind::Closing, "pr_draft", "started", "backlog"),
            None
        );
        assert_eq!(
            decide(LinkKind::Closing, "pr_opened", "unstarted", "backlog"),
            None
        );
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
