// SPDX-License-Identifier: AGPL-3.0-or-later

//! Signed HTTP deliveries against isolated migrated SQLite databases. Outgoing
//! GitHub requests use loopback-only clients with redirects disabled.

pub mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use trakkt_core::{
    db_execute, db_fetch_scalar,
    test_helpers::{seed_team, seed_user, seed_workspace},
};
use trakkt_github::schema::{self, GitHubInstallation};
use trakkt_types::enums::{GitHubAutomationStep, StatusCategory};
use trakkt_server::state::AppState;

const WORKSPACE: &str = "github-workspace";
const FOREIGN: &str = "foreign-workspace";

static TEST_KEY: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    use rsa::pkcs8::EncodePrivateKey;
    rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048)
        .expect("generate isolated RSA fixture key")
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("encode generated fixture key")
        .as_bytes()
        .to_vec()
});

struct Fixture {
    state: AppState,
    router: Router,
    secret: String,
    issue: String,
    foreign_issue: String,
    personal: GitHubInstallation,
    organization: GitHubInstallation,
}

async fn issue(state: &AppState, workspace: &str, team: &str) -> String {
    trakkt_auth::issue_service::create_issue(
        &state.db,
        &trakkt_types::models::CreateIssueParams {
            workspace_id: workspace.into(),
            team_id: team.into(),
            creator_id: "github-owner".into(),
            title: "Identical KEY-1 in separate workspaces".into(),
            description: None,
            priority: 0,
            assignee_id: None,
            due_date: None,
            label_ids: Vec::new(),
            project_id: None,
            milestone_id: None,
            estimate: None,
        },
        None,
    )
    .await
    .expect("create issue through allocation and sync service")
    .issue_id
}

async fn fixture() -> Fixture {
    let state = common::test_state().await;
    seed_user(&state.db, "github-owner", "github-owner@example.test")
        .await
        .expect("seed owner");
    for (workspace, team) in [(WORKSPACE, "github-team"), (FOREIGN, "foreign-team")] {
        seed_workspace(&state.db, workspace, "github-owner")
            .await
            .expect("seed workspace");
        seed_team(&state.db, team, workspace, "KEY")
            .await
            .expect("seed identical team key");
        trakkt_auth::status_service::seed_default_statuses(&state.db, workspace)
            .await
            .expect("seed statuses");
    }
    let local = issue(&state, WORKSPACE, "github-team").await;
    let foreign = issue(&state, FOREIGN, "foreign-team").await;
    // Match the route's environment preference without changing process-global
    // environment or touching a configured external service.
    let secret = std::env::var("GITHUB_WEBHOOK_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "test-webhook-secret-not-real".into());
    let encrypted = trakkt_auth::encryption::encrypt(&secret, &state.encryption_key)
        .expect("encrypt fixture webhook secret");
    let app = schema::create_github_app(
        &state.db,
        42,
        "fixture-app",
        "fixture-client",
        "unused",
        "unused",
        &encrypted,
    )
    .await
    .expect("configure local webhook secret");
    let mut installations = Vec::new();
    for (github_id, account, kind) in [
        (101, "fixture-personal", "User"),
        (202, "fixture-org", "Organization"),
    ] {
        let inst = schema::create_installation(
            &state.db,
            WORKSPACE,
            &app.github_app_id,
            github_id,
            account,
            kind,
            Some(&json!([format!("{account}/repo")])),
        )
        .await
        .expect("seed separate installation identity");
        db_execute!(&state.db,
            "UPDATE github_installations SET authorization_verified_at = CURRENT_TIMESTAMP, github_account_id = $1 WHERE installation_id = $2",
            github_id, &inst.installation_id).expect("model already verified GitHub authorization");
        installations.push(
            schema::get_installation_by_id(&state.db, &inst.installation_id)
                .await
                .expect("read verified installation")
                .expect("installation exists"),
        );
    }
    let personal = installations.remove(0);
    let organization = installations.remove(0);
    let router = Router::new()
        .nest("/webhooks", trakkt_server::routes::github::routes())
        .with_state(state.clone());
    Fixture {
        state,
        router,
        secret,
        issue: local,
        foreign_issue: foreign,
        personal,
        organization,
    }
}

// RFC 2104 HMAC, using the SHA-256 dependency already exposed by the server.
// This independent signer does not reuse production verification code.
fn signature(secret: &str, body: &[u8]) -> String {
    let mut key = [0u8; 64];
    if secret.len() > 64 {
        key[..32].copy_from_slice(&Sha256::digest(secret.as_bytes()));
    } else {
        key[..secret.len()].copy_from_slice(secret.as_bytes());
    }
    let mut inner = Sha256::new();
    inner.update(key.map(|byte| byte ^ 0x36));
    inner.update(body);
    let mut outer = Sha256::new();
    outer.update(key.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    format!("sha256={:x}", outer.finalize())
}

async fn deliver(f: &Fixture, event: &str, delivery: &str, payload: Value) {
    let body = serde_json::to_vec(&payload).expect("serialize signed fixture payload");
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhooks/github")
                .header("x-github-event", event)
                .header("x-github-delivery", delivery)
                .header("x-hub-signature-256", signature(&f.secret, &body))
                .body(Body::from(body))
                .expect("construct signed request"),
        )
        .await
        .expect("dispatch webhook through real route");
    assert_eq!(response.status(), StatusCode::OK, "delivery {delivery}");
}

fn pr(inst: &GitHubInstallation, number: i64, action: &str) -> Value {
    json!({"action":action,"installation":{"id":inst.github_installation_id},
        "repository":{"full_name":format!("{}/repo",inst.account_login)},
        "pull_request":{"number":number,"title":"Fix KEY-1","body":"Closes KEY-1",
            "head":{"ref":"feature/KEY-1"},"base":{"ref":"main"},"merged":action=="closed",
            "html_url":format!("https://github.com/{}/repo/pull/{number}",inst.account_login),"user":{"login":"fixture-author"}}})
}

fn push(inst: &GitHubInstallation) -> Value {
    json!({"installation":{"id":inst.github_installation_id},"repository":{"full_name":format!("{}/repo",inst.account_login)},
        "ref":"refs/heads/feature/KEY-1","created":true,"commits":[{"id":"fixture-sha","message":"Fix KEY-1","url":"https://github.com/fixture/repo/commit/fixture-sha"}]})
}

async fn links(f: &Fixture, id: &str) -> Vec<schema::GitHubLink> {
    schema::list_links_for_issue(&f.state.db, id)
        .await
        .expect("read retained links")
}

#[tokio::test]
async fn signed_pr_and_push_deliveries_route_each_installation_only_to_its_workspace() {
    let f = fixture().await;
    deliver(
        &f,
        "pull_request",
        "personal-pr",
        pr(&f.personal, 1, "opened"),
    )
    .await;
    deliver(&f, "push", "org-push", push(&f.organization)).await;
    let local = links(&f, &f.issue).await;
    assert_eq!(local.len(), 3);
    assert_eq!(
        local
            .iter()
            .filter(|l| l.installation_id == f.personal.installation_id)
            .count(),
        1
    );
    assert_eq!(
        local
            .iter()
            .filter(|l| l.installation_id == f.organization.installation_id)
            .count(),
        2
    );
    assert!(local.iter().all(|l| l.workspace_id == WORKSPACE));
    assert!(links(&f, &f.foreign_issue).await.is_empty());
}

#[tokio::test]
async fn disconnected_history_survives_and_unsuspend_never_revives_incoming_processing() {
    let f = fixture().await;
    deliver(
        &f,
        "pull_request",
        "before-disconnect",
        pr(&f.personal, 1, "opened"),
    )
    .await;
    schema::disconnect_installation(&f.state.db, &f.personal.installation_id, WORKSPACE)
        .await
        .expect("disconnect one account");
    for action in ["suspend", "unsuspend"] {
        deliver(
            &f,
            "installation",
            action,
            json!({"action":action,"installation":{"id":101}}),
        )
        .await;
    }
    deliver(
        &f,
        "pull_request",
        "disconnected-merge",
        pr(&f.personal, 1, "closed"),
    )
    .await;
    deliver(&f, "push", "disconnected-push", push(&f.personal)).await;
    let history = links(&f, &f.issue).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state.as_deref(), Some("open"));
    let current = schema::get_installation_by_id(&f.state.db, &f.personal.installation_id)
        .await
        .expect("read disconnected account")
        .expect("history installation retained");
    assert!(current.disconnected_at.is_some());
    assert!(current.suspended_at.is_none());
    assert!(!current.is_active());
    deliver(
        &f,
        "pull_request",
        "other-still-functional",
        pr(&f.organization, 2, "opened"),
    )
    .await;
    assert_eq!(links(&f, &f.issue).await.len(), 2);
    assert!(links(&f, &f.foreign_issue).await.is_empty());
}

#[tokio::test]
async fn repository_removal_selected_empty_and_uninstall_gate_only_the_target_connection() {
    let f = fixture().await;
    schema::update_installation_token(
        &f.state.db,
        &f.personal.installation_id,
        "fixture-encrypted-token",
        "2030-01-01T00:00:00Z",
    )
    .await
    .expect("seed cached token");
    deliver(&f, "installation_repositories", "remove-last-repo", json!({"action":"removed","installation":{"id":101},
        "repository_selection":"selected","repositories_removed":[{"full_name":"fixture-personal/repo"}]})).await;
    let current = schema::get_installation_by_id(&f.state.db, &f.personal.installation_id)
        .await
        .expect("read narrowed selection")
        .expect("connection exists");
    assert_eq!(current.repository_selection, "selected");
    assert_eq!(
        serde_json::from_str::<Value>(
            current
                .target_repos
                .as_deref()
                .expect("selected-empty stored")
        )
        .expect("parse repository selection"),
        json!([])
    );
    assert!(current.access_token_encrypted.is_none());
    deliver(
        &f,
        "pull_request",
        "removed-repo-pr",
        pr(&f.personal, 1, "opened"),
    )
    .await;
    deliver(&f, "push", "removed-repo-push", push(&f.personal)).await;
    assert!(links(&f, &f.issue).await.is_empty());
    deliver(
        &f,
        "installation",
        "uninstall-org",
        json!({"action":"deleted","installation":{"id":202}}),
    )
    .await;
    deliver(
        &f,
        "installation",
        "unsuspend-uninstalled",
        json!({"action":"unsuspend","installation":{"id":202}}),
    )
    .await;
    deliver(
        &f,
        "pull_request",
        "uninstalled-pr",
        pr(&f.organization, 2, "opened"),
    )
    .await;
    assert!(links(&f, &f.issue).await.is_empty());
    let removed = schema::get_installation_by_id(&f.state.db, &f.organization.installation_id)
        .await
        .expect("read uninstall lifecycle")
        .expect("uninstalled history retained");
    assert!(removed.uninstalled_at.is_some());
    assert!(current.uninstalled_at.is_none());
}

#[tokio::test]
async fn processed_legacy_delivery_stays_deduplicated_and_new_event_repairs_missing_link() {
    let f = fixture().await;
    let event = schema::create_event(
        &f.state.db,
        "legacy-ignored",
        None,
        "pull_request",
        Some("opened"),
        None,
    )
    .await
    .expect("record historical ignored delivery");
    schema::mark_event_processed(&f.state.db, &event)
        .await
        .expect("model historical processed delivery");
    deliver(
        &f,
        "pull_request",
        "legacy-ignored",
        pr(&f.personal, 60, "opened"),
    )
    .await;
    assert!(links(&f, &f.issue).await.is_empty());
    deliver(
        &f,
        "pull_request",
        "new-synchronize",
        pr(&f.personal, 60, "synchronize"),
    )
    .await;
    assert_eq!(links(&f, &f.issue).await.len(), 1);
    deliver(
        &f,
        "pull_request",
        "new-synchronize",
        pr(&f.personal, 60, "synchronize"),
    )
    .await;
    assert_eq!(links(&f, &f.issue).await.len(), 1);
    let count = db_fetch_scalar!(
        &f.state.db,
        i64,
        "SELECT COUNT(*) FROM github_events WHERE github_delivery_id = $1",
        "new-synchronize"
    )
    .expect("count durable delivery records");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn rejected_settings_sync_rolls_back_disconnect_and_signed_lifecycle_mutation() {
    let f = fixture().await;
    let before = schema::list_installations_for_workspace(&f.state.db, WORKSPACE)
        .await
        .expect("snapshot connections");
    trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts_of_type(
        &f.state.db,
        "GITHUB_INSTALLATION",
    )
    .await;
    schema::disconnect_installation(&f.state.db, &f.personal.installation_id, WORKSPACE)
        .await
        .expect_err("disconnect must reject a failed durable settings entry");
    deliver(
        &f,
        "installation",
        "rejected-suspend",
        json!({"action":"suspend","installation":{"id":202}}),
    )
    .await;
    let after = schema::list_installations_for_workspace(&f.state.db, WORKSPACE)
        .await
        .expect("snapshot rolled back connections");
    assert_eq!(
        serde_json::to_value(before).expect("serialize prior state"),
        serde_json::to_value(after).expect("serialize rolled back state")
    );
    let error: Option<String> = db_fetch_scalar!(
        &f.state.db,
        Option<String>,
        "SELECT error FROM github_events WHERE github_delivery_id = $1",
        "rejected-suspend"
    )
    .expect("read durable failed event outcome");
    assert!(error.is_some());
}

#[tokio::test]
async fn invalid_signature_cannot_record_or_mutate_a_connection() {
    let f = fixture().await;
    let body = serde_json::to_vec(&pr(&f.personal, 1, "opened")).expect("serialize rejected event");
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhooks/github")
                .header("x-github-event", "pull_request")
                .header("x-github-delivery", "invalid-signature")
                .header(
                    "x-hub-signature-256",
                    signature("wrong-fixture-secret", &body),
                )
                .body(Body::from(body))
                .expect("construct invalid signature request"),
        )
        .await
        .expect("dispatch rejected signature");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !schema::event_exists(&f.state.db, "invalid-signature")
            .await
            .expect("read dedup ledger")
    );
    assert!(links(&f, &f.issue).await.is_empty());
}

/// Exercise the outgoing HTTP boundary as well as the incoming signed route.
/// The client constructor refuses non-loopback URLs and disables redirects.
/// Put the ticket back in Backlog the way a person would. A GitHub event never
/// reopens finished work, so a test that needs it open again has to say so.
async fn reopen_by_hand(f: &Fixture) {
    move_by_hand(f, "backlog").await;
}

#[tokio::test]
async fn shared_merge_rules_use_each_account_token_and_disconnected_events_send_nothing() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let foreign_before = trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.foreign_issue)
        .await
        .expect("read foreign ticket")
        .expect("foreign issue exists")
        .status_id;
    let started =
        trakkt_auth::status_service::get_status_by_category(&f.state.db, WORKSPACE, "started")
            .await
            .expect("resolve started status")
            .expect("started status seeded")
            .status_id;
    let completed =
        trakkt_auth::status_service::get_status_by_category(&f.state.db, WORKSPACE, "completed")
            .await
            .expect("resolve completed status")
            .expect("completed status seeded")
            .status_id;
    for (inst, number) in [(&f.personal, 1), (&f.organization, 2)] {
        if number == 2 {
            // The ticket is Done from the first PR's merge. Opening another PR
            // that names it is not a decision to reopen it.
            let comment_count = || {
                stub.requests()
                    .iter()
                    .filter(|(path, _)| path.ends_with("/comments"))
                    .count()
            };
            let comments_before = comment_count();
            deliver(
                &f,
                "pull_request",
                "outbound-open-2-on-finished-work",
                pr(inst, number, "opened"),
            )
            .await;
            assert_eq!(
                trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
                    .await
                    .expect("read finished ticket")
                    .expect("issue exists")
                    .status_id,
                completed,
                "a PR opened on a Done ticket must leave it Done"
            );
            assert_eq!(
                comment_count(),
                comments_before,
                "nothing moved, so nothing is announced on the PR"
            );
            reopen_by_hand(&f).await;
        }
        deliver(
            &f,
            "pull_request",
            &format!("outbound-open-{number}"),
            pr(inst, number, "opened"),
        )
        .await;
        assert_eq!(
            trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
                .await
                .expect("read transitioned ticket")
                .expect("issue exists")
                .status_id,
            started
        );
        let opened_history =
            trakkt_auth::activity_service::list_issue_activities(&f.state.db, &f.issue)
                .await
                .expect("read PR-open status history");
        let opened = opened_history
            .iter()
            .rev()
            .find(|a| a.action_type == "status_changed")
            .expect("opening a PR records its automatic status change");
        assert_eq!(
            opened.old_value.as_deref(),
            Some("Backlog"),
            "opening a PR on an open ticket records the automatic move"
        );
        assert_eq!(opened.new_value.as_deref(), Some("In Progress"));
        assert_eq!(
            opened.action_source,
            trakkt_types::enums::ActionSource::Github
        );
        assert_eq!(opened.action_source_label.as_deref(), Some("GitHub"));
        assert!(opened.actor_id.is_none());
        deliver(
            &f,
            "pull_request",
            &format!("outbound-merge-{number}"),
            pr(inst, number, "closed"),
        )
        .await;
        assert_eq!(
            trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
                .await
                .expect("read merged ticket")
                .expect("issue exists")
                .status_id,
            completed
        );
    }
    let history = trakkt_auth::activity_service::list_issue_activities(&f.state.db, &f.issue)
        .await
        .expect("read merge status history");
    let status_history: Vec<_> = history
        .iter()
        .filter(|a| a.action_type == "status_changed")
        .collect();
    assert_eq!(
        status_history.len(),
        5,
        "an open and a merge for each PR, plus the manual reopen between them"
    );
    assert_eq!(
        status_history
            .last()
            .expect("merge history")
            .new_value
            .as_deref(),
        Some("Done")
    );
    let before_disconnect = stub.requests();
    let comments: Vec<_> = before_disconnect
        .iter()
        .filter(|(path, _)| path.ends_with("/comments"))
        .cloned()
        .collect();
    assert_eq!(
        comments,
        vec![
            (
                "/repos/fixture-personal/repo/issues/1/comments".into(),
                "Bearer fixture-personal-token".into()
            ),
            (
                "/repos/fixture-personal/repo/issues/1/comments".into(),
                "Bearer fixture-personal-token".into()
            ),
            (
                "/repos/fixture-org/repo/issues/2/comments".into(),
                "Bearer fixture-org-token".into()
            ),
            (
                "/repos/fixture-org/repo/issues/2/comments".into(),
                "Bearer fixture-org-token".into()
            ),
        ]
    );
    for id in [101, 202] {
        assert_eq!(
            before_disconnect
                .iter()
                .filter(|(path, _)| path == &format!("/app/installations/{id}/access_tokens"))
                .count(),
            1,
            "separate account token is cached"
        );
    }
    schema::disconnect_installation(&f.state.db, &f.personal.installation_id, WORKSPACE)
        .await
        .expect("disconnect only personal account");
    deliver(
        &f,
        "pull_request",
        "outbound-disconnected-open",
        pr(&f.personal, 3, "opened"),
    )
    .await;
    deliver(
        &f,
        "pull_request",
        "outbound-disconnected-merge",
        pr(&f.personal, 3, "closed"),
    )
    .await;
    assert_eq!(
        stub.requests(),
        before_disconnect
    );
    assert_eq!(
        trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
            .await
            .expect("read unchanged ticket")
            .expect("issue exists")
            .status_id,
        completed
    );
    assert_eq!(
        links(&f, &f.issue).await.len(),
        2,
        "historical links remain, disconnected PR adds none"
    );
    reopen_by_hand(&f).await;
    deliver(
        &f,
        "pull_request",
        "outbound-org-still-works",
        pr(&f.organization, 4, "opened"),
    )
    .await;
    assert_eq!(
        trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
            .await
            .expect("read functional account transition")
            .expect("issue exists")
            .status_id,
        started
    );
    assert_eq!(
        stub.requests()
            .last()
            .expect("other account posts comment"),
        &(
            "/repos/fixture-org/repo/issues/4/comments".to_string(),
            "Bearer fixture-org-token".to_string()
        )
    );
    assert_eq!(
        trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.foreign_issue)
            .await
            .expect("read isolated foreign ticket")
            .expect("foreign ticket exists")
            .status_id,
        foreign_before
    );
    assert!(links(&f, &f.foreign_issue).await.is_empty());
    stub.stop().await;
}

#[tokio::test]
async fn all_to_selected_without_client_quarantines_scope_and_other_account_works() {
    let f = fixture().await;
    schema::update_installation_repos(&f.state.db, &f.personal.installation_id, None)
        .await
        .expect("set verified all-repository scope");
    let mut payload = pr(&f.personal, 8, "opened");
    payload["repository"]["full_name"] = json!("fixture-personal/another-repo");
    deliver(&f, "pull_request", "all-repos-new-repo", payload).await;
    assert_eq!(links(&f, &f.issue).await.len(), 1);
    deliver(&f, "installation_repositories", "all-to-selected-empty", json!({"action":"removed","installation":{"id":101},"repository_selection":"selected","repositories_removed":[]})).await;
    let pending = schema::get_installation_by_id(&f.state.db, &f.personal.installation_id)
        .await
        .expect("read failed refresh state")
        .expect("connection retained");
    assert!(pending.repository_scope_pending);
    assert!(!pending.is_active());
    let error: Option<String> = db_fetch_scalar!(
        &f.state.db,
        Option<String>,
        "SELECT error FROM github_events WHERE github_delivery_id = $1",
        "all-to-selected-empty"
    )
    .expect("read actionable refresh failure");
    assert!(
        error
            .expect("missing client records actionable error")
            .contains("reconnect")
    );
    let mut blocked = pr(&f.personal, 9, "opened");
    blocked["repository"]["full_name"] = json!("fixture-personal/another-repo");
    deliver(&f, "pull_request", "selected-empty-blocked", blocked).await;
    assert_eq!(
        links(&f, &f.issue).await.len(),
        1,
        "empty selection never inherits all-repository access"
    );
    deliver(
        &f,
        "pull_request",
        "selected-empty-other-account",
        pr(&f.organization, 10, "opened"),
    )
    .await;
    assert_eq!(links(&f, &f.issue).await.len(), 2);
}

#[tokio::test]
async fn signed_all_to_selected_fetches_retained_repositories_and_cannot_revive_after_disconnect() {
    use axum::{
        Json,
        extract::State,
        http::{HeaderMap, Uri},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct ScopeFixture {
        block: AtomicBool,
        reached: tokio::sync::mpsc::Sender<()>,
        release: tokio::sync::Notify,
    }
    async fn api(
        State(state): State<Arc<ScopeFixture>>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Json<Value> {
        match uri.path() {
            "/app/installations/101" => Json(
                json!({"id":101,"app_id":42,"account":{"id":101,"login":"fixture-personal","type":"User"},
                "target_type":"User","permissions":{},"events":[],"repository_selection":"selected","suspended_at":null}),
            ),
            "/app/installations/101/access_tokens" | "/app/installations/202/access_tokens" => {
                Json(
                    json!({"token":"fixture-scope-token","expires_at":(chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()}),
                )
            }
            "/installation/repositories" => {
                assert_eq!(
                    headers
                        .get("authorization")
                        .expect("repository query needs account token"),
                    "Bearer fixture-scope-token"
                );
                let page = uri.query().expect("explicit repository pagination");
                let name = if page == "per_page=100&page=1" {
                    if state.block.load(Ordering::SeqCst) {
                        state
                            .reached
                            .send(())
                            .await
                            .expect("signal in-flight repository snapshot");
                        state.release.notified().await;
                    }
                    "retained-repo"
                } else {
                    assert_eq!(page, "per_page=100&page=2");
                    "newly-selected-repo"
                };
                Json(
                    json!({"total_count":2,"repositories":[{"full_name":format!("fixture-personal/{name}"),"name":name,"private":true}]}),
                )
            }
            path => panic!("scope fixture rejects unexpected GitHub request: {path}"),
        }
    }
    let (reached, mut observed) = tokio::sync::mpsc::channel(1);
    let scenario = Arc::new(ScopeFixture {
        block: AtomicBool::new(false),
        reached,
        release: tokio::sync::Notify::new(),
    });
    let mut f = fixture().await;
    let (shutdown, server) = serve_github(Router::new().fallback(api).with_state(scenario.clone()), &mut f).await;
    schema::update_installation_repos(&f.state.db, &f.personal.installation_id, None)
        .await
        .expect("start with all repositories");
    let payload = json!({"action":"removed","installation":{"id":101},"repository_selection":"selected",
        "repositories_removed":[{"full_name":"fixture-personal/repo"}],"repositories_added":[]});
    deliver(
        &f,
        "installation_repositories",
        "authoritative-selection",
        payload.clone(),
    )
    .await;
    let selected = schema::get_installation_by_id(&f.state.db, &f.personal.installation_id)
        .await
        .expect("read authoritative scope")
        .expect("connection retained");
    assert!(!selected.repository_scope_pending);
    assert_eq!(selected.repository_selection, "selected");
    assert_eq!(
        serde_json::from_str::<Value>(
            selected
                .target_repos
                .as_deref()
                .expect("selected scope stored")
        )
        .expect("parse authoritative scope"),
        json!([
            "fixture-personal/newly-selected-repo",
            "fixture-personal/retained-repo"
        ])
    );
    let mut retained = pr(&f.personal, 20, "opened");
    retained["repository"]["full_name"] = json!("fixture-personal/retained-repo");
    deliver(&f, "pull_request", "retained-repo-active", retained.clone()).await;
    deliver(
        &f,
        "pull_request",
        "removed-repo-inactive",
        pr(&f.personal, 21, "opened"),
    )
    .await;
    assert_eq!(links(&f, &f.issue).await.len(), 1);
    schema::update_installation_repos(&f.state.db, &f.personal.installation_id, None)
        .await
        .expect("prepare a second all-to-selected reconciliation");
    scenario.block.store(true, Ordering::SeqCst);
    let body = serde_json::to_vec(&payload).expect("serialize concurrent signed scope delivery");
    let request = Request::builder()
        .method("POST")
        .uri("/webhooks/github")
        .header("x-github-event", "installation_repositories")
        .header("x-github-delivery", "stale-selection")
        .header("x-hub-signature-256", signature(&f.secret, &body))
        .body(Body::from(body))
        .expect("build concurrent scope request");
    let pending = tokio::spawn(f.router.clone().oneshot(request));
    tokio::time::timeout(std::time::Duration::from_secs(10), observed.recv())
        .await
        .expect("scope fetch reaches barrier")
        .expect("barrier signal exists");
    let quarantined = schema::get_installation_by_id(&f.state.db, &f.personal.installation_id)
        .await
        .expect("read in-flight scope")
        .expect("connection exists");
    assert!(quarantined.repository_scope_pending);
    assert!(!quarantined.is_active());
    retained["pull_request"]["number"] = json!(22);
    deliver(&f, "pull_request", "scope-pending-pr-blocked", retained).await;
    assert_eq!(links(&f, &f.issue).await.len(), 1);
    schema::disconnect_installation(&f.state.db, &f.personal.installation_id, WORKSPACE)
        .await
        .expect("disconnect while network snapshot is in flight");
    scenario.release.notify_one();
    let response =
        trakkt_core::test_helpers::task::join_soon(pending, "stale signed scope reconciliation")
            .await
            .expect("signed route returns response");
    assert_eq!(response.status(), StatusCode::OK);
    let disconnected = schema::get_installation_by_id(&f.state.db, &f.personal.installation_id)
        .await
        .expect("read stale response outcome")
        .expect("connection retained");
    assert!(disconnected.disconnected_at.is_some());
    assert!(disconnected.repository_scope_pending);
    assert!(disconnected.token_generation > quarantined.token_generation);
    assert_eq!(
        disconnected.target_repos, quarantined.target_repos,
        "late snapshot must not write scope after generation changes"
    );
    assert!(!disconnected.is_active());
    deliver(
        &f,
        "pull_request",
        "scope-race-other-account",
        pr(&f.organization, 23, "opened"),
    )
    .await;
    assert_eq!(links(&f, &f.issue).await.len(), 2);
    shutdown.send(()).expect("shutdown scope API fixture");
    trakkt_core::test_helpers::task::join_soon(server, "scope API graceful shutdown").await;
}

// ─── TRA-10115: link and automate by intent ─────────────────────────────────
//
// These drive the signed webhook route end to end against a loopback GitHub
// stub, because the incident was a webhook delivery that moved tickets. Every
// case reads the ticket's status back through the issue service.

/// Serve `api` on a loopback port as the GitHub API, point the fixture's GitHub
/// client at it, and rebuild the webhook router around the new client. Returns
/// the shutdown signal and the server task; stop with both.
async fn serve_github(
    api: Router,
    f: &mut Fixture,
) -> (tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback GitHub fixture");
    let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, api)
            .with_graceful_shutdown(async {
                stopped.await.expect("receive fixture shutdown");
            })
            .await
            .expect("serve loopback GitHub fixture");
    });
    f.state.github_client = Some(std::sync::Arc::new(
        trakkt_github::GitHubClient::for_test_endpoint(42, &TEST_KEY, &endpoint)
            .expect("build loopback-only GitHub client"),
    ));
    f.router = Router::new()
        .nest("/webhooks", trakkt_server::routes::github::routes())
        .with_state(f.state.clone());
    (shutdown, server)
}

/// A loopback GitHub that answers token, comment and pull request requests. It records every
/// request (path and credentials) and every comment body, so a test can assert
/// what was, and was not, sent to GitHub and with whose token.
struct GithubStub {
    requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    comments: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pulls: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (String, bool)>>>,
    shutdown: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

impl GithubStub {
    async fn attach(f: &mut Fixture) -> Self {
        use axum::{Json, extract::State, http::HeaderMap};
        use std::sync::{Arc, Mutex};
        type Requests = Arc<Mutex<Vec<(String, String)>>>;
        type Comments = Arc<Mutex<Vec<String>>>;
        type Pulls = Arc<Mutex<std::collections::HashMap<String, (String, bool)>>>;
        async fn answer(
            State((requests, comments, pulls)): State<(Requests, Comments, Pulls)>,
            uri: axum::http::Uri,
            headers: HeaderMap,
            body: String,
        ) -> Json<Value> {
            let path = uri.path().to_owned();
            let authorization = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .expect("all GitHub requests carry credentials")
                .to_owned();
            requests
                .lock()
                .expect("lock request ledger")
                .push((path.clone(), authorization));
            if path.ends_with("/access_tokens") {
                let token = match path.as_str() {
                    "/app/installations/101/access_tokens" => "fixture-personal-token",
                    "/app/installations/202/access_tokens" => "fixture-org-token",
                    _ => panic!("unexpected installation token request: {path}"),
                };
                return Json(json!({"token":token,"expires_at":
                    (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()}));
            }
            if let Some((_, number)) = path.split_once("/pulls/") {
                // What GitHub says about a PR: open unless the test set it.
                let (state, merged) = pulls
                    .lock()
                    .expect("read pull request states")
                    .get(&path)
                    .cloned()
                    .unwrap_or(("open".to_string(), false));
                return Json(json!({"number":number.parse::<u64>().unwrap_or(0),"title":"stub","state":state,
                    "merged":merged,"draft":false,"html_url":"https://github.com/stub","head":{"ref":"stub","sha":"stub"}}));
            }
            assert!(
                path.ends_with("/comments"),
                "unexpected outbound mutation: {path}"
            );
            let parsed: Value = serde_json::from_str(&body).expect("comment body is JSON");
            comments
                .lock()
                .expect("lock comment ledger")
                .push(parsed["body"].as_str().unwrap_or_default().to_owned());
            Json(json!({"id":1}))
        }
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let comments: Comments = Arc::new(Mutex::new(Vec::new()));
        let pulls: Pulls = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let api = Router::new()
            .fallback(answer)
            .with_state((requests.clone(), comments.clone(), pulls.clone()));
        let (shutdown, server) = serve_github(api, f).await;
        schema::seed_default_transition_rules(&f.state.db, WORKSPACE)
            .await
            .expect("seed default workspace rules");
        Self {
            requests,
            comments,
            pulls,
            shutdown,
            server,
        }
    }

    /// Every request GitHub received: (path, authorization header).
    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().expect("read request ledger").clone()
    }

    /// Tell the stub what GitHub reports for a PR from now on.
    fn set_pull(&self, repo: &str, number: i64, state: &str, merged: bool) {
        self.pulls
            .lock()
            .expect("set pull request state")
            .insert(
                format!("/repos/{repo}/pulls/{number}"),
                (state.to_string(), merged),
            );
    }

    fn comments(&self) -> Vec<String> {
        self.comments.lock().expect("read comment ledger").clone()
    }

    async fn stop(self) {
        self.shutdown.send(()).expect("stop GitHub stub listener");
        trakkt_core::test_helpers::task::join_soon(self.server, "GitHub stub shutdown").await;
    }
}

/// What a pull request says about itself.
struct Pr<'a> {
    number: i64,
    title: &'a str,
    body: &'a str,
    branch: &'a str,
    base: &'a str,
    draft: bool,
}

impl<'a> Pr<'a> {
    fn new(number: i64, title: &'a str, body: &'a str, branch: &'a str) -> Self {
        Self {
            number,
            title,
            body,
            branch,
            base: "main",
            draft: false,
        }
    }

    fn event(&self, inst: &GitHubInstallation, action: &str, merged: bool) -> Value {
        json!({"action":action,"installation":{"id":inst.github_installation_id},
            "repository":{"full_name":format!("{}/repo",inst.account_login)},
            "pull_request":{"number":self.number,"title":self.title,"body":self.body,
                "head":{"ref":self.branch},"base":{"ref":self.base},"merged":merged,
                "draft":self.draft,
                "html_url":format!("https://github.com/{}/repo/pull/{}",inst.account_login,self.number),
                "user":{"login":"fixture-author"}}})
    }
}

async fn status_category(f: &Fixture, issue_id: &str) -> String {
    trakkt_auth::issue_service::get_issue_by_id(&f.state.db, issue_id)
        .await
        .expect("read ticket status")
        .expect("issue exists")
        .status_category
}

async fn send(f: &Fixture, delivery: &str, pr: &Pr<'_>, action: &str, merged: bool) {
    deliver(f, "pull_request", delivery, pr.event(&f.personal, action, merged)).await;
}

#[tokio::test]
async fn a_pull_request_that_only_mentions_a_ticket_never_changes_it_or_comments() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    let mention = Pr::new(
        10,
        "Tidy the settings page",
        "This is unrelated to, but was noticed while reading, KEY-1.",
        "feature/settings-tidy",
    );
    send(&f, "mention-open", &mention, "opened", false).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "backlog",
        "a bare mention in the body started the ticket"
    );
    send(&f, "mention-merge", &mention, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    assert!(
        stub.comments().is_empty(),
        "a mention posted a status comment: {:?}",
        stub.comments()
    );
    stub.stop().await;
}

fn status_comments(comments: &[String]) -> usize {
    comments.iter().filter(|c| c.contains("Status moved")).count()
}

#[tokio::test]
async fn fixes_starts_on_open_and_completes_on_merge_and_says_so_once_each() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(11, "Tidy", "Fixes KEY-1", "feature/tidy");
    send(&f, "fixes-open", &pr, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    send(&f, "fixes-merge", &pr, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    let comments = stub.comments();
    assert_eq!(comments.len(), 2, "one comment per real change: {comments:?}");
    assert_eq!(status_comments(&comments), 1);
    stub.stop().await;
}

#[tokio::test]
async fn refs_starts_the_ticket_but_a_merge_never_completes_it() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(12, "Groundwork", "Refs KEY-1", "feature/groundwork");
    send(&f, "refs-open", &pr, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    send(&f, "refs-merge", &pr, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    stub.stop().await;
}

#[tokio::test]
async fn related_to_links_the_ticket_for_display_and_changes_nothing() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(13, "Sibling work", "Related to KEY-1", "feature/sibling");
    send(&f, "related-open", &pr, "opened", false).await;
    send(&f, "related-merge", &pr, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    let linked = links(&f, &f.issue).await;
    assert_eq!(linked.len(), 1, "the relation is still shown on the issue");
    assert_eq!(linked[0].link_kind, "related");
    assert!(stub.comments().is_empty());
    stub.stop().await;
}

#[tokio::test]
async fn ignore_suppresses_even_a_branch_name_link() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(14, "Rework", "Ignore KEY-1", "feature/KEY-1-rework");
    send(&f, "ignore-open", &pr, "opened", false).await;
    send(&f, "ignore-merge", &pr, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    assert!(links(&f, &f.issue).await.is_empty(), "skip means no link at all");
    assert!(stub.comments().is_empty());
    stub.stop().await;
}

#[tokio::test]
async fn a_branch_name_links_and_starts_without_any_magic_word() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(15, "Tidy", "", "feature/KEY-1-tidy");
    send(&f, "branch-open", &pr, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    assert_eq!(links(&f, &f.issue).await[0].link_kind, "branch_title");
    stub.stop().await;
}

#[tokio::test]
async fn two_closing_prs_complete_the_ticket_only_after_both_merge() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let first = Pr::new(20, "Part one", "Fixes KEY-1", "feature/one");
    let second = Pr::new(21, "Part two", "Fixes KEY-1", "feature/two");
    send(&f, "two-open-1", &first, "opened", false).await;
    send(&f, "two-open-2", &second, "opened", false).await;
    send(&f, "two-merge-1", &first, "closed", true).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "started",
        "the other closing PR is still open"
    );
    send(&f, "two-merge-2", &second, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    stub.stop().await;
}

#[tokio::test]
async fn a_closing_pr_closed_without_merging_cancels_the_ticket_only_when_nothing_else_is_linked() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(22, "Abandoned", "Fixes KEY-1", "feature/abandoned");
    send(&f, "closed-open", &pr, "opened", false).await;
    send(&f, "closed-close", &pr, "closed", false).await;
    // The existing workspace rule (closed -> cancelled) applies, and under the
    // Completed/Cancelled guard that is final: a later PR cannot undo it.
    assert_eq!(status_category(&f, &f.issue).await, "cancelled");
    assert_eq!(stub.comments().len(), 2, "{:?}", stub.comments());
    let replacement = Pr::new(23, "Replacement", "Fixes KEY-1", "feature/replacement");
    send(&f, "closed-replacement-open", &replacement, "opened", false).await;
    send(&f, "closed-replacement-merge", &replacement, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "cancelled");
    stub.stop().await;
}

#[tokio::test]
async fn a_pr_closed_without_merging_leaves_the_ticket_alone_unless_it_alone_said_it_closes_it() {
    // Contributing, branch/title and related PRs never cancel.
    for (label, body, branch) in [
        ("refs", "Refs KEY-1", "feature/x"),
        ("branch", "", "feature/KEY-1-x"),
        ("related", "Related to KEY-1", "feature/y"),
    ] {
        let mut f = fixture().await;
        let stub = GithubStub::attach(&mut f).await;
        let pr = Pr::new(24, "Work", body, branch);
        send(&f, &format!("{label}-open"), &pr, "opened", false).await;
        let after_open = status_category(&f, &f.issue).await;
        send(&f, &format!("{label}-close"), &pr, "closed", false).await;
        assert_eq!(status_category(&f, &f.issue).await, after_open, "{label}");
        stub.stop().await;
    }
    // A closing PR abandoned while a sibling is still open leaves the ticket.
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let a = Pr::new(25, "One", "Fixes KEY-1", "feature/a");
    let b = Pr::new(26, "Two", "Fixes KEY-1", "feature/b");
    send(&f, "sibling-open-a", &a, "opened", false).await;
    send(&f, "sibling-open-b", &b, "opened", false).await;
    send(&f, "sibling-close-a", &a, "closed", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    stub.stop().await;
}

#[tokio::test]
async fn an_open_contributing_or_branch_pr_holds_back_a_closing_merge_but_a_mention_does_not() {
    for (label, body, branch, holds) in [
        ("contributing", "Refs KEY-1", "feature/x", true),
        ("branch", "", "feature/KEY-1-x", true),
        ("related", "Related to KEY-1", "feature/y", false),
    ] {
        let mut f = fixture().await;
        let stub = GithubStub::attach(&mut f).await;
        let sibling = Pr::new(27, "Sibling", body, branch);
        let closing = Pr::new(28, "Fix", "Fixes KEY-1", "feature/fix");
        send(&f, &format!("{label}-open-sibling"), &sibling, "opened", false).await;
        send(&f, &format!("{label}-open-fix"), &closing, "opened", false).await;
        send(&f, &format!("{label}-merge-fix"), &closing, "closed", true).await;
        let expected = if holds { "started" } else { "completed" };
        assert_eq!(status_category(&f, &f.issue).await, expected, "{label}");
        stub.stop().await;
    }
}

#[tokio::test]
async fn a_branch_or_title_link_completes_on_merge_like_a_closing_word() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(29, "KEY-1: tidy the page", "", "feature/tidy");
    send(&f, "title-open", &pr, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    send(&f, "title-merge", &pr, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    stub.stop().await;
}

#[tokio::test]
async fn a_draft_pr_does_not_start_the_ticket_and_marking_it_ready_does() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let mut pr = Pr::new(30, "Work in progress", "Fixes KEY-1", "feature/wip");
    pr.draft = true;
    send(&f, "draft-open", &pr, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    assert_eq!(links(&f, &f.issue).await[0].state.as_deref(), Some("draft"));
    pr.draft = false;
    send(&f, "draft-ready", &pr, "ready_for_review", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    assert_eq!(links(&f, &f.issue).await[0].state.as_deref(), Some("open"));
    stub.stop().await;
}

#[tokio::test]
async fn commits_only_ever_leave_activity_and_links_never_a_status_change() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let mut payload = push(&f.personal);
    payload["ref"] = json!("refs/heads/feature/unrelated");
    payload["commits"] = json!([
        {"id":"c1","message":"Fixes KEY-1","url":"https://github.com/fixture/repo/commit/c1"},
        {"id":"c2","message":"see KEY-1 for background","url":"https://github.com/fixture/repo/commit/c2"},
        {"id":"c3","message":"Ignore KEY-1","url":"https://github.com/fixture/repo/commit/c3"}
    ]);
    deliver(&f, "push", "commit-mentions", payload).await;
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    let mut kinds: Vec<(String, String)> = links(&f, &f.issue)
        .await
        .into_iter()
        .map(|l| (l.ref_identifier, l.link_kind))
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            ("c1".to_string(), "closing".to_string()),
            ("c2".to_string(), "related".to_string())
        ],
        "a skipped commit links nothing"
    );
    assert!(stub.comments().is_empty());
    stub.stop().await;
}

#[tokio::test]
async fn a_pr_in_the_other_connected_repository_that_mentions_the_ticket_changes_nothing() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(40, "Unrelated product work", "Mirrors KEY-1 over there", "feature/x");
    for (delivery, action, merged) in [("org-open", "opened", false), ("org-merge", "closed", true)] {
        deliver(&f, "pull_request", delivery, pr.event(&f.organization, action, merged)).await;
    }
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    assert!(stub.comments().is_empty());
    stub.stop().await;
}

#[tokio::test]
async fn editing_a_pr_to_add_or_drop_a_magic_word_changes_the_link_kind() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let mention = Pr::new(50, "Notes", "mentions KEY-1", "feature/notes");
    send(&f, "edit-open", &mention, "opened", false).await;
    assert_eq!(links(&f, &f.issue).await[0].link_kind, "related");
    let closing = Pr::new(50, "Notes", "Fixes KEY-1", "feature/notes");
    send(&f, "edit-up", &closing, "edited", false).await;
    assert_eq!(links(&f, &f.issue).await[0].link_kind, "closing");
    send(&f, "edit-merge", &closing, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    let nothing = Pr::new(51, "Other", "mentions KEY-1", "feature/other");
    send(&f, "edit-open-2", &nothing, "opened", false).await;
    let gone = Pr::new(51, "Other", "no longer mentions anything", "feature/other");
    send(&f, "edit-drop", &gone, "edited", false).await;
    assert!(
        links(&f, &f.issue)
            .await
            .iter()
            .all(|l| l.ref_identifier != "51"),
        "dropping the key drops the link"
    );
    stub.stop().await;
}

#[tokio::test]
async fn linkback_can_be_turned_off_without_stopping_the_status_change() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    schema::set_linkback_enabled(&f.state.db, WORKSPACE, false, None)
        .await
        .expect("turn linkback off");
    let pr = Pr::new(60, "Tidy", "Fixes KEY-1", "feature/quiet");
    send(&f, "quiet-open", &pr, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    assert!(stub.comments().is_empty(), "{:?}", stub.comments());
    stub.stop().await;
}

#[tokio::test]
async fn a_team_can_silence_or_retarget_a_step_and_the_workspace_default_returns_when_cleared() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let team = "github-team";
    schema::set_team_transition_rule(&f.state.db, WORKSPACE, team, GitHubAutomationStep::PrOpened, None, None)
        .await
        .expect("team opts out of the opened step");
    let pr = Pr::new(70, "Tidy", "Fixes KEY-1", "feature/team");
    send(&f, "team-open", &pr, "opened", false).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "backlog",
        "a no-action team rule beats the workspace default"
    );
    schema::set_team_transition_rule(&f.state.db, WORKSPACE, team, GitHubAutomationStep::PrOpened, Some(StatusCategory::Unstarted), None)
        .await
        .expect("team retargets the opened step");
    send(&f, "team-reopen", &pr, "reopened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "unstarted");
    schema::clear_team_transition_rule(&f.state.db, WORKSPACE, team, GitHubAutomationStep::PrOpened, None)
        .await
        .expect("remove the override");
    send(&f, "team-ready", &pr, "ready_for_review", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    stub.stop().await;
}

async fn link_state(f: &Fixture, issue_id: &str, number: &str) -> Option<String> {
    links(f, issue_id)
        .await
        .into_iter()
        .find(|l| l.link_type == "pull_request" && l.ref_identifier == number)
        .and_then(|l| l.state)
}

#[tokio::test]
async fn a_merge_held_back_by_an_open_sibling_completes_when_the_sibling_is_closed_unmerged() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let a = Pr::new(80, "Part one", "Fixes KEY-1", "feature/a");
    let b = Pr::new(81, "Part two", "Fixes KEY-1", "feature/b");
    send(&f, "abandon-open-a", &a, "opened", false).await;
    send(&f, "abandon-open-b", &b, "opened", false).await;
    send(&f, "abandon-merge-a", &a, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "started", "held by B");
    send(&f, "abandon-close-b", &b, "closed", false).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "completed",
        "B was abandoned, A merged and said it fixes the ticket"
    );
    stub.stop().await;
}

#[tokio::test]
async fn editing_a_merged_pr_leaves_it_merged_so_it_never_blocks_its_sibling() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let a = Pr::new(82, "Part one", "Fixes KEY-1", "feature/a");
    let b = Pr::new(83, "Part two", "Fixes KEY-1", "feature/b");
    send(&f, "edit-merged-open-a", &a, "opened", false).await;
    send(&f, "edit-merged-open-b", &b, "opened", false).await;
    send(&f, "edit-merged-merge-a", &a, "closed", true).await;
    // A typo fix on the merged PR. GitHub's payload says it is merged.
    let mut edited = a.event(&f.personal, "edited", true);
    edited["pull_request"]["state"] = json!("closed");
    deliver(&f, "pull_request", "edit-merged-edit-a", edited).await;
    assert_eq!(link_state(&f, &f.issue, "82").await.as_deref(), Some("merged"));
    // A stale open-looking delivery for the finished PR changes nothing either.
    deliver(
        &f,
        "pull_request",
        "edit-merged-stale-open-a",
        a.event(&f.personal, "opened", false),
    )
    .await;
    assert_eq!(link_state(&f, &f.issue, "82").await.as_deref(), Some("merged"));
    send(&f, "edit-merged-merge-b", &b, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    stub.stop().await;
}

#[tokio::test]
async fn a_merge_held_back_by_a_sibling_is_released_when_the_sibling_stops_closing_the_ticket() {
    for (label, body) in [("downgrade", "Related to KEY-1"), ("drop", "no ticket here"), ("skip", "Ignore KEY-1")] {
        let mut f = fixture().await;
        let stub = GithubStub::attach(&mut f).await;
        let a = Pr::new(84, "Part one", "Fixes KEY-1", "feature/a");
        let b = Pr::new(85, "Part two", "Fixes KEY-1", "feature/b");
        send(&f, &format!("{label}-open-a"), &a, "opened", false).await;
        send(&f, &format!("{label}-open-b"), &b, "opened", false).await;
        send(&f, &format!("{label}-merge-a"), &a, "closed", true).await;
        assert_eq!(status_category(&f, &f.issue).await, "started", "{label}: held by B");
        let reworded = Pr::new(85, "Part two", body, "feature/b");
        send(&f, &format!("{label}-edit-b"), &reworded, "edited", false).await;
        assert_eq!(
            status_category(&f, &f.issue).await,
            "completed",
            "{label}: B no longer works on the ticket, so A's merge completes it"
        );
        stub.stop().await;
    }
}

#[tokio::test]
async fn a_stale_open_pr_in_an_unreachable_repository_does_not_block_completion() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let elsewhere = Pr::new(86, "Other half", "Fixes KEY-1", "feature/elsewhere");
    deliver(
        &f,
        "pull_request",
        "unreachable-open",
        elsewhere.event(&f.organization, "opened", false),
    )
    .await;
    // The organization's connection goes away; its PR can never report again.
    schema::disconnect_installation(&f.state.db, &f.organization.installation_id, WORKSPACE)
        .await
        .expect("disconnect the organization connection");
    let here = Pr::new(87, "This half", "Fixes KEY-1", "feature/here");
    send(&f, "unreachable-open-here", &here, "opened", false).await;
    send(&f, "unreachable-merge-here", &here, "closed", true).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "completed",
        "the disconnected repository's PR cannot hold the merge forever"
    );
    stub.stop().await;
}

#[tokio::test]
async fn a_status_update_decided_from_an_earlier_read_never_overwrites_a_newer_status() {
    let f = fixture().await;
    let started =
        trakkt_auth::status_service::get_status_by_category(&f.state.db, WORKSPACE, "started")
            .await
            .expect("resolve started status")
            .expect("started status seeded");
    let issue = trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
        .await
        .expect("read ticket")
        .expect("issue exists");
    // The automation read `started`; by the time it writes, the ticket is elsewhere.
    let stale_read = trakkt_types::models::IssueUpdate {
        status_id: Some(started.status_id),
        expected_status_id: Some("a-status-the-ticket-has-left".into()),
        ..Default::default()
    };
    let result = trakkt_auth::issue_service::update_issue(
        &f.state.db,
        WORKSPACE,
        &issue.team_key,
        issue.number,
        &stale_read,
        None,
        trakkt_types::enums::ActionSource::Github,
        None,
        None,
    )
    .await;
    assert!(
        matches!(result, Err(trakkt_core::Error::PreconditionFailed(_))),
        "{result:?}"
    );
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
}

#[tokio::test]
async fn ignore_removes_the_branch_link_an_earlier_push_made() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let mut payload = push(&f.personal);
    payload["ref"] = json!("refs/heads/feature/KEY-1-rework");
    payload["commits"] = json!([]);
    deliver(&f, "push", "ignore-push-first", payload).await;
    assert_eq!(
        links(&f, &f.issue).await.iter().filter(|l| l.link_type == "branch").count(),
        1,
        "the push linked the branch"
    );
    let pr = Pr::new(88, "Rework", "Ignore KEY-1", "feature/KEY-1-rework");
    send(&f, "ignore-pr-after", &pr, "opened", false).await;
    assert!(
        links(&f, &f.issue).await.is_empty(),
        "the PR said ignore, so no link to the ticket remains"
    );
    assert_eq!(status_category(&f, &f.issue).await, "backlog");
    stub.stop().await;
}

#[tokio::test]
async fn manual_completion_comments_on_the_closing_pr_only_while_linkback_is_on() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let pr = Pr::new(89, "Tidy", "Fixes KEY-1", "feature/manual");
    send(&f, "manual-open", &pr, "opened", false).await;
    let before = stub.comments().len();
    let client = f.state.github_client.clone().expect("stub client attached");
    let notify = || {
        trakkt_github::transitions::notify_github_links_on_completion(
            &f.state.db,
            &client,
            &f.state.encryption_key,
            &f.issue,
            "KEY",
            1,
            "Identical KEY-1 in separate workspaces",
            "Done",
            "http://localhost",
        )
    };
    notify().await.expect("notify with linkback on");
    assert_eq!(stub.comments().len(), before + 1, "linkback on comments");
    schema::set_linkback_enabled(&f.state.db, WORKSPACE, false, None)
        .await
        .expect("turn linkback off");
    notify().await.expect("notify with linkback off");
    assert_eq!(stub.comments().len(), before + 1, "linkback off posts nothing");
    stub.stop().await;
}

async fn comments_after(stub: &GithubStub, since: usize) -> Vec<String> {
    stub.comments().into_iter().skip(since).collect()
}

#[tokio::test]
async fn nothing_but_a_held_back_merge_ever_completes_a_reopened_ticket() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let fix = Pr::new(90, "Fix", "Fixes KEY-1", "feature/fix");
    send(&f, "reopen-open", &fix, "opened", false).await;
    send(&f, "reopen-merge", &fix, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    // A person finds the fix incomplete and reopens the ticket.
    reopen_by_hand(&f).await;
    let baseline = stub.comments().len();

    // Editing the merged PR, a mention PR opening/editing/closing/merging, and a
    // contributing PR merging are all things a PR can do. None completes it.
    let mut edited = fix.event(&f.personal, "edited", true);
    edited["pull_request"]["state"] = json!("closed");
    deliver(&f, "pull_request", "reopen-edit-merged", edited).await;
    let mention = Pr::new(91, "Notes", "see KEY-1 for background", "feature/notes");
    send(&f, "reopen-mention-open", &mention, "opened", false).await;
    let renamed = Pr::new(91, "Notes", "no longer mentions it", "feature/notes");
    send(&f, "reopen-mention-edit", &renamed, "edited", false).await;
    send(&f, "reopen-mention-close", &mention, "closed", false).await;
    let mention_two = Pr::new(92, "More notes", "Related to KEY-1", "feature/more");
    send(&f, "reopen-mention2-open", &mention_two, "opened", false).await;
    send(&f, "reopen-mention2-merge", &mention_two, "closed", true).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "backlog",
        "edits and mentions leave the reopened ticket exactly where the person put it"
    );
    assert!(
        comments_after(&stub, baseline).await.is_empty(),
        "mentions and edits posted nothing: {:?}",
        comments_after(&stub, baseline).await
    );
    // A contributing PR may start the ticket, and its merge must not complete it.
    let contributes = Pr::new(93, "Groundwork", "Refs KEY-1", "feature/ground");
    send(&f, "reopen-contrib-open", &contributes, "opened", false).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    let after_open = comments_after(&stub, baseline).await;
    assert_eq!(after_open.len(), 1, "{after_open:?}");
    assert!(after_open[0].contains("Status moved to"), "{after_open:?}");
    send(&f, "reopen-contrib-merge", &contributes, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "started");
    assert_eq!(
        comments_after(&stub, baseline).await.len(),
        1,
        "the contributing merge announced nothing"
    );
    stub.stop().await;
}

#[tokio::test]
async fn an_open_sibling_is_checked_against_github_so_a_missed_event_cannot_block_the_ticket() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    // The organization's PR opened, then merged on GitHub without us hearing.
    let elsewhere = Pr::new(94, "Other half", "Fixes KEY-1", "feature/elsewhere");
    deliver(
        &f,
        "pull_request",
        "missed-open",
        elsewhere.event(&f.organization, "opened", false),
    )
    .await;
    stub.set_pull("fixture-org/repo", 94, "closed", true);
    let here = Pr::new(95, "This half", "Fixes KEY-1", "feature/here");
    send(&f, "missed-open-here", &here, "opened", false).await;
    send(&f, "missed-merge-here", &here, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    assert_eq!(
        link_state(&f, &f.issue, "94").await.as_deref(),
        Some("merged"),
        "the stale link was corrected from GitHub"
    );
    stub.stop().await;
}

#[tokio::test]
async fn a_sibling_github_cannot_be_asked_about_keeps_holding_the_merge() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let elsewhere = Pr::new(96, "Other half", "Fixes KEY-1", "feature/elsewhere");
    deliver(
        &f,
        "pull_request",
        "suspended-open",
        elsewhere.event(&f.organization, "opened", false),
    )
    .await;
    // Suspended is temporary: the connection will be back.
    schema::suspend_installation(&f.state.db, &f.organization.installation_id)
        .await
        .expect("suspend the organization connection");
    let here = Pr::new(97, "This half", "Fixes KEY-1", "feature/here");
    send(&f, "suspended-open-here", &here, "opened", false).await;
    send(&f, "suspended-merge-here", &here, "closed", true).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "started",
        "an open PR behind a suspended connection still holds the merge"
    );
    stub.stop().await;
}

#[tokio::test]
async fn a_merged_pr_never_becomes_open_again_and_a_closed_one_only_by_reopening() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let merged = Pr::new(98, "Done", "Fixes KEY-1", "feature/done");
    send(&f, "fence-open-merged", &merged, "opened", false).await;
    send(&f, "fence-merge", &merged, "closed", true).await;
    for (delivery, action) in [("fence-edit", "edited"), ("fence-reopen", "reopened"), ("fence-late-open", "opened")] {
        send(&f, delivery, &merged, action, false).await;
        assert_eq!(
            link_state(&f, &f.issue, "98").await.as_deref(),
            Some("merged"),
            "{action} cannot undo a merge"
        );
    }
    let closed = Pr::new(99, "Closed", "Refs KEY-1", "feature/closed");
    send(&f, "fence-open-closed", &closed, "opened", false).await;
    send(&f, "fence-close", &closed, "closed", false).await;
    send(&f, "fence-edit-closed", &closed, "edited", false).await;
    assert_eq!(link_state(&f, &f.issue, "99").await.as_deref(), Some("closed"));
    send(&f, "fence-reopen-closed", &closed, "reopened", false).await;
    assert_eq!(link_state(&f, &f.issue, "99").await.as_deref(), Some("open"));
    stub.stop().await;
}

async fn move_by_hand(f: &Fixture, category: &str) {
    let issue = trakkt_auth::issue_service::get_issue_by_id(&f.state.db, &f.issue)
        .await
        .expect("read the ticket to move")
        .expect("issue exists");
    let status = trakkt_auth::status_service::get_status_by_category(&f.state.db, WORKSPACE, category)
        .await
        .expect("resolve status")
        .expect("status seeded");
    trakkt_auth::issue_service::update_issue(
        &f.state.db,
        WORKSPACE,
        &issue.team_key,
        issue.number,
        &trakkt_types::models::IssueUpdate {
            status_id: Some(status.status_id),
            ..Default::default()
        },
        None,
        trakkt_types::enums::ActionSource::Api,
        None,
        None,
    )
    .await
    .expect("move the ticket by hand");
}

async fn pending_marks(f: &Fixture) -> Vec<bool> {
    links(f, &f.issue)
        .await
        .into_iter()
        .filter(|l| l.link_type == "pull_request")
        .map(|l| l.completion_pending)
        .collect()
}

#[tokio::test]
async fn finishing_the_ticket_by_hand_clears_a_held_merge_so_it_cannot_come_back() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let a = Pr::new(100, "Part one", "Fixes KEY-1", "feature/a");
    let b = Pr::new(101, "Part two", "Fixes KEY-1", "feature/b");
    send(&f, "hand-open-a", &a, "opened", false).await;
    send(&f, "hand-open-b", &b, "opened", false).await;
    send(&f, "hand-merge-a", &a, "closed", true).await;
    assert!(pending_marks(&f).await.contains(&true), "A's merge is held back by B");
    // A person finishes the ticket, then reopens it for follow-up work.
    move_by_hand(&f, "completed").await;
    assert!(!pending_marks(&f).await.contains(&true), "finishing it settled the held merge");
    move_by_hand(&f, "started").await;
    send(&f, "hand-close-b", &b, "closed", false).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "started",
        "the held merge must not re-complete a ticket a person reopened"
    );
    stub.stop().await;
}

#[tokio::test]
async fn a_mark_cannot_be_set_on_a_ticket_that_is_already_finished() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let a = Pr::new(102, "Fix", "Fixes KEY-1", "feature/a");
    send(&f, "late-open-a", &a, "opened", false).await;
    send(&f, "late-merge-a", &a, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    let link = links(&f, &f.issue).await.into_iter().next().expect("the PR link");
    // A settle that read a sibling as open lost a race with the completion.
    schema::set_completion_pending(&f.state.db, &link.link_id, true)
        .await
        .expect("late mark attempt");
    assert!(!pending_marks(&f).await.contains(&true));
    stub.stop().await;
}

#[tokio::test]
async fn a_connection_coming_back_settles_a_merge_it_missed_while_away() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let elsewhere = Pr::new(103, "Other half", "Fixes KEY-1", "feature/elsewhere");
    deliver(&f, "pull_request", "away-open", elsewhere.event(&f.organization, "opened", false)).await;
    let here = Pr::new(104, "This half", "Fixes KEY-1", "feature/here");
    send(&f, "away-open-here", &here, "opened", false).await;
    schema::suspend_installation(&f.state.db, &f.organization.installation_id)
        .await
        .expect("suspend the organization connection");
    send(&f, "away-merge-here", &here, "closed", true).await;
    assert_eq!(status_category(&f, &f.issue).await, "started", "held by the suspended PR");
    // The organization's PR merged while the connection was away; that event was dropped.
    stub.set_pull("fixture-org/repo", 103, "closed", true);
    deliver(
        &f,
        "installation",
        "away-unsuspend",
        json!({"action":"unsuspend","installation":{"id":f.organization.github_installation_id}}),
    )
    .await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "completed",
        "nothing else would ever have released the held merge"
    );
    stub.stop().await;
}

#[tokio::test]
async fn a_merged_pr_in_a_repository_that_is_gone_still_counts_as_the_fix() {
    let mut f = fixture().await;
    let stub = GithubStub::attach(&mut f).await;
    let merged_elsewhere = Pr::new(105, "The fix", "Fixes KEY-1", "feature/elsewhere");
    deliver(&f, "pull_request", "gone-open", merged_elsewhere.event(&f.organization, "opened", false)).await;
    let here = Pr::new(106, "Also this", "Fixes KEY-1", "feature/here");
    send(&f, "gone-open-here", &here, "opened", false).await;
    deliver(&f, "pull_request", "gone-merge", merged_elsewhere.event(&f.organization, "closed", true)).await;
    assert_eq!(status_category(&f, &f.issue).await, "started", "held by the open PR here");
    schema::disconnect_installation(&f.state.db, &f.organization.installation_id, WORKSPACE)
        .await
        .expect("disconnect the organization connection");
    send(&f, "gone-close-here", &here, "closed", false).await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "completed",
        "the merged fix released the hold; it must not be cancelled because its repository left"
    );
    stub.stop().await;
}

/// Personal PR A merges while organization PR X is open, so the completion is
/// held. Returns the stub, ready for the connection to lose reach.
async fn hold_a_merge_behind_an_organization_pr(f: &mut Fixture, tag: &str) -> GithubStub {
    let stub = GithubStub::attach(f).await;
    let x = Pr::new(110, "Other half", "Fixes KEY-1", "feature/elsewhere");
    deliver(f, "pull_request", &format!("{tag}-open-x"), x.event(&f.organization, "opened", false)).await;
    let a = Pr::new(111, "This half", "Fixes KEY-1", "feature/here");
    send(f, &format!("{tag}-open-a"), &a, "opened", false).await;
    send(f, &format!("{tag}-merge-a"), &a, "closed", true).await;
    assert_eq!(status_category(f, &f.issue).await, "started", "{tag}: held by the organization PR");
    stub
}

#[tokio::test]
async fn removing_the_repository_of_the_holding_pr_releases_the_held_merge() {
    let mut f = fixture().await;
    let stub = hold_a_merge_behind_an_organization_pr(&mut f, "repo-removed").await;
    deliver(
        &f,
        "installation_repositories",
        "repo-removed-event",
        json!({"action":"removed","installation":{"id":f.organization.github_installation_id},
            "repository_selection":"selected","repositories_added":[],
            "repositories_removed":[{"full_name":"fixture-org/repo"}]}),
    )
    .await;
    assert_eq!(
        status_category(&f, &f.issue).await,
        "completed",
        "the repository that held the merge can no longer report"
    );
    stub.stop().await;
}

#[tokio::test]
async fn uninstalling_the_connection_of_the_holding_pr_releases_the_held_merge() {
    let mut f = fixture().await;
    let stub = hold_a_merge_behind_an_organization_pr(&mut f, "uninstalled").await;
    deliver(
        &f,
        "installation",
        "uninstalled-event",
        json!({"action":"deleted","installation":{"id":f.organization.github_installation_id}}),
    )
    .await;
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    stub.stop().await;
}

#[tokio::test]
async fn the_settings_actions_settle_held_merges_through_the_same_sweep() {
    // Disconnecting from settings and completing a reauthorization both call
    // `settle_pending_completions`; one issue failing never stops the others.
    let mut f = fixture().await;
    let stub = hold_a_merge_behind_an_organization_pr(&mut f, "settings").await;
    schema::disconnect_installation(&f.state.db, &f.organization.installation_id, WORKSPACE)
        .await
        .expect("disconnect the organization connection");
    let client = f.state.github_client.clone().expect("stub client attached");
    trakkt_github::transitions::settle_pending_completions(
        &f.state.db,
        &client,
        WORKSPACE,
        &f.state.encryption_key,
        None,
    )
    .await
    .expect("sweep after the connection change");
    assert_eq!(status_category(&f, &f.issue).await, "completed");
    assert!(
        pending_marks(&f).await.iter().all(|p| !p),
        "the held merge is settled"
    );
    stub.stop().await;
}
