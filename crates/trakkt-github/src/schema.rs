// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database queries for GitHub integration tables.
//!
//! All functions are free functions taking `db: &DbPool` as the first argument,
//! following the project's service-layer conventions.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;

// ─── Row types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GitHubApp {
    pub github_app_id: String,
    pub app_id: i64,
    pub app_name: String,
    pub client_id: String,
    pub client_secret_encrypted: String,
    pub private_key_encrypted: String,
    pub webhook_secret_encrypted: String,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GitHubInstallation {
    pub installation_id: String,
    pub workspace_id: String,
    pub github_app_id: String,
    pub github_installation_id: i64,
    pub account_login: String,
    pub account_type: String,
    pub target_repos: Option<String>,
    pub access_token_encrypted: Option<String>,
    pub token_expires_at: Option<String>,
    pub created_at: String,
    pub suspended_at: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GitHubLink {
    pub link_id: String,
    pub workspace_id: String,
    pub issue_id: String,
    pub installation_id: String,
    pub link_type: String,
    pub github_id: Option<i64>,
    pub github_node_id: Option<String>,
    pub repo_full_name: String,
    pub ref_identifier: String,
    pub title: Option<String>,
    pub state: Option<String>,
    pub url: String,
    pub author_login: Option<String>,
    pub close_intent: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GitHubEvent {
    pub event_id: String,
    pub github_delivery_id: String,
    pub installation_id: Option<String>,
    pub event_type: String,
    pub action: Option<String>,
    pub payload_summary: Option<String>,
    pub processed_at: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GitHubTransitionRule {
    pub rule_id: String,
    pub workspace_id: String,
    pub trigger_event: String,
    pub close_intent_required: bool,
    pub target_status_category: String,
    pub enabled: bool,
    pub created_at: String,
}

// ─── github_apps ─────────────────────────────────────────────────────────────

/// Get the (singleton) GitHub App configuration.
pub async fn get_github_app(db: &DbPool) -> trakkt_core::Result<Option<GitHubApp>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubApp,
        "SELECT github_app_id, app_id, app_name, client_id, \
                client_secret_encrypted, private_key_encrypted, \
                webhook_secret_encrypted, \
                CAST(created_at AS TEXT) AS created_at \
         FROM github_apps \
         LIMIT 1"
    )?;
    Ok(row)
}

/// Create a new GitHub App configuration row.
pub async fn create_github_app(
    db: &DbPool,
    app_id: i64,
    app_name: &str,
    client_id: &str,
    client_secret_encrypted: &str,
    private_key_encrypted: &str,
    webhook_secret_encrypted: &str,
) -> trakkt_core::Result<GitHubApp> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let github_app_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO github_apps \
         (github_app_id, app_id, app_name, client_id, client_secret_encrypted, \
          private_key_encrypted, webhook_secret_encrypted, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, {now})"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &github_app_id, app_id, app_name, client_id,
        client_secret_encrypted, private_key_encrypted, webhook_secret_encrypted
    )?;

    let row = trakkt_core::db_fetch_one!(
        db,
        GitHubApp,
        "SELECT github_app_id, app_id, app_name, client_id, \
                client_secret_encrypted, private_key_encrypted, \
                webhook_secret_encrypted, \
                CAST(created_at AS TEXT) AS created_at \
         FROM github_apps WHERE github_app_id = $1",
        &github_app_id
    )?;
    Ok(row)
}

// ─── github_installations ────────────────────────────────────────────────────

/// Look up an installation by its GitHub-assigned installation ID.
pub async fn get_installation_by_github_id(
    db: &DbPool,
    github_installation_id: i64,
) -> trakkt_core::Result<Option<GitHubInstallation>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubInstallation,
        "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at \
         FROM github_installations \
         WHERE github_installation_id = $1",
        github_installation_id
    )?;
    Ok(row)
}

/// Get the GitHub installation for a workspace.
pub async fn get_installation_for_workspace(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<GitHubInstallation>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubInstallation,
        "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at \
         FROM github_installations \
         WHERE workspace_id = $1",
        workspace_id
    )?;
    Ok(row)
}

/// Create a new GitHub installation record.
pub async fn create_installation(
    db: &DbPool,
    workspace_id: &str,
    github_app_id: &str,
    github_installation_id: i64,
    account_login: &str,
    account_type: &str,
    target_repos: Option<&serde_json::Value>,
) -> trakkt_core::Result<GitHubInstallation> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let installation_id = uuid::Uuid::new_v4().to_string();
    let target_repos_json = target_repos.map(|v| v.to_string());
    let repos_cast = sql_compat::cast_to_json(is_pg, "$7");

    let sql = format!(
        "INSERT INTO github_installations \
         (installation_id, workspace_id, github_app_id, github_installation_id, \
          account_login, account_type, target_repos, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {repos_cast}, {now})"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &installation_id, workspace_id, github_app_id, github_installation_id,
        account_login, account_type, &target_repos_json
    )?;

    let row = trakkt_core::db_fetch_one!(
        db,
        GitHubInstallation,
        "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at \
         FROM github_installations WHERE installation_id = $1",
        &installation_id
    )?;
    Ok(row)
}

/// Update the cached access token and its expiry for an installation.
pub async fn update_installation_token(
    db: &DbPool,
    installation_id: &str,
    access_token_encrypted: &str,
    token_expires_at: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "UPDATE github_installations \
         SET access_token_encrypted = $1, token_expires_at = $2 \
         WHERE installation_id = $3",
        access_token_encrypted, token_expires_at, installation_id
    )?;
    Ok(())
}

/// Mark an installation as suspended.
pub async fn suspend_installation(
    db: &DbPool,
    installation_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE github_installations SET suspended_at = {now} WHERE installation_id = $1"
    );
    trakkt_core::db_execute!(db, &sql, installation_id)?;
    Ok(())
}

/// Clear the suspended status and update the account details of an installation.
///
/// Used during the reconnection flow (disconnect then reconnect) to reactivate
/// an existing installation record with potentially updated account information.
pub async fn reactivate_installation(
    db: &DbPool,
    installation_id: &str,
    github_installation_id: i64,
    account_login: &str,
    account_type: &str,
    target_repos: Option<&serde_json::Value>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let target_repos_json = target_repos.map(|v| v.to_string());
    let repos_cast = sql_compat::cast_to_json(is_pg, "$4");
    let sql = format!(
        "UPDATE github_installations \
         SET suspended_at = NULL, github_installation_id = $1, \
             account_login = $2, account_type = $3, target_repos = {repos_cast} \
         WHERE installation_id = $5"
    );
    trakkt_core::db_execute!(
        db, &sql,
        github_installation_id, account_login, account_type, &target_repos_json, installation_id
    )?;
    Ok(())
}

/// Update the target_repos JSONB for an installation.
pub async fn update_installation_repos(
    db: &DbPool,
    installation_id: &str,
    target_repos: Option<&serde_json::Value>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let target_repos_json = target_repos.map(|v| v.to_string());
    let repos_cast = sql_compat::cast_to_json(is_pg, "$1");
    let sql = format!(
        "UPDATE github_installations SET target_repos = {repos_cast} WHERE installation_id = $2"
    );
    trakkt_core::db_execute!(db, &sql, &target_repos_json, installation_id)?;
    Ok(())
}

/// Clear the suspended status of an installation.
pub async fn unsuspend_installation(
    db: &DbPool,
    installation_id: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "UPDATE github_installations SET suspended_at = NULL WHERE installation_id = $1",
        installation_id
    )?;
    Ok(())
}

// ─── github_links ────────────────────────────────────────────────────────────

/// List all GitHub links for an issue.
pub async fn list_links_for_issue(
    db: &DbPool,
    issue_id: &str,
) -> trakkt_core::Result<Vec<GitHubLink>> {
    let rows: Vec<GitHubLink> = trakkt_core::db_fetch_all!(
        db,
        GitHubLink,
        "SELECT link_id, workspace_id, issue_id, installation_id, link_type, \
                github_id, github_node_id, repo_full_name, ref_identifier, \
                title, state, url, author_login, close_intent, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM github_links \
         WHERE issue_id = $1 \
         ORDER BY created_at DESC",
        issue_id
    )?;
    Ok(rows)
}

/// Parameters for creating a GitHub link.
pub struct CreateLinkParams<'a> {
    pub workspace_id: &'a str,
    pub issue_id: &'a str,
    pub installation_id: &'a str,
    pub link_type: &'a str,
    pub github_id: Option<i64>,
    pub github_node_id: Option<&'a str>,
    pub repo_full_name: &'a str,
    pub ref_identifier: &'a str,
    pub title: Option<&'a str>,
    pub state: Option<&'a str>,
    pub url: &'a str,
    pub author_login: Option<&'a str>,
    pub close_intent: bool,
}

/// Upsert a GitHub link — create or update on conflict.
///
/// On conflict (same workspace, issue, link_type, repo, ref_identifier),
/// update the mutable fields: title, state, url, author_login, close_intent,
/// github_id, github_node_id. Returns the resulting row.
pub async fn upsert_link(
    db: &DbPool,
    params: &CreateLinkParams<'_>,
) -> trakkt_core::Result<GitHubLink> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let link_id = uuid::Uuid::new_v4().to_string();

    let sql = format!(
        "INSERT INTO github_links \
         (link_id, workspace_id, issue_id, installation_id, link_type, \
          github_id, github_node_id, repo_full_name, ref_identifier, \
          title, state, url, author_login, close_intent, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, {now}, {now}) \
         ON CONFLICT (workspace_id, issue_id, link_type, repo_full_name, ref_identifier) \
         DO UPDATE SET \
            title = EXCLUDED.title, \
            state = EXCLUDED.state, \
            url = EXCLUDED.url, \
            author_login = EXCLUDED.author_login, \
            close_intent = EXCLUDED.close_intent, \
            github_id = EXCLUDED.github_id, \
            github_node_id = EXCLUDED.github_node_id, \
            updated_at = {now}"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &link_id, params.workspace_id, params.issue_id, params.installation_id, params.link_type,
        params.github_id, params.github_node_id, params.repo_full_name, params.ref_identifier,
        params.title, params.state, params.url, params.author_login, params.close_intent
    )?;

    // Fetch the row — either newly created or updated (conflict case).
    let row = trakkt_core::db_fetch_one!(
        db,
        GitHubLink,
        "SELECT link_id, workspace_id, issue_id, installation_id, link_type, \
                github_id, github_node_id, repo_full_name, ref_identifier, \
                title, state, url, author_login, close_intent, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM github_links \
         WHERE workspace_id = $1 AND issue_id = $2 AND link_type = $3 \
               AND repo_full_name = $4 AND ref_identifier = $5",
        params.workspace_id, params.issue_id, params.link_type,
        params.repo_full_name, params.ref_identifier
    )?;
    Ok(row)
}

/// Delete all links for a specific GitHub object (by type+repo+ref_identifier) in a
/// workspace where the issue_id is NOT in the provided keep list.
///
/// Used when a PR is edited and refs are removed — the keep list contains the
/// issue IDs still referenced, and any other links for this PR are deleted.
/// Returns the number of rows deleted.
pub async fn delete_links_not_matching_issues(
    db: &DbPool,
    workspace_id: &str,
    link_type: &str,
    repo_full_name: &str,
    ref_identifier: &str,
    keep_issue_ids: &[String],
) -> trakkt_core::Result<u64> {
    if keep_issue_ids.is_empty() {
        // Delete all links for this object in this workspace
        let result = trakkt_core::db_execute!(
            db,
            "DELETE FROM github_links \
             WHERE workspace_id = $1 AND link_type = $2 \
                   AND repo_full_name = $3 AND ref_identifier = $4",
            workspace_id, link_type, repo_full_name, ref_identifier
        )?;
        return Ok(result.rows_affected());
    }

    // Build IN clause for the keep list.
    let (in_clause, _) = trakkt_core::db::in_clause_placeholders(keep_issue_ids.len(), 5);
    let sql = format!(
        "DELETE FROM github_links \
         WHERE workspace_id = $1 AND link_type = $2 \
               AND repo_full_name = $3 AND ref_identifier = $4 \
               AND issue_id NOT IN {in_clause}"
    );

    // Dynamically bind the keep_issue_ids. Use db_with_pool! since the
    // number of binds is variable and the macros expect a fixed list.
    let rows_affected: u64 = trakkt_core::db_with_pool!(db, |pool| {
        let mut query = sqlx::query(&sql)
            .bind(workspace_id)
            .bind(link_type)
            .bind(repo_full_name)
            .bind(ref_identifier);
        for id in keep_issue_ids {
            query = query.bind(id);
        }
        let result = query.execute(pool).await?;
        Ok::<u64, sqlx::Error>(result.rows_affected())
    })?;

    Ok(rows_affected)
}

/// Update the state (and optionally title) of a GitHub link.
pub async fn update_link_state(
    db: &DbPool,
    link_id: &str,
    state: &str,
    title: Option<&str>,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE github_links SET state = $1, title = $2, updated_at = {now} WHERE link_id = $3"
    );
    trakkt_core::db_execute!(db, &sql, state, title, link_id)?;
    Ok(())
}

// ─── github_events ───────────────────────────────────────────────────────────

/// Record a new webhook event. Returns the generated event ID.
pub async fn create_event(
    db: &DbPool,
    github_delivery_id: &str,
    installation_id: Option<&str>,
    event_type: &str,
    action: Option<&str>,
    payload_summary: Option<&serde_json::Value>,
) -> trakkt_core::Result<String> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let event_id = uuid::Uuid::new_v4().to_string();
    let payload_json = payload_summary.map(|v| v.to_string());
    let payload_cast = sql_compat::cast_to_json(is_pg, "$6");

    let sql = format!(
        "INSERT INTO github_events \
         (event_id, github_delivery_id, installation_id, event_type, action, \
          payload_summary, created_at) \
         VALUES ($1, $2, $3, $4, $5, {payload_cast}, {now})"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &event_id, github_delivery_id, installation_id, event_type, action, &payload_json
    )?;
    Ok(event_id)
}

/// Mark an event as successfully processed.
pub async fn mark_event_processed(
    db: &DbPool,
    event_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE github_events SET processed_at = {now} WHERE event_id = $1"
    );
    trakkt_core::db_execute!(db, &sql, event_id)?;
    Ok(())
}

/// Mark an event as failed with an error message.
pub async fn mark_event_failed(
    db: &DbPool,
    event_id: &str,
    error: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "UPDATE github_events SET error = $1 WHERE event_id = $2",
        error, event_id
    )?;
    Ok(())
}

/// Check whether an event with the given delivery ID has already been recorded.
///
/// Used for idempotency — GitHub may redeliver webhooks.
pub async fn event_exists(
    db: &DbPool,
    github_delivery_id: &str,
) -> trakkt_core::Result<bool> {
    let count: i64 = trakkt_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM github_events WHERE github_delivery_id = $1",
        github_delivery_id
    )?;
    Ok(count > 0)
}

// ─── github_transition_rules ─────────────────────────────────────────────────

/// List all transition rules for a workspace.
pub async fn list_transition_rules(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<GitHubTransitionRule>> {
    let rows: Vec<GitHubTransitionRule> = trakkt_core::db_fetch_all!(
        db,
        GitHubTransitionRule,
        "SELECT rule_id, workspace_id, trigger_event, close_intent_required, \
                target_status_category, enabled, \
                CAST(created_at AS TEXT) AS created_at \
         FROM github_transition_rules \
         WHERE workspace_id = $1 \
         ORDER BY trigger_event ASC",
        workspace_id
    )?;
    Ok(rows)
}

/// Seed the default transition rules for a workspace.
///
/// Uses INSERT ... ON CONFLICT DO NOTHING so this is idempotent.
pub async fn seed_default_transition_rules(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let bool_true = sql_compat::bool_true(is_pg);
    let bool_false = sql_compat::bool_false(is_pg);

    let rule_id_1 = uuid::Uuid::new_v4().to_string();
    let rule_id_2 = uuid::Uuid::new_v4().to_string();
    let rule_id_3 = uuid::Uuid::new_v4().to_string();

    // pr_opened -> started (no close intent required)
    let sql = format!(
        "INSERT INTO github_transition_rules \
         (rule_id, workspace_id, trigger_event, close_intent_required, \
          target_status_category, enabled, created_at) \
         VALUES ($1, $2, $3, {bool_false}, $4, {bool_true}, {now}) \
         ON CONFLICT DO NOTHING"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &rule_id_1, workspace_id, "pr_opened", "started"
    )?;

    // pr_merged -> completed (close intent required)
    let sql = format!(
        "INSERT INTO github_transition_rules \
         (rule_id, workspace_id, trigger_event, close_intent_required, \
          target_status_category, enabled, created_at) \
         VALUES ($1, $2, $3, {bool_true}, $4, {bool_true}, {now}) \
         ON CONFLICT DO NOTHING"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &rule_id_2, workspace_id, "pr_merged", "completed"
    )?;

    // pr_closed -> cancelled (close intent required)
    let sql = format!(
        "INSERT INTO github_transition_rules \
         (rule_id, workspace_id, trigger_event, close_intent_required, \
          target_status_category, enabled, created_at) \
         VALUES ($1, $2, $3, {bool_true}, $4, {bool_true}, {now}) \
         ON CONFLICT DO NOTHING"
    );
    trakkt_core::db_execute!(
        db, &sql,
        &rule_id_3, workspace_id, "pr_closed", "cancelled"
    )?;

    Ok(())
}
