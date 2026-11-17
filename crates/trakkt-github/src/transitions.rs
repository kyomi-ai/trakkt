// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic issue status transitions triggered by GitHub events.
//!
//! A PR event moves a linked issue only when all of these hold, in order:
//! 1. The event is an automation step (draft, opened/ready, merged, closed).
//! 2. The link's [`LinkKind`] is allowed to fire that step ([`behavior`]).
//!    Merge and close are settled across all of the issue's PRs instead of
//!    fired by one link ([`Run::settle_issue`]).
//! 3. A rule exists for the step: the issue's team override, else the
//!    workspace default. A team override with no target means "no action".
//! 4. The move is forward, and the issue is not completed or cancelled
//!    ([`decide`]). GitHub never reopens finished work or moves work backwards.
//! 5. For merge and close, no other working-link PR on the issue is still
//!    open: the merge status applies when the last one is done.
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
use crate::schema::{self, GitHubInstallation, GitHubTransitionRule};
use trakkt_types::enums::{GitHubAutomationStep, LinkKind, StatusCategory};

// ─── Trigger Event Mapping ─────────────────────────────────────────────────

/// What a webhook event means for the automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// Fire a step's rule for the PR's links (draft, opened/ready).
    Step(GitHubAutomationStep),
    /// The PR finished or changed what it links. Completion is decided from
    /// every PR on the issue, so each linked issue is re-evaluated.
    Settle,
}

/// Map a webhook action and the PR's draft flag to an event.
///
/// Returns `None` for actions that are nothing to the automation
/// (`synchronize`, ...).
fn determine_event(action: &str, draft: bool) -> Option<Event> {
    match action {
        // A reopened PR starts the issue like an opened one; forward-only
        // keeps that from undoing anything.
        "opened" | "reopened" if draft => Some(Event::Step(GitHubAutomationStep::PrDraft)),
        "opened" | "reopened" | "ready_for_review" => {
            Some(Event::Step(GitHubAutomationStep::PrOpened))
        }
        "converted_to_draft" => Some(Event::Step(GitHubAutomationStep::PrDraft)),
        // Merged or closed without merge: either may be the last open PR.
        // `edited` can drop or downgrade a link that was holding a merge back.
        "closed" | "edited" => Some(Event::Settle),
        _ => None,
    }
}

/// What a link of a given kind is allowed to do. This is the single place the
/// per-kind behaviour lives; changing what a kind does is changing one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KindBehavior {
    /// A working link: may fire the draft and opened/ready steps, and while its
    /// PR is open it holds back the completion of the issue's other PRs.
    working: bool,
    /// Its merge may complete the issue.
    completes: bool,
    /// Its PR closed without merging may cancel the issue (when nothing else
    /// is linked). Only an explicit closing word says the PR was the work.
    cancels_when_abandoned: bool,
}

const fn behavior(kind: LinkKind) -> KindBehavior {
    match kind {
        // Shown on the issue and the PR, nothing more.
        LinkKind::Related => KindBehavior {
            working: false,
            completes: false,
            cancels_when_abandoned: false,
        },
        // Linear's model: the branch name and PR title are working links and
        // a merge completes the issue like a closing word.
        LinkKind::BranchTitle => KindBehavior {
            working: true,
            completes: true,
            cancels_when_abandoned: false,
        },
        LinkKind::Contributing => KindBehavior {
            working: true,
            completes: false,
            cancels_when_abandoned: false,
        },
        LinkKind::Closing => KindBehavior {
            working: true,
            completes: true,
            cancels_when_abandoned: true,
        },
    }
}

/// Whether a link of this kind may fire this step. Merge and close are not
/// steps a single link fires: see [`Run::settle_issue`].
fn kind_may_fire(kind: LinkKind, step: GitHubAutomationStep) -> bool {
    match step {
        GitHubAutomationStep::PrDraft | GitHubAutomationStep::PrOpened => behavior(kind).working,
        GitHubAutomationStep::PrMerged => behavior(kind).completes,
        GitHubAutomationStep::PrClosed => behavior(kind).cancels_when_abandoned,
    }
}

/// The pure part of the decision: given the issue's current category and the
/// rule's target, which category (if any) to move to.
///
/// Completed and cancelled issues are never moved: reopening finished or
/// abandoned work is a decision for a person, not a side effect of a PR that
/// happens to name the ticket. And a move is forward only, so the same
/// category never moves either (In Review is never pulled back to In Progress).
fn decide(current: StatusCategory, target: StatusCategory) -> Option<StatusCategory> {
    (!current.is_closed() && target.rank() > current.rank()).then_some(target)
}

// ─── Rule Matching ─────────────────────────────────────────────────────────

/// The target the issue's team has configured for a step.
enum ResolvedRule {
    /// Move to this status category.
    Target(StatusCategory),
    /// The step does nothing for this issue.
    NoAction,
}

fn parse_category(value: &str) -> trakkt_core::Result<StatusCategory> {
    StatusCategory::parse(value).ok_or_else(|| {
        trakkt_core::Error::Internal(format!("unknown status category '{value}' in a rule"))
    })
}

/// Find the rule for a step: the issue's team override, else the enabled
/// workspace default. `None` means nothing is configured, so nothing happens.
async fn resolve_rule(
    db: &DbPool,
    workspace_id: &str,
    team_id: &str,
    step: GitHubAutomationStep,
) -> trakkt_core::Result<Option<ResolvedRule>> {
    if let Some(team_rule) = schema::get_team_transition_rule(db, team_id, step).await? {
        return Ok(Some(match team_rule.target_status_category {
            Some(category) => ResolvedRule::Target(parse_category(&category)?),
            None => ResolvedRule::NoAction,
        }));
    }
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubTransitionRule,
        "SELECT rule_id, workspace_id, trigger_event, target_status_category, enabled, \
                CAST(created_at AS TEXT) AS created_at \
         FROM github_transition_rules \
         WHERE workspace_id = $1 AND trigger_event = $2 AND enabled = $3",
        workspace_id,
        step.as_str(),
        true
    )?;
    row.map(|r| parse_category(&r.target_status_category).map(ResolvedRule::Target))
        .transpose()
}

/// A PR link on an issue with its parsed kind.
struct PrLink {
    link: schema::GitHubLink,
    kind: LinkKind,
}

impl PrLink {
    fn is_open(&self) -> bool {
        matches!(self.link.state.as_deref(), Some("open" | "draft"))
    }

    fn is_merged(&self) -> bool {
        self.link.state.as_deref() == Some("merged")
    }
}

/// Every PR link on an issue that can still hear from GitHub.
///
/// A link in a repository the connection no longer reaches (removed from the
/// installation, disconnected, uninstalled) can never receive its merge or
/// close event, so its stored `open` state is not evidence of anything and
/// must not hold back a completion.
async fn reachable_pr_links(db: &DbPool, issue_id: &str) -> trakkt_core::Result<Vec<PrLink>> {
    let mut installations: std::collections::HashMap<String, Option<GitHubInstallation>> =
        std::collections::HashMap::new();
    let mut reachable = Vec::new();
    for link in schema::list_pr_links_for_issue(db, issue_id).await? {
        if !installations.contains_key(&link.installation_id) {
            let installation = schema::get_installation_by_id(db, &link.installation_id).await?;
            installations.insert(link.installation_id.clone(), installation);
        }
        let live = installations[&link.installation_id]
            .as_ref()
            .is_some_and(|i| i.allows_repository(&link.repo_full_name));
        if live {
            let kind = link.kind()?;
            reachable.push(PrLink { link, kind });
        }
    }
    Ok(reachable)
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

/// Everything one webhook event needs while moving issues.
struct Run<'a> {
    db: &'a DbPool,
    github_client: &'a GitHubClient,
    installation: &'a GitHubInstallation,
    workspace_id: &'a str,
    repo: &'a str,
    pr_number: i64,
    encryption_key: &'a [u8; 32],
    ws_manager: Option<&'a trakkt_auth::websocket::WebSocketManager>,
    base_url: &'a str,
    linkback: bool,
    /// The installation token, fetched on the first comment that needs one.
    /// `Some(None)` means the fetch failed and is not retried for this event.
    token: Option<Option<String>>,
    /// Set when the connection stopped admitting outbound requests; the rest
    /// of the event still moves issues, it just stops commenting.
    comments_blocked: bool,
}

impl Run<'_> {
    /// Move an issue to a status category and say so on the PR.
    ///
    /// The update is a compare-and-set on the status the decision was made
    /// from: if a person moved the issue in the meantime, nothing is written.
    async fn move_issue(
        &mut self,
        issue: &trakkt_types::models::IssueWithDetails,
        target: StatusCategory,
        label: &str,
    ) -> trakkt_core::Result<()> {
        let Some(target_status) = trakkt_auth::status_service::get_status_by_category(
            self.db,
            self.workspace_id,
            target.as_str(),
        )
        .await?
        else {
            tracing::warn!(
                workspace_id = %self.workspace_id,
                category = %target,
                "No status found in target category for transition rule — skipping"
            );
            return Ok(());
        };
        if issue.status_id == target_status.status_id {
            return Ok(());
        }

        let updates = trakkt_types::models::IssueUpdate {
            status_id: Some(target_status.status_id.clone()),
            expected_status_id: Some(issue.status_id.clone()),
            ..Default::default()
        };

        #[cfg(test)]
        schema::mutation_pause::wait(&self.installation.installation_id).await;
        let admission = schema::ConnectionAdmission {
            installation: self.installation,
            repository: self.repo,
        };
        match trakkt_auth::issue_service::update_issue_with_admission(
            self.db,
            self.workspace_id,
            &issue.team_key,
            issue.number,
            &updates,
            None, // No actor user — system-initiated
            trakkt_types::enums::ActionSource::Github,
            Some("GitHub"),
            self.ws_manager,
            Some(&admission),
        )
        .await
        {
            Ok(_) => {
                tracing::info!(
                    issue_key = %issue.team_key,
                    issue_number = issue.number,
                    step = %label,
                    target_status = %target_status.name,
                    "Applied automatic status transition"
                );
            }
            Err(trakkt_core::Error::Conflict(_)) => {
                tracing::debug!(
                    issue_id = %issue.issue_id,
                    "Issue status changed while the automation decided — leaving it alone"
                );
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(
                    issue_id = %issue.issue_id,
                    error = %e,
                    "Failed to apply status transition"
                );
                return Ok(());
            }
        }

        self.post_linkback(issue, &target_status.name, label).await
    }

    /// Post the bot comment on the PR (best-effort): only for a real change,
    /// and only if the workspace has linkback on.
    async fn post_linkback(
        &mut self,
        issue: &trakkt_types::models::IssueWithDetails,
        target_status_name: &str,
        label: &str,
    ) -> trakkt_core::Result<()> {
        if !self.linkback || self.comments_blocked {
            return Ok(());
        }
        if self.token.is_none() {
            self.token = Some(
                match get_installation_token(
                    self.db,
                    self.github_client,
                    self.installation,
                    self.encryption_key,
                    self.repo,
                )
                .await
                {
                    Ok(t) => Some(t),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            installation_id = %self.installation.installation_id,
                            "Failed to get installation token for bot comments — transitions still apply"
                        );
                        None
                    }
                },
            );
        }
        let Some(Some(token)) = &self.token else {
            return Ok(());
        };
        let outbound = match schema::admit_outbound(self.db, self.installation, self.repo).await {
            Ok(outbound) => outbound,
            Err(e) => {
                // The connection stopped admitting requests. Issues still move
                // (each update is admitted on its own); only comments stop.
                tracing::warn!(error = %e, "Outbound comments are no longer admitted for this event");
                self.comments_blocked = true;
                return Ok(());
            }
        };
        let comment_body = format_transition_comment(
            label,
            &issue.team_key,
            issue.number,
            &issue.title,
            target_status_name,
            self.base_url,
        );
        if let Err(e) = self
            .github_client
            .create_comment(token, self.repo, self.pr_number as u64, &comment_body)
            .await
        {
            tracing::warn!(
                error = %e,
                repo = %self.repo,
                pr_number = self.pr_number,
                "Failed to post bot comment on PR — transition was applied successfully"
            );
        }
        outbound.commit().await
    }

    /// Re-evaluate an issue against all of its PRs after one of them finished
    /// or changed what it links.
    ///
    /// While any working-link PR is open nothing completes: the merge status
    /// applies when the *last* one is done. Then, if a completing PR merged,
    /// the issue is completed, whichever event lets go of it: a sibling merging,
    /// being closed without merge, or being edited so it no longer links the
    /// issue. A PR closed without merging is never a merge on its own; it
    /// cancels the issue only if it is `abandoned_by` (this event's PR), says
    /// it closes the issue, and nothing else linked has merged.
    async fn settle_issue(
        &mut self,
        issue_id: &str,
        abandoned_by: Option<(&str, &str)>,
    ) -> trakkt_core::Result<()> {
        let siblings = reachable_pr_links(self.db, issue_id).await?;
        if siblings
            .iter()
            .any(|l| l.is_open() && behavior(l.kind).working)
        {
            return Ok(());
        }
        let step = if siblings
            .iter()
            .any(|l| l.is_merged() && behavior(l.kind).completes)
        {
            GitHubAutomationStep::PrMerged
        } else if let Some((repo, number)) = abandoned_by
            && siblings.iter().any(|l| {
                l.link.repo_full_name == repo
                    && l.link.ref_identifier == number
                    && behavior(l.kind).cancels_when_abandoned
            })
            && !siblings
                .iter()
                .any(|l| l.is_merged() && behavior(l.kind).working)
        {
            GitHubAutomationStep::PrClosed
        } else {
            return Ok(());
        };
        let Some(issue) = trakkt_auth::issue_service::get_issue_by_id(self.db, issue_id).await?
        else {
            return Ok(());
        };
        let Some(ResolvedRule::Target(target)) =
            resolve_rule(self.db, self.workspace_id, &issue.team_id, step).await?
        else {
            return Ok(());
        };
        let current = parse_category(&issue.status_category)?;
        let Some(target) = decide(current, target) else {
            tracing::debug!(issue_id, status = %issue.status_name, "Leaving the issue alone");
            return Ok(());
        };
        self.move_issue(&issue, target, step.as_str()).await
    }
}

/// Apply automatic status transitions after a PR event is processed.
///
/// Called from the webhook handler after `process_pull_request` has created/
/// updated GitHub links. For a draft or opened event, each linked issue is
/// decided from its link kind and the rules; for a merge, close or edit, each
/// linked issue (and each issue the edit unlinked, `released_issue_ids`) is
/// settled against all of its PRs. See the module docs and
/// docs/ARCHITECTURE.md ("GitHub Link Intent").
pub async fn apply_transition_rules(
    db: &DbPool,
    github_client: &GitHubClient,
    installation: &GitHubInstallation,
    workspace_id: &str,
    action: &str,
    payload: &serde_json::Value,
    released_issue_ids: &[String],
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
    let Some(pr) = payload.get("pull_request") else {
        return Ok(());
    };
    let Some(pr_number) = pr.get("number").and_then(|v| v.as_i64()) else {
        return Ok(());
    };
    let Some(repo) = payload
        .pointer("/repository/full_name")
        .and_then(|v| v.as_str())
    else {
        return Ok(());
    };
    let draft = pr.get("draft").and_then(|v| v.as_bool()).unwrap_or(false);
    let merged = pr.get("merged").and_then(|v| v.as_bool()).unwrap_or(false);
    let Some(event) = determine_event(action, draft) else {
        return Ok(());
    };

    // Find all PR links for this PR in this workspace.
    let links =
        schema::list_pr_links_by_ref(db, workspace_id, repo, &pr_number.to_string()).await?;
    if links.is_empty() && released_issue_ids.is_empty() {
        return Ok(());
    }

    let mut run = Run {
        db,
        github_client,
        installation,
        workspace_id,
        repo,
        pr_number,
        encryption_key,
        ws_manager,
        base_url,
        linkback: schema::linkback_enabled(db, workspace_id).await?,
        token: None,
        comments_blocked: false,
    };

    match event {
        Event::Step(step) => {
            for link in &links {
                if !kind_may_fire(link.kind()?, step) {
                    continue;
                }
                let Some(issue) =
                    trakkt_auth::issue_service::get_issue_by_id(db, &link.issue_id).await?
                else {
                    tracing::warn!(issue_id = %link.issue_id, "Linked issue not found — skipping transition");
                    continue;
                };
                let Some(ResolvedRule::Target(target)) =
                    resolve_rule(db, workspace_id, &issue.team_id, step).await?
                else {
                    continue;
                };
                let current = parse_category(&issue.status_category)?;
                let Some(target) = decide(current, target) else {
                    tracing::debug!(
                        issue_id = %link.issue_id,
                        status = %issue.status_name,
                        step = %step.as_str(),
                        "Leaving the issue alone"
                    );
                    continue;
                };
                run.move_issue(&issue, target, step.as_str()).await?;
            }
        }
        Event::Settle => {
            let mut issue_ids: Vec<&str> = links.iter().map(|l| l.issue_id.as_str()).collect();
            issue_ids.extend(released_issue_ids.iter().map(String::as_str));
            issue_ids.sort_unstable();
            issue_ids.dedup();
            let pr_ref = pr_number.to_string();
            // Closed without merging: this PR is what may cancel the issue.
            let abandoned = (action == "closed" && !merged).then_some((repo, pr_ref.as_str()));
            for issue_id in issue_ids {
                run.settle_issue(issue_id, abandoned).await?;
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
    // The PRs that would have completed this issue, by the same definition the
    // automation uses.
    let mut links = Vec::new();
    for link in schema::list_pr_links_for_issue(db, issue_id).await? {
        if behavior(link.kind()?).completes {
            links.push(link);
        }
    }
    let Some(first) = links.first() else {
        return Ok(());
    };
    // The linkback switch covers every comment the integration posts on a PR.
    if !schema::linkback_enabled(db, &first.workspace_id).await? {
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

    use GitHubAutomationStep::*;
    use StatusCategory::*;

    #[test]
    fn determine_event_maps_every_lifecycle_action() {
        // (action, draft) -> event
        let cases = [
            ("opened", false, Some(Event::Step(PrOpened))),
            ("opened", true, Some(Event::Step(PrDraft))),
            ("reopened", false, Some(Event::Step(PrOpened))),
            ("reopened", true, Some(Event::Step(PrDraft))),
            ("ready_for_review", false, Some(Event::Step(PrOpened))),
            ("converted_to_draft", true, Some(Event::Step(PrDraft))),
            ("closed", false, Some(Event::Settle)),
            ("edited", false, Some(Event::Settle)),
            ("synchronize", false, None),
        ];
        for (action, draft, expected) in cases {
            assert_eq!(
                determine_event(action, draft),
                expected,
                "{action} draft={draft}"
            );
        }
    }

    /// Link kind x step x starting category, against the rule defaults, for the
    /// steps a single link fires (draft, opened/ready). Completion is settled
    /// across PRs and is tested end to end.
    #[test]
    fn kind_gate_and_decide_table() {
        use LinkKind::*;
        let categories = [Backlog, Unstarted, Started, Completed, Cancelled];
        // (kind, step, target) -> the category each starting category moves to
        let table: [(
            LinkKind,
            GitHubAutomationStep,
            StatusCategory,
            [Option<StatusCategory>; 5],
        ); 8] = [
            (
                Closing,
                PrOpened,
                Started,
                [Some(Started), Some(Started), None, None, None],
            ),
            (
                Closing,
                PrDraft,
                Unstarted,
                [Some(Unstarted), None, None, None, None],
            ),
            (
                Contributing,
                PrOpened,
                Started,
                [Some(Started), Some(Started), None, None, None],
            ),
            (
                Contributing,
                PrDraft,
                Unstarted,
                [Some(Unstarted), None, None, None, None],
            ),
            (
                BranchTitle,
                PrOpened,
                Started,
                [Some(Started), Some(Started), None, None, None],
            ),
            (
                BranchTitle,
                PrDraft,
                Unstarted,
                [Some(Unstarted), None, None, None, None],
            ),
            (Related, PrOpened, Started, [None; 5]),
            (Related, PrDraft, Unstarted, [None; 5]),
        ];
        for (kind, step, target, expected) in table {
            for (current, want) in categories.iter().zip(expected) {
                let got = kind_may_fire(kind, step)
                    .then(|| decide(*current, target))
                    .flatten();
                assert_eq!(got, want, "{kind:?} {step:?} from {current}");
            }
        }
    }

    #[test]
    fn which_kinds_complete_and_which_cancel() {
        for kind in [LinkKind::Related, LinkKind::Contributing] {
            assert!(!kind_may_fire(kind, PrMerged), "{kind:?} must not complete");
        }
        for kind in [LinkKind::Closing, LinkKind::BranchTitle] {
            assert!(kind_may_fire(kind, PrMerged), "{kind:?} completes on merge");
        }
        // Only an explicit closing word lets an abandoned PR cancel the issue.
        for kind in [
            LinkKind::Related,
            LinkKind::Contributing,
            LinkKind::BranchTitle,
        ] {
            assert!(!kind_may_fire(kind, PrClosed), "{kind:?} must not cancel");
        }
        assert!(kind_may_fire(LinkKind::Closing, PrClosed));
    }

    #[test]
    fn decide_never_moves_backwards_or_within_a_category() {
        // In Review and In Progress are both `started`: a later event must not
        // pull an issue back to In Progress.
        assert_eq!(decide(Started, Started), None);
        assert_eq!(decide(Started, Backlog), None);
        assert_eq!(decide(Unstarted, Backlog), None);
        assert_eq!(decide(Started, Completed), Some(Completed));
    }

    #[test]
    fn decide_leaves_completed_and_cancelled_alone() {
        for finished in [Completed, Cancelled] {
            for target in StatusCategory::all() {
                assert_eq!(
                    decide(finished, *target),
                    None,
                    "{finished:?} -> {target:?}"
                );
            }
        }
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
