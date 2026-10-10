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
#[tokio::test]
async fn shared_merge_rules_use_each_account_token_and_disconnected_events_send_nothing() {
    use axum::{
        Json,
        extract::State,
        http::{HeaderMap, Uri},
    };
    use std::sync::{Arc, Mutex};
    type Calls = Arc<Mutex<Vec<(String, String)>>>;
    async fn github_fixture(
        State(calls): State<Calls>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Json<Value> {
        let path = uri.path().to_owned();
        let authorization = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .expect("all GitHub requests carry credentials")
            .to_owned();
        calls
            .lock()
            .expect("lock fixture request ledger")
            .push((path.clone(), authorization));
        if path.ends_with("/access_tokens") {
            let token = match path.as_str() {
                "/app/installations/101/access_tokens" => "fixture-personal-token",
                "/app/installations/202/access_tokens" => "fixture-org-token",
                _ => panic!("unexpected installation token request: {path}"),
            };
            Json(
                json!({"token":token,"expires_at":(chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()}),
            )
        } else {
            assert!(
                path.ends_with("/comments"),
                "unexpected outbound mutation: {path}"
            );
            Json(json!({"id":1}))
        }
    }
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind isolated GitHub fixture");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("read fixture address")
    );
    let api = Router::new()
        .fallback(github_fixture)
        .with_state(calls.clone());
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, api)
            .with_graceful_shutdown(async {
                stopped.await.expect("receive fixture shutdown");
            })
            .await
            .expect("serve isolated GitHub fixture");
    });
    let mut f = fixture().await;
    f.state.github_client = Some(Arc::new(
        trakkt_github::GitHubClient::for_test_endpoint(42, &TEST_KEY, &endpoint)
            .expect("build loopback-only GitHub client"),
    ));
    f.router = Router::new()
        .nest("/webhooks", trakkt_server::routes::github::routes())
        .with_state(f.state.clone());
    schema::seed_default_transition_rules(&f.state.db, WORKSPACE)
        .await
        .expect("seed shared workspace rules");
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
    let before_disconnect = calls.lock().expect("snapshot outgoing calls").clone();
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
        *calls.lock().expect("check disconnected outgoing ledger"),
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
        calls
            .lock()
            .expect("read other account outgoing call")
            .last()
            .expect("other account posts comment"),
        &(
            "/repos/fixture-org/repo/issues/4/comments".into(),
            "Bearer fixture-org-token".into()
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
    shutdown.send(()).expect("stop GitHub fixture listener");
    trakkt_core::test_helpers::task::join_soon(server, "GitHub fixture graceful shutdown").await;
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind isolated scope API");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("read scope API address")
    );
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let router = Router::new().fallback(api).with_state(scenario.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                stopped.await.expect("receive scope API shutdown");
            })
            .await
            .expect("serve scope API fixture");
    });
    let mut f = fixture().await;
    f.state.github_client = Some(Arc::new(
        trakkt_github::GitHubClient::for_test_endpoint(42, &TEST_KEY, &endpoint)
            .expect("construct local scope API client"),
    ));
    f.router = Router::new()
        .nest("/webhooks", trakkt_server::routes::github::routes())
        .with_state(f.state.clone());
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
