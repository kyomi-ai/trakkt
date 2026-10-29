// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real HTTP MCP calls against migrated SQLite, with shared REST handler comparisons.
pub mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use trakkt_core::{
    db_execute, db_fetch_scalar,
    test_helpers::{seed_team, seed_user, seed_workspace},
};
use trakkt_server::{routes, state::AppState};

const WS: &str = "receipt-workspace";
const USER: &str = "receipt-user";

async fn fixture() -> AppState {
    let state = common::test_state().await;
    seed_user(&state.db, USER, "receipt@example.test")
        .await
        .expect("seed receipt caller");
    seed_workspace(&state.db, WS, USER)
        .await
        .expect("seed receipt workspace");
    seed_team(&state.db, "receipt-team", WS, "RCP")
        .await
        .expect("seed receipt team");
    seed_team(&state.db, "move-team", WS, "MOV")
        .await
        .expect("seed destination team");
    trakkt_auth::status_service::seed_default_statuses(&state.db, WS)
        .await
        .expect("seed default statuses");
    state
}

async fn rpc(state: &AppState, name: &str, arguments: Value) -> Value {
    let response = Router::new()
        .nest("/mcp", routes::mcp::routes())
        .with_state(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    format!("Bearer {}", common::access_token(state, USER, WS)),
                )
                .body(Body::from(
                    json!({"jsonrpc":"2.0", "id":1, "method":"tools/call",
                "params":{"name":name,"arguments":arguments}})
                    .to_string(),
                ))
                .expect("build MCP request"),
        )
        .await
        .expect("execute MCP request");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read MCP response");
    serde_json::from_slice(&bytes).expect("decode JSON-RPC response")
}

async fn call(state: &AppState, name: &str, arguments: Value) -> (Value, String) {
    let response = rpc(state, name, arguments).await;
    assert!(response.get("error").is_none(), "{name}: {response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("MCP text content")
        .to_string();
    let receipt: Value = serde_json::from_str(&text).expect("receipt is JSON");
    assert_eq!(text, receipt.to_string(), "MCP serialization is compact");
    (receipt, text)
}

async fn shared(state: &AppState, name: &str, arguments: Value) -> Value {
    let ctx = trakkt_api::ApiCtx::from_bearer(
        WS.into(),
        USER.into(),
        &state.db,
        &state.ws_manager,
        &*state.attachment_storage,
        None,
        None,
        "http://localhost",
        trakkt_types::enums::ActionSource::User,
        None,
    );
    let operations = trakkt_api::all_operations();
    let op = operations
        .iter()
        .find(|op| op.name == name)
        .expect("registered REST operation");
    (op.handler)(ctx, arguments)
        .await
        .expect("execute shared REST handler")
}

fn stable_size(mut receipt: Value) -> usize {
    // Compare actual receipts while removing only variable generated IDs and clock precision.
    for key in [
        "issue_id",
        "project_id",
        "milestone_id",
        "comment_id",
        "release_id",
        "created_at",
        "updated_at",
        "started_at",
        "completed_at",
    ] {
        if receipt.get(key).is_some() {
            receipt[key] = json!("generated");
        }
    }
    receipt.to_string().len()
}

#[tokio::test]
async fn nine_priority_tools_exclude_prose_and_have_size_independent_receipts() {
    let mut sizes = Vec::new();
    for repetitions in [1, 4000] {
        let state = fixture().await;
        let prose = "PROSE_SENTINEL markdown text\n".repeat(repetitions);
        db_execute!(
            &state.db,
            "UPDATE teams SET description = $1 WHERE team_id = 'receipt-team'",
            &prose
        )
        .expect("seed unrelated team prose");
        let mut outputs = Vec::new();
        let (issue, text) = call(
            &state,
            "create_issue",
            json!({"team_key":"RCP","title":"Issue","description":prose}),
        )
        .await;
        assert_eq!(issue["key"], "RCP-1");
        assert!(issue["issue_id"].is_string());
        assert!(issue["created_at"].is_string());
        outputs.push(("create_issue", issue, text));
        let status = trakkt_auth::status_service::get_status_by_category(&state.db, WS, "started")
            .await
            .expect("lookup started status")
            .expect("started default exists");
        let (updated, text) = call(
            &state,
            "update_issue",
            json!({"issue_identifier":"RCP-1","status_id":status.status_id}),
        )
        .await;
        assert_eq!(updated["status_category"], "started");
        outputs.push(("update_issue", updated, text));
        let (comment, text) = call(
            &state,
            "add_comment",
            json!({"issue_identifier":"RCP-1","body":prose}),
        )
        .await;
        assert!(comment["comment_id"].is_string());
        outputs.push(("add_comment", comment, text));
        let (project, text) = call(
            &state,
            "create_project",
            json!({"name":"Project","description":prose}),
        )
        .await;
        let project_id = project["project_id"].clone();
        assert!(project_id.is_string());
        outputs.push(("create_project", project, text));
        let (updated, text) = call(
            &state,
            "update_project",
            json!({"project_id":project_id,"description":prose}),
        )
        .await;
        outputs.push(("update_project", updated, text));
        let (milestone, text) = call(
            &state,
            "create_milestone",
            json!({"project_id":project_id,"name":"Milestone","description":prose}),
        )
        .await;
        let milestone_id = milestone["milestone_id"].clone();
        assert!(milestone_id.is_string());
        outputs.push(("create_milestone", milestone, text));
        let (updated, text) = call(
            &state,
            "update_milestone",
            json!({"milestone_id":milestone_id,"description":prose}),
        )
        .await;
        outputs.push(("update_milestone", updated, text));
        let (release, text) = call(
            &state,
            "create_release",
            json!({"team_key":"RCP","tag_name":"v1","notes":prose,"commit_shas":[]}),
        )
        .await;
        assert_eq!(release["issue_count"], 0);
        assert!(release["release_id"].is_string());
        outputs.push(("create_release", release, text));
        let (team, text) = call(
            &state,
            "update_team_settings",
            json!({"team_key":"RCP","settings":{"auto_archive_days":30}}),
        )
        .await;
        assert_eq!(team["settings"]["auto_archive_days"], 30);
        outputs.push(("update_team_settings", team, text));
        for (name, receipt, text) in &outputs {
            assert_eq!(receipt["success"], true, "{name}");
            assert!(!text.contains("PROSE_SENTINEL"), "{name} echoed prose");
            for field in ["description", "body", "notes", "labels", "workspace_id"] {
                assert!(receipt.get(field).is_none(), "{name} contains {field}");
            }
        }
        // Shared handlers still return full REST entities, including persisted submitted prose.
        let rest_issue = shared(&state, "get_issue", json!({"issue_identifier":"RCP-1"})).await;
        assert_eq!(rest_issue["issue"]["description"], prose);
        let rest_project = shared(&state, "get_project", json!({"project_id":project_id})).await;
        assert_eq!(rest_project["project"]["description"], prose);
        let rest_release = shared(
            &state,
            "get_release",
            json!({"release_id":outputs[7].1["release_id"]}),
        )
        .await;
        assert_eq!(rest_release["notes"], prose);
        let rest_comment = shared(
            &state,
            "add_comment",
            json!({"issue_identifier":"RCP-1","body":prose}),
        )
        .await;
        assert_eq!(rest_comment["body"], prose);
        if repetitions == 4000 {
            for (name, _, text) in &outputs {
                println!("{name}: compact receipt {} bytes", text.len());
            }
            println!(
                "update_issue representative full pretty entity {} bytes; compact receipt {} bytes",
                serde_json::to_string_pretty(&rest_issue["issue"])
                    .expect("serialize full issue")
                    .len(),
                outputs[1].2.len()
            );
        }
        sizes.push(
            outputs
                .into_iter()
                .map(|(name, value, _)| (name, stable_size(value)))
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        sizes[0], sizes[1],
        "growing prose must not grow any priority receipt"
    );
}

#[tokio::test]
async fn moved_issue_key_and_requested_effective_values_are_retained() {
    let state = fixture().await;
    let (created, _) = call(
        &state,
        "create_issue",
        json!({"team_key":"RCP","title":"Move"}),
    )
    .await;
    let (moved, _) = call(
        &state,
        "update_issue",
        json!({"issue_identifier":"RCP-1","move_to_team_id":"move-team","priority":2}),
    )
    .await;
    assert_eq!(moved["issue_id"], created["issue_id"]);
    assert_eq!(moved["key"], "MOV-1");
    assert_eq!(moved["priority"], 2);
    let (updated, _) = call(
        &state,
        "update_issue",
        json!({"issue_identifier":"MOV-1","assignee":null}),
    )
    .await;
    assert_eq!(updated.get("assignee_id"), Some(&Value::Null));
}

#[tokio::test]
async fn primary_errors_remain_errors_and_partial_relation_warnings_keep_identity() {
    let state = fixture().await;
    let missing = rpc(
        &state,
        "update_issue",
        json!({"issue_identifier":"RCP-999","priority":2}),
    )
    .await;
    assert!(missing["error"].is_object());
    assert!(missing.get("result").is_none());
    let invalid = rpc(&state, "create_project", json!({})).await;
    assert!(invalid["error"].is_object());
    let (created, _) = call(&state, "create_issue", json!({"team_key":"RCP","title":"Partial",
        "parent_issue_id":"missing-parent", "relations":[{"relation_type":"blocks","issue":"RCP-999"}]})).await;
    assert_eq!(created["success"], true);
    assert!(created["issue_id"].is_string());
    assert_eq!(created["key"], "RCP-1");
    assert_eq!(
        created["relation_warnings"]
            .as_array()
            .expect("partial relation warnings")
            .len(),
        2
    );
    assert!(
        created["relation_warnings"][0]
            .as_str()
            .expect("parent warning")
            .contains("not create_issue")
    );
    assert_eq!(
        shared(&state, "get_issue", json!({"issue_identifier":"RCP-1"})).await["issue"]["title"],
        "Partial"
    );
}

#[tokio::test]
async fn duplicate_auto_close_reports_actual_outcomes_without_changing_rest_shape() {
    let state = fixture().await;
    for title in ["Source", "Target", "Rest source", "Failed source"] {
        call(
            &state,
            "create_issue",
            json!({"team_key":"RCP","title":title}),
        )
        .await;
    }
    let args = json!({"source_issue":"RCP-1","target_issue":"RCP-2","relation_type":"duplicate"});
    let (relation, _) = call(&state, "add_relation", args).await;
    assert!(relation["relation_id"].is_string());
    assert_eq!(relation["auto_close"]["status"], "closed");
    assert_eq!(
        shared(&state, "get_issue", json!({"issue_identifier":"RCP-1"})).await["issue"]["status_category"],
        "cancelled"
    );
    // A source may have only one duplicate relation. Remove the first relation;
    // its already-cancelled status remains and is the next request's precondition.
    call(
        &state,
        "remove_relation",
        json!({"relation_id":relation["relation_id"]}),
    )
    .await;
    let (again, _) = call(
        &state,
        "add_relation",
        json!({"source_issue":"RCP-1","target_issue":"RCP-3","relation_type":"duplicate"}),
    )
    .await;
    assert_eq!(again["auto_close"]["status"], "already_cancelled");
    let rest = shared(
        &state,
        "add_relation",
        json!({"source_issue":"RCP-3","target_issue":"RCP-2","relation_type":"duplicate"}),
    )
    .await;
    assert!(rest.get("auto_close").is_none());
    // A real SQLite trigger fails only the secondary status update, after relation insertion commits.
    db_execute!(&state.db, "CREATE TRIGGER reject_auto_close BEFORE UPDATE OF status_id ON issues BEGIN SELECT RAISE(FAIL, 'injected auto-close failure'); END")
        .expect("install secondary update rejection");
    let (partial, _) = call(
        &state,
        "add_relation",
        json!({"source_issue":"RCP-4","target_issue":"RCP-2","relation_type":"duplicate"}),
    )
    .await;
    assert_eq!(partial["auto_close"]["status"], "failed");
    assert!(partial["relation_id"].is_string());
    assert!(
        partial["auto_close"]["warning"]
            .as_str()
            .expect("actionable secondary warning")
            .contains("do not retry")
    );
    let count: i64 = db_fetch_scalar!(
        &state.db,
        i64,
        "SELECT COUNT(*) FROM issue_relations WHERE relation_id = $1",
        partial["relation_id"]
            .as_str()
            .expect("persisted relation ID")
    )
    .expect("read relation persistence");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn upload_link_outcomes_preserve_saved_attachment_and_rest_contract() {
    let state = fixture().await;
    let (issue, _) = call(
        &state,
        "create_issue",
        json!({"team_key":"RCP","title":"Upload target"}),
    )
    .await;
    let args = json!({"filename":"receipt.txt","content_type":"text/plain","content_base64":"aGVsbG8=","issue_id":issue["issue_id"]});
    let (linked, _) = call(&state, "upload_attachment", args.clone()).await;
    assert_eq!(linked["issue_link"]["status"], "linked");
    let rest = shared(&state, "upload_attachment", args).await;
    let mut keys: Vec<_> = rest
        .as_object()
        .expect("REST attachment metadata")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "attachment_id",
            "content_type",
            "filename",
            "size_bytes",
            "url"
        ]
    );
    let (partial, _) = call(&state, "upload_attachment", json!({"filename":"receipt.txt","content_type":"text/plain","content_base64":"aGVsbG8=","issue_id":"missing"})).await;
    assert_eq!(partial["issue_link"]["status"], "failed");
    assert!(partial["attachment_id"].is_string());
    let attachment = shared(
        &state,
        "download_attachment",
        json!({"attachment_id":partial["attachment_id"]}),
    )
    .await;
    assert_eq!(attachment["content_base64"], "aGVsbG8=");
    let invalid = rpc(
        &state,
        "upload_attachment",
        json!({"filename":"receipt.txt","content_type":"text/plain","content_base64":"!invalid"}),
    )
    .await;
    assert!(invalid["error"].is_object());
}

#[tokio::test]
async fn release_receipt_retains_nonzero_deduplicated_issue_count() {
    let state = fixture().await;
    let (issue, _) = call(
        &state,
        "create_issue",
        json!({"team_key":"RCP","title":"Released issue"}),
    )
    .await;
    db_execute!(&state.db,
        "INSERT INTO github_apps (github_app_id, app_id, app_name, client_id, client_secret_encrypted, private_key_encrypted, webhook_secret_encrypted) VALUES ('receipt-app', 4242, 'Receipt app', 'client', 'enc', 'enc', 'enc')")
        .expect("seed release GitHub app");
    db_execute!(&state.db,
        "INSERT INTO github_installations (installation_id, workspace_id, github_app_id, github_installation_id, account_login, account_type) VALUES ('receipt-install', $1, 'receipt-app', 99, 'receipt', 'User')", WS)
        .expect("seed release GitHub installation");
    db_execute!(&state.db,
        "INSERT INTO github_links (link_id, workspace_id, issue_id, installation_id, link_type, repo_full_name, ref_identifier, url) VALUES ('receipt-link', $1, $2, 'receipt-install', 'commit', 'receipt/repo', 'abcdef123456', 'https://github.test/commit/abcdef123456')", WS, issue["issue_id"].as_str().expect("release issue identity"))
        .expect("seed commit link for release resolution");
    let (release, _) = call(&state, "create_release", json!({"team_key":"RCP", "tag_name":"v-count", "commit_shas":["abcdef", "abcdef123456"],"notes":"Private release notes"})).await;
    assert_eq!(release["issue_count"], 1);
    let stored = shared(
        &state,
        "get_release",
        json!({"release_id":release["release_id"]}),
    )
    .await;
    assert_eq!(
        stored["issues"]
            .as_array()
            .expect("release issue links")
            .len(),
        1
    );
    assert_eq!(stored["notes"], "Private release notes");
    assert!(release.get("notes").is_none());
}
