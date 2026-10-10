// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database queries for GitHub integration tables.
//!
//! All functions are free functions taking `db: &DbPool` as the first argument,
//! following the project's service-layer conventions.

use trakkt_core::DbPool;
use trakkt_core::sql_compat;

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

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, serde::Serialize)]
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
    pub disconnected_at: Option<String>,
    pub uninstalled_at: Option<String>,
    pub authorization_verified_at: Option<String>,
    pub github_account_id: Option<i64>,
    pub repository_selection: String,
    pub token_generation: i64,
    pub repository_scope_pending: bool,
}

impl GitHubInstallation {
    /// Legacy connections stay quarantined until user authorization revalidates identity.
    pub fn is_active(&self) -> bool {
        !self.repository_scope_pending
            && self.disconnected_at.is_none()
            && self.suspended_at.is_none()
            && self.uninstalled_at.is_none()
            && self.authorization_verified_at.is_some()
    }

    /// NULL means all only when the verified selection says all; [] means none.
    pub fn allows_repository(&self, repository: &str) -> bool {
        if !self.is_active() {
            return false;
        }
        if self.repository_selection == "all" {
            return true;
        }
        match self
            .target_repos
            .as_deref()
            .map(serde_json::from_str::<Vec<String>>)
        {
            Some(Ok(repos)) => repos
                .iter()
                .any(|repo| repo.eq_ignore_ascii_case(repository)),
            Some(Err(error)) => {
                tracing::warn!(installation_id = %self.installation_id, %error, "Invalid repository scope; denying event");
                false
            }
            None => false,
        }
    }
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

/// Result row for reverse-lookup queries (commit SHA -> issues, branch -> issues).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RefLookupResult {
    pub link_id: String,
    pub ref_identifier: String,
    pub repo_full_name: String,
    pub link_title: Option<String>,
    pub url: String,
    pub author_login: Option<String>,
    pub state: Option<String>,
    pub link_created_at: String,
    pub issue_id: String,
    pub team_key: String,
    pub number: i32,
    pub issue_title: String,
    pub description: Option<String>,
    pub status_name: String,
    pub status_category: String,
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
        db,
        &sql,
        &github_app_id,
        app_id,
        app_name,
        client_id,
        client_secret_encrypted,
        private_key_encrypted,
        webhook_secret_encrypted
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

/// Rotate app credentials while preserving installation references and OAuth settings.
pub async fn update_github_app_credentials(
    db: &DbPool,
    github_app_id: &str,
    app_name: &str,
    private_key_encrypted: &str,
    webhook_secret_encrypted: &str,
) -> trakkt_core::Result<()> {
    trakkt_core::db_execute!(
        db,
        "UPDATE github_apps SET app_name = $2, private_key_encrypted = $3, \
         webhook_secret_encrypted = $4 WHERE github_app_id = $1",
        github_app_id,
        app_name,
        private_key_encrypted,
        webhook_secret_encrypted
    )?;
    Ok(())
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
                account_login, account_type, CAST(target_repos AS TEXT) AS target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at, \
                CAST(disconnected_at AS TEXT) AS disconnected_at, \
                CAST(uninstalled_at AS TEXT) AS uninstalled_at, \
                CAST(authorization_verified_at AS TEXT) AS authorization_verified_at, \
                github_account_id, repository_selection, token_generation, repository_scope_pending \
         FROM github_installations \
         WHERE github_installation_id = $1",
        github_installation_id
    )?;
    Ok(row)
}

/// List every connection, including disconnected historical installations.
pub async fn list_installations_for_workspace(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<GitHubInstallation>> {
    let row = trakkt_core::db_fetch_all!(
        db,
        GitHubInstallation,
        "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, CAST(target_repos AS TEXT) AS target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at, \
                CAST(disconnected_at AS TEXT) AS disconnected_at, \
                CAST(uninstalled_at AS TEXT) AS uninstalled_at, \
                CAST(authorization_verified_at AS TEXT) AS authorization_verified_at, \
                github_account_id, repository_selection, token_generation, repository_scope_pending \
         FROM github_installations \
         WHERE workspace_id = $1 ORDER BY account_login ASC, created_at ASC, installation_id ASC",
        workspace_id
    )?;
    Ok(row)
}

/// Legacy single-connection helper: never silently select among multiple connections.
pub async fn get_installation_for_workspace(
    db: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<GitHubInstallation>> {
    let mut rows = list_installations_for_workspace(db, workspace_id).await?;
    if rows.len() > 1 {
        return Err(trakkt_core::Error::Conflict(
            "Select an explicit GitHub connection".into(),
        ));
    }
    Ok(rows.pop())
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
          account_login, account_type, target_repos, repository_selection, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {repos_cast}, $8, {now}) ON CONFLICT DO NOTHING"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &installation_id,
        workspace_id,
        github_app_id,
        github_installation_id,
        account_login,
        account_type,
        &target_repos_json,
        if target_repos.is_none() {
            "all"
        } else {
            "selected"
        }
    )?;

    let row = trakkt_core::db_fetch_one!(
        db,
        GitHubInstallation,
        "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, CAST(target_repos AS TEXT) AS target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at, \
                CAST(disconnected_at AS TEXT) AS disconnected_at, \
                CAST(uninstalled_at AS TEXT) AS uninstalled_at, \
                CAST(authorization_verified_at AS TEXT) AS authorization_verified_at, \
                github_account_id, repository_selection, token_generation, repository_scope_pending \
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
    let expiry = sql_compat::cast_to_timestamptz(db.is_postgres(), "$2");
    let sql = format!(
        "UPDATE github_installations \
         SET access_token_encrypted = $1, token_expires_at = {expiry} \
         WHERE installation_id = $3"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        access_token_encrypted,
        token_expires_at,
        installation_id
    )?;
    Ok(())
}

/// Lifecycle changes invalidate tokens and publish settings invalidation atomically.
async fn change_installation(
    db: &DbPool,
    installation_id: &str,
    assignments: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    let mut tx = db.begin().await?;
    let sql = format!(
        "UPDATE github_installations SET {assignments}, access_token_encrypted = NULL, token_expires_at = NULL, token_generation = token_generation + 1 WHERE installation_id = $1"
    );
    trakkt_core::tx_execute!(&mut tx, &sql, installation_id)?;
    record_installation_change(&mut tx, installation_id)
        .await?
        .commit_and_deliver(tx, ws_manager)
        .await
}

async fn record_installation_change(
    tx: &mut trakkt_core::db::DbTx,
    installation_id: &str,
) -> trakkt_core::Result<trakkt_auth::sync_log_service::SyncBatch<'static>> {
    use trakkt_auth::sync_log_service::{SyncAudience, SyncBatch};
    let workspace = trakkt_core::tx_fetch_scalar!(
        &mut *tx,
        String,
        "SELECT workspace_id FROM github_installations WHERE installation_id = $1",
        installation_id
    )?;
    let mut batch = SyncBatch::new();
    batch
        .record(
            tx,
            "GITHUB_INSTALLATION",
            installation_id,
            &workspace,
            SyncAudience::Workspace,
            trakkt_types::sync::SyncActionType::Update,
            Some(serde_json::json!({"installation_id":installation_id,"workspace_id":workspace})),
        )
        .await?;
    Ok(batch)
}

pub async fn disconnect_installation(
    db: &DbPool,
    installation_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    disconnect_installation_with_delivery(db, installation_id, workspace_id, None).await
}

pub async fn disconnect_installation_with_delivery(
    db: &DbPool,
    installation_id: &str,
    workspace_id: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    let row = get_installation_by_id(db, installation_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound("GitHub connection not found".into()))?;
    if row.workspace_id != workspace_id {
        return Err(trakkt_core::Error::Forbidden(
            "GitHub connection belongs to another workspace".into(),
        ));
    }
    let now = sql_compat::now(db.is_postgres());
    change_installation(
        db,
        installation_id,
        &format!("disconnected_at = {now}"),
        ws_manager,
    )
    .await
}

pub async fn suspend_installation(db: &DbPool, installation_id: &str) -> trakkt_core::Result<()> {
    suspend_installation_with_delivery(db, installation_id, None).await
}

pub async fn suspend_installation_with_delivery(
    db: &DbPool,
    installation_id: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    let now = sql_compat::now(db.is_postgres());
    change_installation(
        db,
        installation_id,
        &format!("suspended_at = {now}"),
        ws_manager,
    )
    .await
}

pub async fn uninstall_installation(db: &DbPool, installation_id: &str) -> trakkt_core::Result<()> {
    uninstall_installation_with_delivery(db, installation_id, None).await
}

pub async fn uninstall_installation_with_delivery(
    db: &DbPool,
    installation_id: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    let now = sql_compat::now(db.is_postgres());
    change_installation(
        db,
        installation_id,
        &format!("uninstalled_at = {now}"),
        ws_manager,
    )
    .await
}

pub async fn unsuspend_installation(db: &DbPool, installation_id: &str) -> trakkt_core::Result<()> {
    unsuspend_installation_with_delivery(db, installation_id, None).await
}

pub async fn unsuspend_installation_with_delivery(
    db: &DbPool,
    installation_id: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    change_installation(db, installation_id, "suspended_at = NULL", ws_manager).await
}

/// Authoritative scope replacement invalidates in-flight token refreshes.
pub async fn update_installation_repos(
    db: &DbPool,
    installation_id: &str,
    target_repos: Option<&serde_json::Value>,
) -> trakkt_core::Result<()> {
    update_installation_repos_with_delivery(db, installation_id, target_repos, None).await
}

pub async fn update_installation_repos_with_delivery(
    db: &DbPool,
    installation_id: &str,
    target_repos: Option<&serde_json::Value>,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    let repos = target_repos.map(serde_json::Value::to_string);
    let cast = sql_compat::cast_to_json(db.is_postgres(), "$2");
    let selection = if target_repos.is_none() {
        "all"
    } else {
        "selected"
    };
    let mut tx = db.begin().await?;
    let sql = format!(
        "UPDATE github_installations SET target_repos = {cast}, repository_selection = $3, access_token_encrypted = NULL, token_expires_at = NULL, token_generation = token_generation + 1 WHERE installation_id = $1"
    );
    trakkt_core::tx_execute!(&mut tx, &sql, installation_id, &repos, selection)?;
    record_installation_change(&mut tx, installation_id)
        .await?
        .commit_and_deliver(tx, ws_manager)
        .await
}

/// Serialize repository deltas against the current row, never a pre-webhook snapshot.
pub async fn apply_repository_delta(
    db: &DbPool,
    installation_id: &str,
    selection: &str,
    added: &[String],
    removed: &[String],
) -> trakkt_core::Result<()> {
    apply_repository_delta_with_delivery(db, installation_id, selection, added, removed, None).await
}

pub async fn apply_repository_delta_with_delivery(
    db: &DbPool,
    installation_id: &str,
    selection: &str,
    added: &[String],
    removed: &[String],
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    if !matches!(selection, "all" | "selected") {
        return Err(trakkt_core::Error::BadRequest(
            "Invalid GitHub repository selection".into(),
        ));
    }
    let is_pg = db.is_postgres();
    let mut tx = db.begin().await?;
    // This write obtains SQLite's writer lock; PostgreSQL obtains the row lock.
    trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE github_installations SET token_generation = token_generation + 1, access_token_encrypted = NULL, token_expires_at = NULL WHERE installation_id = $1",
        installation_id
    )?;
    let current = trakkt_core::tx_fetch_scalar!(
        &mut tx,
        Option<String>,
        "SELECT CAST(target_repos AS TEXT) FROM github_installations WHERE installation_id = $1",
        installation_id
    )?;
    let repos = if selection == "all" {
        None
    } else {
        let mut repos: Vec<String> = match current {
            Some(value) => serde_json::from_str(&value).map_err(|error| {
                trakkt_core::Error::Internal(format!(
                    "Invalid stored GitHub repository scope: {error}"
                ))
            })?,
            None => Vec::new(),
        };
        for repo in added {
            if !repos.contains(repo) {
                repos.push(repo.clone());
            }
        }
        repos.retain(|repo| !removed.contains(repo));
        repos.sort();
        Some(serde_json::to_string(&repos).map_err(|error| {
            trakkt_core::Error::Internal(format!("Cannot encode GitHub repository scope: {error}"))
        })?)
    };
    let cast = sql_compat::cast_to_json(is_pg, "$2");
    let sql = format!(
        "UPDATE github_installations SET target_repos = {cast}, repository_selection = $3 WHERE installation_id = $1"
    );
    trakkt_core::tx_execute!(&mut tx, &sql, installation_id, &repos, selection)?;
    record_installation_change(&mut tx, installation_id)
        .await?
        .commit_and_deliver(tx, ws_manager)
        .await
}

/// Quarantine scope before remote reconciliation; failure remains visible and denied.
pub async fn begin_repository_reconciliation(
    db: &DbPool,
    installation_id: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<i64> {
    let pending = sql_compat::bool_true(db.is_postgres());
    let mut tx = db.begin().await?;
    let sql = format!(
        "UPDATE github_installations SET repository_scope_pending = {pending}, token_generation = token_generation + 1, access_token_encrypted = NULL, token_expires_at = NULL WHERE installation_id = $1"
    );
    trakkt_core::tx_execute!(&mut tx, &sql, installation_id)?;
    let generation = trakkt_core::tx_fetch_scalar!(
        &mut tx,
        i64,
        "SELECT token_generation FROM github_installations WHERE installation_id = $1",
        installation_id
    )?;
    record_installation_change(&mut tx, installation_id)
        .await?
        .commit_and_deliver(tx, ws_manager)
        .await?;
    Ok(generation)
}

/// Never let a slow fetch replace scope after a newer disconnect or scope change.
pub async fn finish_repository_reconciliation(
    db: &DbPool,
    installation_id: &str,
    expected_generation: i64,
    target_repos: Option<&serde_json::Value>,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<bool> {
    let repos = target_repos.map(serde_json::Value::to_string);
    let cast = sql_compat::cast_to_json(db.is_postgres(), "$3");
    let pending = sql_compat::bool_false(db.is_postgres());
    let selection = if target_repos.is_none() {
        "all"
    } else {
        "selected"
    };
    let mut tx = db.begin().await?;
    let sql = format!(
        "UPDATE github_installations SET repository_scope_pending = {pending}, target_repos = {cast}, repository_selection = $4 WHERE installation_id = $1 AND token_generation = $2 AND repository_scope_pending = {}",
        sql_compat::bool_true(db.is_postgres())
    );
    let changed = trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        installation_id,
        expected_generation,
        &repos,
        selection
    )?;
    if changed.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    record_installation_change(&mut tx, installation_id)
        .await?
        .commit_and_deliver(tx, ws_manager)
        .await?;
    Ok(true)
}

/// Publish a refreshed token only if no lifecycle/scope change occurred meanwhile.
pub async fn cache_installation_token(
    db: &DbPool,
    installation: &GitHubInstallation,
    token: &str,
    expires_at: &str,
) -> trakkt_core::Result<bool> {
    let expiry = sql_compat::cast_to_timestamptz(db.is_postgres(), "$2");
    let sql = format!(
        "UPDATE github_installations SET access_token_encrypted = $1, token_expires_at = {expiry} WHERE installation_id = $3 AND token_generation = $4 AND disconnected_at IS NULL AND suspended_at IS NULL AND uninstalled_at IS NULL AND authorization_verified_at IS NOT NULL AND repository_scope_pending = {}",
        sql_compat::bool_false(db.is_postgres())
    );
    Ok(trakkt_core::db_execute!(
        db,
        &sql,
        token,
        expires_at,
        &installation.installation_id,
        installation.token_generation
    )?
    .rows_affected()
        == 1)
}

/// Serializes a connection's permitted side effect with disconnect and scope changes.
pub struct ConnectionAdmission<'a> {
    pub installation: &'a GitHubInstallation,
    pub repository: &'a str,
}

impl trakkt_core::db::TransactionAdmission for ConnectionAdmission<'_> {
    fn admit<'a>(
        &'a self,
        tx: &'a mut trakkt_core::db::DbTx,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = trakkt_core::Result<()>> + Send + 'a>>
    {
        Box::pin(async move {
            // A write obtains a row lock on PostgreSQL and the writer lock on
            // SQLite before reading authorization. Lifecycle mutations use the
            // same row, so they cannot pass admission before this commit.
            trakkt_core::tx_execute!(
                &mut *tx,
                "UPDATE github_installations SET token_generation = token_generation WHERE installation_id = $1",
                &self.installation.installation_id
            )?;
            let current = trakkt_core::tx_fetch_optional!(
                &mut *tx,
                GitHubInstallation,
                "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, CAST(target_repos AS TEXT) AS target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at, \
                CAST(disconnected_at AS TEXT) AS disconnected_at, \
                CAST(uninstalled_at AS TEXT) AS uninstalled_at, \
                CAST(authorization_verified_at AS TEXT) AS authorization_verified_at, \
                github_account_id, repository_selection, token_generation, repository_scope_pending \
         FROM github_installations \
         WHERE installation_id = $1",
                &self.installation.installation_id
            )?;
            if !current.is_some_and(|current| {
                current.workspace_id == self.installation.workspace_id
                    && current.token_generation == self.installation.token_generation
                    && current.allows_repository(self.repository)
            }) {
                return Err(trakkt_core::Error::Conflict(
                    "GitHub connection changed before mutation admission".into(),
                ));
            }
            Ok(())
        })
    }
}

/// Hold admission through an outbound request. Callers must avoid pool access
/// until this transaction is committed or dropped.
pub async fn admit_outbound(
    db: &DbPool,
    installation: &GitHubInstallation,
    repository: &str,
) -> trakkt_core::Result<trakkt_core::db::DbTx> {
    use trakkt_core::db::TransactionAdmission;
    let mut tx = db.begin().await?;
    ConnectionAdmission {
        installation,
        repository,
    }
    .admit(&mut tx)
    .await?;
    Ok(tx)
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

/// Reverse lookup: find issues linked to a commit SHA (prefix match) or branch name (exact).
///
/// Returns GitHub link rows joined with issue details (team key, number, title,
/// status, description) so callers get full ticket context.
pub async fn lookup_issues_by_ref(
    db: &DbPool,
    workspace_id: &str,
    link_type: &str,
    ref_pattern: &str,
    prefix_match: bool,
) -> trakkt_core::Result<Vec<RefLookupResult>> {
    let ref_condition = if prefix_match {
        "gl.ref_identifier LIKE $3 || '%' ESCAPE '\\'".to_string()
    } else {
        "gl.ref_identifier = $3".to_string()
    };

    let sql = format!(
        "SELECT gl.link_id, gl.ref_identifier, gl.repo_full_name, gl.title AS link_title, \
                gl.url, gl.author_login, gl.state, \
                CAST(gl.created_at AS TEXT) AS link_created_at, \
                i.issue_id, t.key AS team_key, i.number, i.title AS issue_title, \
                i.description, s.name AS status_name, s.category AS status_category \
         FROM github_links gl \
         JOIN issues i ON i.issue_id = gl.issue_id \
         JOIN teams t ON t.team_id = i.team_id \
         JOIN statuses s ON s.status_id = i.status_id \
         WHERE gl.workspace_id = $1 AND gl.link_type = $2 AND {ref_condition} \
         ORDER BY gl.created_at DESC"
    );

    let rows: Vec<RefLookupResult> = trakkt_core::db_fetch_all!(
        db,
        RefLookupResult,
        &sql,
        workspace_id,
        link_type,
        ref_pattern
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
    upsert_link_with_admission(db, params, None).await
}

/// Upsert a link while holding an optional authorization admission lock.
pub async fn upsert_link_with_admission(
    db: &DbPool,
    params: &CreateLinkParams<'_>,
    admission: Option<&dyn trakkt_core::db::TransactionAdmission>,
) -> trakkt_core::Result<GitHubLink> {
    #[cfg(test)]
    mutation_pause::wait(params.installation_id).await;
    let mut tx = db.begin().await?;
    if let Some(admission) = admission {
        admission.admit(&mut tx).await?;
    }
    let is_pg = tx.is_postgres();
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
    trakkt_core::tx_execute!(
        &mut tx,
        &sql,
        &link_id,
        params.workspace_id,
        params.issue_id,
        params.installation_id,
        params.link_type,
        params.github_id,
        params.github_node_id,
        params.repo_full_name,
        params.ref_identifier,
        params.title,
        params.state,
        params.url,
        params.author_login,
        params.close_intent
    )?;

    // Fetch the row — either newly created or updated (conflict case).
    let row = trakkt_core::tx_fetch_one!(
        &mut tx,
        GitHubLink,
        "SELECT link_id, workspace_id, issue_id, installation_id, link_type, \
                github_id, github_node_id, repo_full_name, ref_identifier, \
                title, state, url, author_login, close_intent, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(updated_at AS TEXT) AS updated_at \
         FROM github_links \
         WHERE workspace_id = $1 AND issue_id = $2 AND link_type = $3 \
               AND repo_full_name = $4 AND ref_identifier = $5",
        params.workspace_id,
        params.issue_id,
        params.link_type,
        params.repo_full_name,
        params.ref_identifier
    )?;
    tx.commit().await?;
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
    delete_links_with_admission(
        db,
        workspace_id,
        link_type,
        repo_full_name,
        ref_identifier,
        keep_issue_ids,
        None,
    )
    .await
}

/// Delete stale links while holding authorization through the commit.
pub async fn delete_links_with_admission(
    db: &DbPool,
    workspace_id: &str,
    link_type: &str,
    repo_full_name: &str,
    ref_identifier: &str,
    keep_issue_ids: &[String],
    admission: Option<&dyn trakkt_core::db::TransactionAdmission>,
) -> trakkt_core::Result<u64> {
    let mut tx = db.begin().await?;
    if let Some(admission) = admission {
        admission.admit(&mut tx).await?;
    }
    if keep_issue_ids.is_empty() {
        // Delete all links for this object in this workspace
        let result = trakkt_core::tx_execute!(
            &mut tx,
            "DELETE FROM github_links \
             WHERE workspace_id = $1 AND link_type = $2 \
                   AND repo_full_name = $3 AND ref_identifier = $4",
            workspace_id,
            link_type,
            repo_full_name,
            ref_identifier
        )?;
        let affected = result.rows_affected();
        tx.commit().await?;
        return Ok(affected);
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
    let rows_affected: u64 = trakkt_core::tx_with!(&mut tx, |executor| {
        let mut query = sqlx::query(&sql)
            .bind(workspace_id)
            .bind(link_type)
            .bind(repo_full_name)
            .bind(ref_identifier);
        for id in keep_issue_ids {
            query = query.bind(id);
        }
        let result = query.execute(executor).await?;
        Ok::<u64, sqlx::Error>(result.rows_affected())
    })?;

    tx.commit().await?;
    Ok(rows_affected)
}

/// Find all PR links for a specific PR (by workspace, repo, and PR number).
///
/// Used during transition processing to find all issues linked to a given PR.
pub async fn list_pr_links_by_ref(
    db: &DbPool,
    workspace_id: &str,
    repo_full_name: &str,
    ref_identifier: &str,
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
         WHERE workspace_id = $1 AND link_type = $2 \
               AND repo_full_name = $3 AND ref_identifier = $4",
        workspace_id,
        "pull_request",
        repo_full_name,
        ref_identifier
    )?;
    Ok(rows)
}

/// Find all PR links for a specific issue that have close_intent set.
///
/// Used for outbound notifications when an issue is manually completed.
pub async fn list_close_intent_links_for_issue(
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
         WHERE issue_id = $1 AND link_type = $2 AND close_intent = $3",
        issue_id,
        "pull_request",
        true
    )?;
    Ok(rows)
}

/// Look up an installation by its internal ID.
pub async fn get_installation_by_id(
    db: &DbPool,
    installation_id: &str,
) -> trakkt_core::Result<Option<GitHubInstallation>> {
    let row = trakkt_core::db_fetch_optional!(
        db,
        GitHubInstallation,
        "SELECT installation_id, workspace_id, github_app_id, github_installation_id, \
                account_login, account_type, CAST(target_repos AS TEXT) AS target_repos, \
                access_token_encrypted, \
                CAST(token_expires_at AS TEXT) AS token_expires_at, \
                CAST(created_at AS TEXT) AS created_at, \
                CAST(suspended_at AS TEXT) AS suspended_at, \
                CAST(disconnected_at AS TEXT) AS disconnected_at, \
                CAST(uninstalled_at AS TEXT) AS uninstalled_at, \
                CAST(authorization_verified_at AS TEXT) AS authorization_verified_at, \
                github_account_id, repository_selection, token_generation, repository_scope_pending \
         FROM github_installations \
         WHERE installation_id = $1",
        installation_id
    )?;
    Ok(row)
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
        db,
        &sql,
        &event_id,
        github_delivery_id,
        installation_id,
        event_type,
        action,
        &payload_json
    )?;
    Ok(event_id)
}

/// Mark an event as successfully processed.
pub async fn mark_event_processed(db: &DbPool, event_id: &str) -> trakkt_core::Result<()> {
    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!("UPDATE github_events SET processed_at = {now} WHERE event_id = $1");
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
        error,
        event_id
    )?;
    Ok(())
}

/// Check whether an event with the given delivery ID has already been recorded.
///
/// Used for idempotency — GitHub may redeliver webhooks.
pub async fn event_exists(db: &DbPool, github_delivery_id: &str) -> trakkt_core::Result<bool> {
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
    trakkt_core::db_execute!(db, &sql, &rule_id_1, workspace_id, "pr_opened", "started")?;

    // pr_merged -> completed (close intent required)
    let sql = format!(
        "INSERT INTO github_transition_rules \
         (rule_id, workspace_id, trigger_event, close_intent_required, \
          target_status_category, enabled, created_at) \
         VALUES ($1, $2, $3, {bool_true}, $4, {bool_true}, {now}) \
         ON CONFLICT DO NOTHING"
    );
    trakkt_core::db_execute!(db, &sql, &rule_id_2, workspace_id, "pr_merged", "completed")?;

    // pr_closed -> cancelled (close intent required)
    let sql = format!(
        "INSERT INTO github_transition_rules \
         (rule_id, workspace_id, trigger_event, close_intent_required, \
          target_status_category, enabled, created_at) \
         VALUES ($1, $2, $3, {bool_true}, $4, {bool_true}, {now}) \
         ON CONFLICT DO NOTHING"
    );
    trakkt_core::db_execute!(db, &sql, &rule_id_3, workspace_id, "pr_closed", "cancelled")?;

    Ok(())
}

/// Update the `enabled` flag on a single transition rule.
pub async fn update_transition_rule_enabled(
    db: &DbPool,
    rule_id: &str,
    workspace_id: &str,
    enabled: bool,
) -> trakkt_core::Result<()> {
    update_transition_rule_enabled_with_delivery(db, rule_id, workspace_id, enabled, None).await
}

pub async fn update_transition_rule_enabled_with_delivery(
    db: &DbPool,
    rule_id: &str,
    workspace_id: &str,
    enabled: bool,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> trakkt_core::Result<()> {
    use trakkt_auth::sync_log_service::{SyncAudience, SyncBatch};
    let mut tx = db.begin().await?;
    let changed = trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE github_transition_rules SET enabled = $1 WHERE rule_id = $2 AND workspace_id = $3",
        enabled,
        rule_id,
        workspace_id
    )?;
    if changed.rows_affected() != 1 {
        return Err(trakkt_core::Error::NotFound(
            "GitHub automation rule not found".into(),
        ));
    }
    let mut batch = SyncBatch::new();
    batch.record(&mut tx, trakkt_types::sync::entity_types::GITHUB_TRANSITION_RULE, rule_id, workspace_id, SyncAudience::Workspace, trakkt_types::sync::SyncActionType::Update, Some(serde_json::json!({"rule_id":rule_id,"workspace_id":workspace_id,"enabled":enabled}))).await?;
    batch.commit_and_deliver(tx, ws_manager).await
}

#[cfg(test)]
pub(crate) mod mutation_pause {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};
    use tokio::sync::Notify;
    type Pause = (Arc<Notify>, Arc<Notify>);
    static PAUSES: LazyLock<Mutex<HashMap<String, Pause>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    pub fn register(id: &str) -> Pause {
        let pause = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        PAUSES
            .lock()
            .expect("mutation pause registry")
            .insert(id.into(), pause.clone());
        pause
    }
    pub async fn wait(id: &str) {
        let pause = PAUSES.lock().expect("mutation pause registry").remove(id);
        if let Some((reached, resume)) = pause {
            reached.notify_one();
            resume.notified().await;
        }
    }
}
