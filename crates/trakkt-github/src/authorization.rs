// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace-bound, single-use GitHub App user authorization. App credentials
//! alone never confer authority to attach an installation to a workspace.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::TryRngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use trakkt_auth::sync_log_service::{SyncAudience, SyncBatch};
use trakkt_core::{DbPool, Error, Result, sql_compat};
use trakkt_types::sync::SyncActionType;

use crate::{GitHubClient, GitHubInstallationDetails};

const STATE_LIFETIME_SECONDS: i64 = 600;

pub(crate) struct OAuthConfig {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    callback_url: String,
}

impl OAuthConfig {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let values = [
            "GITHUB_OAUTH_CLIENT_ID",
            "GITHUB_OAUTH_CLIENT_SECRET",
            "GITHUB_OAUTH_CALLBACK_URL",
        ]
        .map(|name| std::env::var(name).ok());
        if values[0].is_none() && values[1].is_none() {
            return Ok(None);
        }
        let [Some(client_id), Some(client_secret), Some(callback_url)] = values else {
            return Err(Error::Internal("Set all three GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET and GITHUB_OAUTH_CALLBACK_URL variables".into()));
        };
        if [&client_id, &client_secret].iter().any(|v| {
            v.trim().is_empty()
                || v.contains(char::is_whitespace)
                || matches!(v.as_str(), "placeholder" | "changeme")
        }) {
            return Err(Error::Internal(
                "GitHub OAuth client credentials must be nonempty real credentials".into(),
            ));
        }
        let url = reqwest::Url::parse(&callback_url).map_err(|_| {
            Error::Internal("GITHUB_OAUTH_CALLBACK_URL must be an absolute URL".into())
        })?;
        if (url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))))
            || url.path() != "/integrations/github/oauth/callback"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::Internal("GITHUB_OAUTH_CALLBACK_URL must use HTTPS and the distinct /integrations/github/oauth/callback path (HTTP localhost allowed for development)".into()));
        }
        Ok(Some(Self {
            client_id,
            client_secret,
            callback_url,
        }))
    }
}

fn invalid_state() -> Error {
    Error::Forbidden("GitHub connection authorization is missing, expired or already used. Start again from integration settings.".into())
}
fn conflict() -> Error {
    Error::Conflict("This GitHub account or installation is already reserved. Contact your administrator; connections cannot be transferred here.".into())
}
fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn random_secret() -> Result<String> {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| Error::Internal("Secure randomness unavailable".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(sqlx::FromRow)]
struct ConnectionState {
    state_hash: String,
    user_id: String,
    workspace_id: String,
    action: String,
    expected_installation_id: Option<i64>,
    expected_account_id: Option<i64>,
    expected_token_generation: Option<i64>,
    installation_id: Option<i64>,
    verifier_encrypted: String,
    expires_at: i64,
    phase: String,
}

/// A descriptor can only be created by successful GitHub user verification.
/// Its private fields prevent browser metadata or App JWTs acting as authority.
pub struct VerifiedConnection {
    pub(crate) app_id: i64,
    pub(crate) account_id: i64,
    pub(crate) account_type: String,
    pub(crate) account_login: String,
    pub(crate) installation_id: i64,
    pub(crate) repository_selection: String,
    pub(crate) repos: Option<serde_json::Value>,
}

impl VerifiedConnection {
    pub fn app_id(&self) -> i64 {
        self.app_id
    }
    pub fn account_id(&self) -> i64 {
        self.account_id
    }
    pub fn account_type(&self) -> &str {
        &self.account_type
    }
    pub fn account_login(&self) -> &str {
        &self.account_login
    }
    pub fn repositories(&self) -> Option<&serde_json::Value> {
        self.repos.as_ref()
    }
    pub fn installation_id(&self) -> i64 {
        self.installation_id
    }
    pub fn repository_selection(&self) -> &str {
        &self.repository_selection
    }
}

/// Always read current membership; session roles and active-workspace claims
/// can change while the user is on GitHub.
pub async fn require_admin(db: &DbPool, user_id: &str, workspace_id: &str) -> Result<()> {
    let sql = format!(
        "SELECT COUNT(*) FROM workspace_users WHERE user_id = $1 AND workspace_id = $2 AND role = 'workspace_admin' AND active = {}",
        sql_compat::bool_true(db.is_postgres())
    );
    let count = trakkt_core::db_fetch_scalar!(db, i64, &sql, user_id, workspace_id)?;
    if count == 0 {
        return Err(Error::Forbidden(
            "Current workspace administrator access required".into(),
        ));
    }
    Ok(())
}

async fn load_state(
    db: &DbPool,
    state: &str,
    user_id: &str,
    phase: &str,
) -> Result<ConnectionState> {
    if state.len() != 43 {
        return Err(invalid_state());
    }
    let row = trakkt_core::db_fetch_optional!(db, ConnectionState,
        "SELECT state_hash, user_id, workspace_id, action, expected_installation_id, expected_account_id, expected_token_generation, installation_id, verifier_encrypted, expires_at, phase FROM github_connection_states WHERE state_hash = $1", hash(state))?
        .ok_or_else(invalid_state)?;
    if row.user_id != user_id
        || row.phase != phase
        || row.expires_at <= chrono::Utc::now().timestamp()
    {
        return Err(invalid_state());
    }
    require_admin(db, user_id, &row.workspace_id).await?;
    Ok(row)
}

impl GitHubClient {
    pub fn user_authorization_configured(&self) -> bool {
        self.oauth.is_some()
    }
    fn oauth_config(&self) -> Result<&OAuthConfig> {
        self.oauth.as_ref().ok_or_else(|| Error::BadRequest("GitHub user authorization is not configured. An administrator must set GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET and GITHUB_OAUTH_CALLBACK_URL; see the GitHub configuration guide.".into()))
    }
    fn authorization_url(&self, state: &str, verifier: &str) -> Result<String> {
        let oauth = self.oauth_config()?;
        let mut url = reqwest::Url::parse(&format!("{}/login/oauth/authorize", self.oauth_base))
            .map_err(|_| Error::Internal("Invalid GitHub authorization endpoint".into()))?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", oauth.client_id.as_str()),
            ("redirect_uri", oauth.callback_url.as_str()),
            ("state", state),
            (
                "code_challenge",
                URL_SAFE_NO_PAD
                    .encode(Sha256::digest(verifier.as_bytes()))
                    .as_str(),
            ),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ]);
        Ok(url.into())
    }
    async fn user_get<T: serde::de::DeserializeOwned>(&self, path: &str, token: &str) -> Result<T> {
        // No response bodies, URLs with credentials, or tokens enter errors/logs.
        let response = self
            .authorization_http
            .get(format!("{}{path}", self.api_base))
            .headers(crate::api_headers(token))
            .send()
            .await
            .map_err(|_| Error::Internal("GitHub user verification request failed".into()))?;
        if !response.status().is_success() {
            return Err(Error::Forbidden(
                "GitHub user verification failed; check access and authorize again".into(),
            ));
        }
        response
            .json()
            .await
            .map_err(|_| Error::Internal("Invalid GitHub user verification response".into()))
    }
    async fn verify_connection(
        &self,
        code: &str,
        verifier: &str,
        installation_id: i64,
    ) -> Result<VerifiedConnection> {
        let oauth = self.oauth_config()?;
        if code.is_empty() || code.len() > 1024 || installation_id <= 0 {
            return Err(invalid_state());
        }
        #[derive(Deserialize)]
        struct Token {
            access_token: Option<String>,
            token_type: Option<String>,
            error: Option<String>,
        }
        let response = self
            .authorization_http
            .post(format!("{}/login/oauth/access_token", self.oauth_base))
            .header("Accept", "application/json")
            .form(&[
                ("client_id", oauth.client_id.as_str()),
                ("client_secret", oauth.client_secret.as_str()),
                ("redirect_uri", oauth.callback_url.as_str()),
                ("code", code),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|_| Error::Internal("GitHub authorization exchange failed".into()))?;
        if !response.status().is_success() {
            return Err(Error::Forbidden(
                "GitHub authorization exchange rejected".into(),
            ));
        }
        let token: Token = response
            .json()
            .await
            .map_err(|_| Error::Forbidden("Invalid GitHub authorization response".into()))?;
        if token.error.is_some() || token.token_type.as_deref() != Some("bearer") {
            return Err(Error::Forbidden("GitHub authorization rejected".into()));
        }
        let token = token
            .access_token
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                Error::Forbidden("GitHub authorization returned no user token".into())
            })?;
        #[derive(Deserialize)]
        struct User {
            id: i64,
            #[serde(rename = "type")]
            account_type: String,
        }
        let user: User = self.user_get("/user", &token).await?;
        if user.id <= 0 || user.account_type != "User" {
            return Err(Error::Forbidden(
                "GitHub user identity could not be verified".into(),
            ));
        }
        #[derive(Deserialize)]
        struct Installations {
            total_count: usize,
            installations: Vec<GitHubInstallationDetails>,
        }
        let mut selected = None;
        let mut seen = 0;
        for page in 1..=1000 {
            let result: Installations = self
                .user_get(
                    &format!("/user/installations?per_page=100&page={page}"),
                    &token,
                )
                .await?;
            let len = result.installations.len();
            seen += len;
            for installation in result.installations {
                if installation.id == installation_id as u64 {
                    selected = Some(installation);
                }
            }
            if seen >= result.total_count {
                break;
            }
            if len == 0 || page == 1000 {
                return Err(Error::Internal(
                    "Incomplete GitHub installation pagination".into(),
                ));
            }
        }
        let details = selected.ok_or_else(|| {
            Error::Forbidden(
                "The authorized GitHub user cannot access the selected installation".into(),
            )
        })?;
        if details.app_id != self.app_id
            || details.suspended_at.is_some()
            || details.account.id == 0
            || i64::try_from(details.account.id).is_err()
            || !matches!(
                details.account.account_type.as_str(),
                "User" | "Organization"
            )
            || details.target_type != details.account.account_type
            || !matches!(details.repository_selection.as_str(), "all" | "selected")
            || (details.account.account_type == "User" && details.account.id != user.id as u64)
        {
            return Err(Error::Forbidden(
                "GitHub installation does not match the authorized App or account".into(),
            ));
        }
        // Compare the user-visible association with fresh App metadata. The App
        // response corroborates identity; user authorization remains required.
        let current = self
            .get_installation_details(installation_id as u64)
            .await?;
        if current.id != details.id
            || current.app_id != details.app_id
            || current.account.id != details.account.id
            || current.account.account_type != details.account.account_type
            || current.suspended_at.is_some()
            || current.repository_selection != details.repository_selection
        {
            return Err(Error::Forbidden(
                "GitHub installation changed during authorization; start again".into(),
            ));
        }
        // The user repository endpoint also revalidates access after pagination.
        #[derive(Deserialize)]
        struct UserRepositories {
            total_count: usize,
        }
        let accessible: UserRepositories = self
            .user_get(
                &format!("/user/installations/{installation_id}/repositories?per_page=1"),
                &token,
            )
            .await?;
        let _accessible_count = accessible.total_count;
        let repos =
            if details.repository_selection == "selected" {
                let installation_token = self
                    .request_installation_token(installation_id as u64)
                    .await?;
                let mut names = Vec::new();
                #[derive(Deserialize)]
                struct Repos {
                    total_count: usize,
                    repositories: Vec<crate::GitHubRepository>,
                }
                for page in 1..=1000 {
                    let result: Repos = self
                        .user_get(
                            &format!("/installation/repositories?per_page=100&page={page}"),
                            &installation_token.token,
                        )
                        .await?;
                    let len = result.repositories.len();
                    names.extend(result.repositories.into_iter().map(|repo| repo.full_name));
                    if names.len() >= result.total_count {
                        break;
                    }
                    if len == 0 || page == 1000 {
                        return Err(Error::Internal(
                            "Incomplete GitHub repository pagination".into(),
                        ));
                    }
                }
                Some(serde_json::to_value(names).map_err(|_| {
                    Error::Internal("Cannot encode GitHub repository selection".into())
                })?)
            } else {
                None
            };
        // Revalidate the user after App repository pagination; a revoked user
        // token must not fall back to the still-valid installation token.
        let final_user: User = self.user_get("/user", &token).await?;
        let final_access: UserRepositories = self
            .user_get(
                &format!("/user/installations/{installation_id}/repositories?per_page=1"),
                &token,
            )
            .await?;
        let _final_access_count = final_access.total_count;
        if final_user.id != user.id || final_user.account_type != "User" {
            return Err(Error::Forbidden(
                "GitHub user access changed during authorization".into(),
            ));
        }
        Ok(VerifiedConnection {
            app_id: self.app_id as i64,
            account_id: details.account.id as i64,
            account_type: details.account.account_type,
            account_login: details.account.login,
            installation_id,
            repository_selection: details.repository_selection,
            repos,
        })
    }
}

/// Admin-only start. Reconnect skips installation setup and authorizes the
/// existing installation directly, without requiring uninstall/reinstall.
pub async fn start_connection(
    db: &DbPool,
    client: &GitHubClient,
    key: &[u8; 32],
    user_id: &str,
    workspace_id: &str,
    connection_id: Option<&str>,
    reinstall: bool,
) -> Result<String> {
    client.oauth_config()?;
    require_admin(db, user_id, workspace_id).await?;
    let existing = if let Some(id) = connection_id {
        let row = crate::schema::get_installation_by_id(db, id)
            .await?
            .ok_or_else(|| Error::NotFound("GitHub connection not found".into()))?;
        if row.workspace_id != workspace_id {
            return Err(Error::Forbidden(
                "GitHub connection belongs to another workspace".into(),
            ));
        }
        Some(row)
    } else {
        None
    };
    let state = random_secret()?;
    let verifier = random_secret()?;
    let verifier_encrypted = trakkt_auth::encryption::encrypt(&verifier, key)?;
    let expires_at = chrono::Utc::now().timestamp() + STATE_LIFETIME_SECONDS;
    let expected_installation = existing.as_ref().map(|v| v.github_installation_id);
    let expected_account: Option<i64> = if let Some(existing) = &existing {
        trakkt_core::db_fetch_scalar!(
            db,
            Option<i64>,
            "SELECT github_account_id FROM github_installations WHERE installation_id = $1",
            &existing.installation_id
        )?
    } else {
        None
    };
    let action = if existing.is_some() {
        "reconnect"
    } else {
        "connect"
    };
    let phase = if existing.is_some() && !reinstall {
        "oauth"
    } else {
        "setup"
    };
    trakkt_core::db_execute!(
        db,
        "DELETE FROM github_connection_states WHERE expires_at <= $1",
        chrono::Utc::now().timestamp()
    )?;
    let inserted = trakkt_core::db_execute!(
        db,
        "INSERT INTO github_connection_states (state_hash, user_id, workspace_id, action, expected_installation_id, expected_account_id, installation_id, verifier_encrypted, expires_at, phase, expected_token_generation) VALUES ($1, $2, $3, $4, $5, $6, $5, $7, $8, $9, $10) ON CONFLICT DO NOTHING",
        hash(&state),
        user_id,
        workspace_id,
        action,
        expected_installation,
        expected_account,
        verifier_encrypted,
        expires_at,
        phase,
        existing.as_ref().map(|row| row.token_generation)
    )?;
    if inserted.rows_affected() != 1 {
        return Err(Error::Internal(
            "Could not allocate GitHub connection authorization".into(),
        ));
    }
    if existing.is_some() && !reinstall {
        client.authorization_url(&state, &verifier)
    } else {
        let mut url = reqwest::Url::parse(&format!(
            "{}/apps/{}/installations/new",
            client.oauth_base, client.app_name
        ))
        .map_err(|_| Error::Internal("Invalid GitHub App slug".into()))?;
        url.query_pairs_mut().append_pair("state", &state);
        Ok(url.into())
    }
}

/// Direct GitHub installs are an untrusted candidate, never authorization.
/// Only an explicit authenticated administrator confirmation starts fresh state.
pub async fn start_direct_connection(
    db: &DbPool,
    client: &GitHubClient,
    key: &[u8; 32],
    user_id: &str,
    workspace_id: &str,
    installation_id: i64,
) -> Result<String> {
    if installation_id <= 0 {
        return Err(invalid_state());
    }
    require_admin(db, user_id, workspace_id).await?;
    let own = crate::schema::list_installations_for_workspace(db, workspace_id).await?
        .into_iter().find(|row| row.github_installation_id == installation_id);
    if let Some(row) = own {
        return start_connection(db, client, key, user_id, workspace_id, Some(&row.installation_id), false).await;
    }
    let setup_url = start_connection(db, client, key, user_id, workspace_id, None, false).await?;
    let state = reqwest::Url::parse(&setup_url)
        .map_err(|_| Error::Internal("Invalid GitHub setup URL".into()))?
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(invalid_state)?;
    advance_setup(db, client, key, user_id, &state, installation_id, "install").await
}

/// Setup IDs select a candidate only; this never changes a connection.
/// Rotate the state between setup and OAuth so the setup URL cannot replay as
/// an authorization response.
pub async fn advance_setup(
    db: &DbPool,
    client: &GitHubClient,
    key: &[u8; 32],
    user_id: &str,
    state: &str,
    installation_id: i64,
    setup_action: &str,
) -> Result<String> {
    client.oauth_config()?;
    if installation_id <= 0 || setup_action != "install" {
        return Err(invalid_state());
    }
    let current = load_state(db, state, user_id, "setup").await?;
    let next = random_secret()?;
    let verifier = trakkt_auth::encryption::decrypt(&current.verifier_encrypted, key)?;
    let result = trakkt_core::db_execute!(
        db,
        "UPDATE github_connection_states SET state_hash = $1, installation_id = $2, phase = 'oauth' WHERE state_hash = $3 AND phase = 'setup' AND expires_at > $4",
        hash(&next),
        installation_id,
        &current.state_hash,
        chrono::Utc::now().timestamp()
    )?;
    if result.rows_affected() != 1 {
        return Err(invalid_state());
    }
    client.authorization_url(&next, &verifier)
}

/// Verify user authority then atomically consume state and claim ownership.
pub async fn complete_connection(
    db: &DbPool,
    client: &GitHubClient,
    key: &[u8; 32],
    user_id: &str,
    state: &str,
    code: &str,
) -> Result<()> {
    complete_connection_with_delivery(db, client, key, user_id, state, code, None).await.map(|_| ())
}

pub async fn complete_connection_with_delivery(
    db: &DbPool,
    client: &GitHubClient,
    key: &[u8; 32],
    user_id: &str,
    state: &str,
    code: &str,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> Result<String> {
    client.oauth_config()?;
    let current = load_state(db, state, user_id, "oauth").await?;
    let installation_id = current.installation_id.ok_or_else(invalid_state)?;
    let verifier = trakkt_auth::encryption::decrypt(&current.verifier_encrypted, key)?;
    let verified = client
        .verify_connection(code, &verifier, installation_id)
        .await?;
    if current.action == "reconnect"
        && (current
            .expected_account_id
            .is_some_and(|id| id != verified.account_id)
            || (current.expected_account_id.is_none()
                && current.expected_installation_id != Some(verified.installation_id)))
    {
        return Err(Error::Forbidden(
            "Reconnect must authorize the same GitHub account".into(),
        ));
    }
    bind_verified_connection(db, user_id, &current, &verified, ws_manager).await?;
    Ok(current.workspace_id)
}

/// Atomic reusable ownership primitive. Must be invoked inside the binding
/// transaction; claims are permanent across disconnect and installation changes.
pub async fn claim_verified_ownership(
    tx: &mut trakkt_core::db::DbTx,
    workspace_id: &str,
    verified: &VerifiedConnection,
) -> Result<()> {
    trakkt_core::tx_execute!(
        &mut *tx,
        "INSERT INTO github_account_claims (app_id, account_id, account_type, workspace_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        verified.app_id,
        verified.account_id,
        &verified.account_type,
        workspace_id
    )?;
    let owner: (String, String) = trakkt_core::tx_fetch_one!(
        &mut *tx,
        (String, String),
        "SELECT workspace_id, account_type FROM github_account_claims WHERE app_id = $1 AND account_id = $2",
        verified.app_id,
        verified.account_id
    )?;
    if owner.0 != workspace_id || owner.1 != verified.account_type {
        return Err(conflict());
    }
    trakkt_core::tx_execute!(
        &mut *tx,
        "INSERT INTO github_installation_claims (installation_id, app_id, account_id, workspace_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        verified.installation_id,
        verified.app_id,
        verified.account_id,
        workspace_id
    )?;
    let owner: (String, i64, i64) = trakkt_core::tx_fetch_one!(
        &mut *tx,
        (String, i64, i64),
        "SELECT workspace_id, app_id, account_id FROM github_installation_claims WHERE installation_id = $1",
        verified.installation_id
    )?;
    if owner.0 != workspace_id
        || owner.1 != verified.app_id
        || (owner.2 != 0 && owner.2 != verified.account_id)
    {
        return Err(conflict());
    }
    trakkt_core::tx_execute!(
        &mut *tx,
        "UPDATE github_installation_claims SET account_id = $1 WHERE installation_id = $2 AND account_id = 0",
        verified.account_id,
        verified.installation_id
    )?;
    Ok(())
}

async fn bind_verified_connection(
    db: &DbPool,
    user_id: &str,
    state: &ConnectionState,
    verified: &VerifiedConnection,
    ws_manager: Option<&trakkt_auth::websocket::WebSocketManager>,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let active = sql_compat::bool_true(is_pg);
    let admin_sql = format!(
        "SELECT COUNT(*) FROM workspace_users WHERE user_id = $1 AND workspace_id = $2 AND role = 'workspace_admin' AND active = {active}"
    );
    let mut tx = db.begin().await?;
    // Lock the membership on Postgres so a concurrent revocation serializes
    // with this commit. SQLite's first write below takes the writer lock.
    let consumed = trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE github_connection_states SET phase = 'consumed', verifier_encrypted = '' WHERE state_hash = $1 AND user_id = $2 AND phase = 'oauth' AND expires_at > $3",
        &state.state_hash,
        user_id,
        chrono::Utc::now().timestamp()
    )?;
    if consumed.rows_affected() != 1 {
        return Err(invalid_state());
    }
    if is_pg {
        let _: (String,) = trakkt_core::tx_fetch_one!(
            &mut tx,
            (String,),
            "SELECT workspace_id FROM workspaces WHERE workspace_id = $1 FOR NO KEY UPDATE",
            &state.workspace_id
        )?;
        let _: Vec<(i32,)> = trakkt_core::tx_fetch_all!(
            &mut tx,
            (i32,),
            "SELECT id FROM workspace_users WHERE user_id = $1 AND workspace_id = $2 FOR UPDATE",
            user_id,
            &state.workspace_id
        )?;
    }
    #[cfg(test)]
    crate::schema::mutation_pause::wait(&format!("workspace-lock-{}", state.state_hash)).await;
    let admins =
        trakkt_core::tx_fetch_scalar!(&mut tx, i64, &admin_sql, user_id, &state.workspace_id)?;
    if admins == 0 {
        return Err(Error::Forbidden(
            "Current workspace administrator access required".into(),
        ));
    }
    // Historical rows predate stable IDs. Never let an unresolved foreign row
    // silently lose ownership following a rename or uninstall/reinstall.
    let unresolved = trakkt_core::tx_fetch_scalar!(
        &mut tx,
        i64,
        "SELECT COUNT(*) FROM github_installations WHERE github_account_id IS NULL AND workspace_id <> $1",
        &state.workspace_id
    )?;
    let own_exact_reconnect = state.action == "reconnect"
        && state.expected_installation_id == Some(verified.installation_id);
    if unresolved > 0 && !own_exact_reconnect {
        return Err(Error::Conflict("Existing GitHub connections require verified account identity backfill before another workspace can connect. Contact your administrator.".into()));
    }
    if state.action == "reconnect" {
        let generation_sql = format!(
            "SELECT token_generation FROM github_installations WHERE github_installation_id = $1 AND workspace_id = $2{}",
            if is_pg { " FOR UPDATE" } else { "" }
        );
        let generation = trakkt_core::tx_fetch_optional!(
            &mut tx,
            (i64,),
            &generation_sql,
            state.expected_installation_id,
            &state.workspace_id
        )?;
        if generation.map(|row| row.0) != state.expected_token_generation {
            return Err(Error::Conflict("GitHub connection changed during authorization. Start again from its settings card.".into()));
        }
    }
    claim_verified_ownership(&mut tx, &state.workspace_id, verified).await?;
    let app_id = trakkt_core::tx_fetch_scalar!(
        &mut tx,
        String,
        "SELECT github_app_id FROM github_apps WHERE app_id = $1",
        verified.app_id
    )?;
    let existing_sql = format!(
        "SELECT installation_id, github_installation_id, github_account_id, (disconnected_at IS NULL AND suspended_at IS NULL AND uninstalled_at IS NULL AND authorization_verified_at IS NOT NULL AND repository_scope_pending = {}) AS active FROM github_installations WHERE github_installation_id = $1 AND workspace_id = $2{}",
        sql_compat::bool_false(is_pg),
        if is_pg { " FOR UPDATE" } else { "" }
    );
    let existing: Option<(String, i64, Option<i64>, bool)> = trakkt_core::tx_fetch_optional!(
        &mut tx,
        (String, i64, Option<i64>, bool),
        &existing_sql,
        verified.installation_id,
        &state.workspace_id
    )?;
    let repos = verified.repos.as_ref().map(serde_json::Value::to_string);
    let repos_cast = sql_compat::cast_to_json(is_pg, "$6");
    let id = existing
        .as_ref()
        .map(|v| v.0.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let action = if let Some(existing) = existing {
        if (existing.2.is_some_and(|id| id != verified.account_id))
            || (existing.2.is_none() && existing.1 != verified.installation_id)
        {
            return Err(Error::Forbidden(
                "Reconnect must authorize the existing GitHub account".into(),
            ));
        }
        // Add-account callbacks may rediscover an active identity, but are not
        // generation-bound reconnects. Consume them idempotently without
        // changing permissions, credentials, or lifecycle state.
        if state.action != "reconnect"
            || state.expected_installation_id != Some(verified.installation_id)
        {
            if !existing.3 {
                return Err(Error::Conflict(
                    "This GitHub connection requires an explicit reconnect from its settings card."
                        .into(),
                ));
            }
            tx.commit().await?;
            return Ok(());
        }
        let sql = format!(
            "UPDATE github_installations SET github_account_id = $2, account_login = $3, account_type = $4, suspended_at = NULL, disconnected_at = NULL, uninstalled_at = NULL, authorization_verified_at = {}, access_token_encrypted = NULL, token_expires_at = NULL, token_generation = token_generation + 1, repository_scope_pending = {}, repository_selection = $7, target_repos = {repos_cast} WHERE installation_id = $5 AND github_installation_id = $1",
            sql_compat::now(is_pg),
            sql_compat::bool_false(is_pg)
        );
        trakkt_core::tx_execute!(
            &mut tx,
            &sql,
            verified.installation_id,
            verified.account_id,
            &verified.account_login,
            &verified.account_type,
            &id,
            &repos,
            &verified.repository_selection
        )?;
        SyncActionType::Update
    } else {
        let sql = format!(
            "INSERT INTO github_installations (installation_id, workspace_id, github_app_id, github_installation_id, github_account_id, target_repos, account_login, account_type, repository_selection, authorization_verified_at) VALUES ($1, $2, $3, $4, $5, {repos_cast}, $7, $8, $9, {}) ON CONFLICT DO NOTHING",
            sql_compat::now(is_pg)
        );
        let inserted = trakkt_core::tx_execute!(
            &mut tx,
            &sql,
            &id,
            &state.workspace_id,
            &app_id,
            verified.installation_id,
            verified.account_id,
            &repos,
            &verified.account_login,
            &verified.account_type,
            &verified.repository_selection
        )?;
        if inserted.rows_affected() != 1 {
            return Err(conflict());
        }
        SyncActionType::Insert
    };
    let mut batch = SyncBatch::new();
    if let Some(previous) = state
        .expected_installation_id
        .filter(|previous| *previous != verified.installation_id)
    {
        let previous_id = trakkt_core::tx_fetch_optional!(
            &mut tx,
            (String,),
            "SELECT installation_id FROM github_installations WHERE github_installation_id = $1 AND workspace_id = $2",
            previous,
            &state.workspace_id
        )?;
        if let Some((previous_id,)) = previous_id {
            let sql = format!(
                "UPDATE github_installations SET uninstalled_at = {}, access_token_encrypted = NULL, token_expires_at = NULL, token_generation = token_generation + 1 WHERE installation_id = $1",
                sql_compat::now(is_pg)
            );
            trakkt_core::tx_execute!(&mut tx, &sql, &previous_id)?;
            batch.record(&mut tx, trakkt_types::sync::entity_types::GITHUB_INSTALLATION, &previous_id, &state.workspace_id, SyncAudience::Workspace, SyncActionType::Update, Some(serde_json::json!({"installation_id":previous_id,"workspace_id":state.workspace_id}))).await?;
        }
    }
    batch.record(&mut tx, "GITHUB_INSTALLATION", &id, &state.workspace_id, SyncAudience::Workspace, action, Some(serde_json::json!({"installation_id": id, "workspace_id": state.workspace_id, "github_installation_id": verified.installation_id, "account_login": verified.account_login, "account_type": verified.account_type, "github_account_id": verified.account_id, "target_repos": verified.repos}))).await?;
    for (event, close_intent, category) in [
        ("pr_opened", false, "started"),
        ("pr_merged", true, "completed"),
        ("pr_closed", true, "cancelled"),
    ] {
        let rule_id = uuid::Uuid::new_v4().to_string();
        let inserted = trakkt_core::tx_execute!(
            &mut tx,
            "INSERT INTO github_transition_rules (rule_id, workspace_id, trigger_event, close_intent_required, target_status_category, enabled) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            &rule_id,
            &state.workspace_id,
            event,
            close_intent,
            category,
            true
        )?;
        if inserted.rows_affected() == 1 {
            batch.record(&mut tx, "GITHUB_TRANSITION_RULE", &rule_id, &state.workspace_id, SyncAudience::Workspace, SyncActionType::Insert, Some(serde_json::json!({"rule_id": rule_id, "workspace_id": state.workspace_id, "trigger_event": event, "close_intent_required": close_intent, "target_status_category": category, "enabled": true}))).await?;
        }
    }
    batch.commit_and_deliver(tx, ws_manager).await
}

/// Operator identity reconciliation is separate from user authorization: it
/// annotates only server-owned historical rows and never reconnects them.
/// A supplied descriptor must be an authenticated GitHub installation response
/// from a trusted historical backup when the original installation was deleted.
pub async fn reconcile_legacy_identity(
    db: &DbPool,
    installation_id: i64,
    details: &GitHubInstallationDetails,
    apply: bool,
) -> Result<()> {
    let existing = crate::schema::get_installation_by_github_id(db, installation_id)
        .await?
        .ok_or_else(|| Error::NotFound("No historical installation with that ID".into()))?;
    let app = crate::schema::get_github_app(db)
        .await?
        .ok_or_else(|| Error::NotFound("No configured GitHub App".into()))?;
    if details.id != installation_id as u64
        || installation_id <= 0
        || details.app_id != app.app_id as u64
        || existing.github_app_id != app.github_app_id
        || details.account.id == 0
        || i64::try_from(details.account.id).is_err()
        || !matches!(
            details.account.account_type.as_str(),
            "User" | "Organization"
        )
        || details.target_type != details.account.account_type
    {
        return Err(Error::Forbidden(
            "Historical evidence does not match the recorded installation and configured App"
                .into(),
        ));
    }
    let verified = VerifiedConnection {
        app_id: app.app_id,
        account_id: details.account.id as i64,
        account_type: details.account.account_type.clone(),
        account_login: details.account.login.clone(),
        installation_id,
        repository_selection: details.repository_selection.clone(),
        repos: None,
    };
    let mut tx = db.begin().await?;
    let recorded: Option<i64> = trakkt_core::tx_fetch_scalar!(
        &mut tx,
        Option<i64>,
        "SELECT github_account_id FROM github_installations WHERE installation_id = $1",
        &existing.installation_id
    )?;
    if recorded.is_some_and(|id| id != verified.account_id) {
        return Err(conflict());
    }
    claim_verified_ownership(&mut tx, &existing.workspace_id, &verified).await?;
    trakkt_core::tx_execute!(
        &mut tx,
        "UPDATE github_installations SET github_account_id = $1 WHERE installation_id = $2 AND (github_account_id IS NULL OR github_account_id = $1)",
        verified.account_id,
        &existing.installation_id
    )?;
    let mut batch = SyncBatch::new();
    batch.record(&mut tx, "GITHUB_INSTALLATION", &existing.installation_id, &existing.workspace_id, SyncAudience::Workspace, SyncActionType::Update, Some(serde_json::json!({"installation_id": existing.installation_id, "github_account_id": verified.account_id}))).await?;
    if !apply {
        tx.rollback().await?;
        return Ok(());
    }
    batch.commit_and_deliver(tx, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Query, State},
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use std::{
        collections::HashMap,
        sync::{Arc, LazyLock},
    };
    use trakkt_core::test_helpers::{seed_user, seed_workspace};

    async fn start_for_test(
        db: &DbPool,
        client: &GitHubClient,
        key: &[u8; 32],
        user: &str,
        workspace: &str,
        reinstall: bool,
    ) -> Result<String> {
        let existing = crate::schema::get_installation_for_workspace(db, workspace).await?;
        start_connection(
            db,
            client,
            key,
            user,
            workspace,
            existing.as_ref().map(|row| row.installation_id.as_str()),
            reinstall,
        )
        .await
    }

    static PEM: LazyLock<Vec<u8>> = LazyLock::new(|| {
        rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048)
            .expect("generate controlled GitHub App test key")
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode controlled GitHub App test key")
            .as_bytes()
            .to_vec()
    });
    const KEY: [u8; 32] = [7; 32];

    #[derive(Clone)]
    struct Scenario {
        installation_id: i64,
        account_id: i64,
        account_type: &'static str,
        account_login: &'static str,
        wrong_app: bool,
        inaccessible: bool,
        revoked: bool,
        fail_page: bool,
        repository_selection: &'static str,
        empty_repositories: bool,
    }
    impl Default for Scenario {
        fn default() -> Self {
            Self {
                installation_id: 123,
                account_id: 55,
                account_type: "Organization",
                account_login: "example",
                wrong_app: false,
                inaccessible: false,
                revoked: false,
                fail_page: false,
                repository_selection: "selected",
                empty_repositories: false,
            }
        }
    }
    fn details(s: &Scenario) -> serde_json::Value {
        serde_json::json!({"id": s.installation_id, "app_id": if s.wrong_app { 99 } else { 42 }, "account": {"id":s.account_id,"login":s.account_login,"type":s.account_type},"target_type":s.account_type,"permissions":{},"events":[],"repository_selection":s.repository_selection,"suspended_at":null})
    }
    async fn user() -> Json<serde_json::Value> {
        Json(serde_json::json!({"id":55,"type":"User"}))
    }
    async fn token(
        axum::extract::Form(fields): axum::extract::Form<HashMap<String, String>>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        if fields.get("code").map(String::as_str) != Some("valid")
            || fields.get("code_verifier").is_none_or(|v| v.len() != 43)
            || fields.get("client_secret").map(String::as_str) != Some("test-secret")
        {
            return (
                StatusCode::OK,
                Json(serde_json::json!({"error":"bad_verification_code"})),
            );
        }
        (
            StatusCode::OK,
            Json(serde_json::json!({"access_token":"transient-user-token","token_type":"bearer"})),
        )
    }
    async fn installations(
        State(s): State<Arc<Scenario>>,
        Query(q): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<serde_json::Value>) {
        assert_eq!(
            headers
                .get("authorization")
                .expect("user installations authorization header"),
            "Bearer transient-user-token"
        );
        let page = q.get("page").map(String::as_str).unwrap_or("1");
        if s.fail_page && page == "2" {
            return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({})));
        }
        if page == "1" {
            let unrelated: Vec<_> = (1000..1100)
                .map(|id| {
                    let mut d = details(&s);
                    d["id"] = id.into();
                    d
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({"total_count":101,"installations":unrelated})),
            )
        } else {
            (
                StatusCode::OK,
                Json(
                    serde_json::json!({"total_count":101,"installations": if s.inaccessible {vec![{let mut d=details(&s);d["id"]=999.into();d}]} else {vec![details(&s)]}}),
                ),
            )
        }
    }
    async fn app_details(State(s): State<Arc<Scenario>>) -> Json<serde_json::Value> {
        Json(details(&s))
    }
    async fn repositories(
        State(s): State<Arc<Scenario>>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let auth = headers
            .get("authorization")
            .expect("repository authorization header")
            .to_str()
            .expect("ASCII authorization");
        if s.revoked && auth == "Bearer transient-user-token" {
            return (StatusCode::FORBIDDEN, Json(serde_json::json!({})));
        }
        (
            StatusCode::OK,
            Json(if s.empty_repositories {
                serde_json::json!({"total_count":0,"repositories":[]})
            } else {
                serde_json::json!({"total_count":1,"repositories":[{"full_name":"example/repo","name":"repo","private":true}]})
            }),
        )
    }
    async fn installation_token() -> Json<serde_json::Value> {
        Json(
            serde_json::json!({"token":"transient-installation-token","expires_at":"2099-01-01T00:00:00Z"}),
        )
    }
    struct ControlledGitHub {
        client: GitHubClient,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for ControlledGitHub {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn controlled(scenario: Scenario) -> ControlledGitHub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind controlled GitHub endpoint");
        let base = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("read controlled GitHub address")
        );
        let router = Router::new()
            .route("/login/oauth/access_token", post(token))
            .route("/user", get(user))
            .route("/user/installations", get(installations))
            .route("/app/installations/456", get(app_details))
            .route("/user/installations/123/repositories", get(repositories))
            .route("/user/installations/456/repositories", get(repositories))
            .route("/installation/repositories", get(repositories))
            .route(
                "/app/installations/123/access_tokens",
                post(installation_token),
            )
            .route(
                "/app/installations/456/access_tokens",
                post(installation_token),
            );
        let router = if scenario.installation_id == 123 {
            router.route("/app/installations/123", get(app_details))
        } else {
            router
        }
        .with_state(Arc::new(scenario));
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve controlled GitHub responses");
        });
        let mut client =
            GitHubClient::new(42, &PEM, "test-app").expect("construct controlled GitHub client");
        client.oauth = Some(OAuthConfig {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            callback_url: "https://test.example/integrations/github/oauth/callback".into(),
        });
        client.api_base = base.clone();
        client.oauth_base = base;
        ControlledGitHub { client, task }
    }
    async fn add_account(db: &DbPool, client: &GitHubClient, installation: i64) {
        let url = start_connection(db, client, &KEY, "admin-a", "workspace-a", None, false)
            .await
            .expect("start Add account even with existing cards");
        let state = reqwest::Url::parse(&url)
            .expect("parse Add setup")
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("Add setup state")
            .1
            .into_owned();
        let url = advance_setup(db, client, &KEY, "admin-a", &state, installation, "install")
            .await
            .expect("advance selected Add account");
        let state = reqwest::Url::parse(&url)
            .expect("parse Add OAuth")
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("Add OAuth state")
            .1
            .into_owned();
        complete_connection(db, client, &KEY, "admin-a", &state, "valid")
            .await
            .expect("bind verified additional account");
    }
    async fn reconnect_account(
        db: &DbPool,
        client: &GitHubClient,
        row: &crate::schema::GitHubInstallation,
    ) {
        let url = start_connection(
            db,
            client,
            &KEY,
            "admin-a",
            "workspace-a",
            Some(&row.installation_id),
            false,
        )
        .await
        .expect("start reconnect for explicit owned connection");
        let state = reqwest::Url::parse(&url)
            .expect("parse explicit reconnect")
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("reconnect state")
            .1
            .into_owned();
        complete_connection(db, client, &KEY, "admin-a", &state, "valid")
            .await
            .expect("reconnect exact installation identity");
    }

    async fn seed(db: &DbPool) {
        seed_user(db, "admin-a", "a@example.test")
            .await
            .expect("seed first GitHub admin");
        seed_workspace(db, "workspace-a", "admin-a")
            .await
            .expect("seed first GitHub workspace");
        seed_user(db, "admin-b", "b@example.test")
            .await
            .expect("seed second GitHub admin");
        seed_workspace(db, "workspace-b", "admin-b")
            .await
            .expect("seed second GitHub workspace");
        crate::schema::create_github_app(
            db,
            42,
            "test-app",
            "test-client",
            "encrypted",
            "encrypted",
            "encrypted",
        )
        .await
        .expect("seed configured GitHub App");
    }
    async fn begin(db: &DbPool, client: &GitHubClient, user: &str, workspace: &str) -> String {
        let url = start_for_test(db, client, &KEY, user, workspace, false)
            .await
            .expect("start authorized connection");
        let state = reqwest::Url::parse(&url)
            .expect("parse setup URL")
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("setup state parameter")
            .1
            .into_owned();
        let url = advance_setup(db, client, &KEY, user, &state, 123, "install")
            .await
            .expect("advance setup to OAuth");
        let parsed = reqwest::Url::parse(&url).expect("parse OAuth URL");
        assert!(
            parsed
                .query_pairs()
                .any(|(k, v)| k == "code_challenge_method" && v == "S256")
        );
        assert!(
            advance_setup(db, client, &KEY, user, &state, 123, "install")
                .await
                .is_err(),
            "setup state must be single-use"
        );
        parsed
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("OAuth state parameter")
            .1
            .into_owned()
    }
    type BindingSnapshot = (i64, i64, i64, i64, i64, serde_json::Value);
    type InstallationSnapshot = (
        String,
        String,
        i64,
        Option<i64>,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    async fn snapshot(db: &DbPool) -> BindingSnapshot {
        let installations = trakkt_core::db_fetch_all!(db, InstallationSnapshot, "SELECT installation_id, workspace_id, github_installation_id, github_account_id, account_login, account_type, CAST(target_repos AS TEXT), access_token_encrypted, CAST(token_expires_at AS TEXT), CAST(suspended_at AS TEXT) FROM github_installations ORDER BY installation_id").expect("snapshot ordered connection metadata and cached tokens");
        let accounts = trakkt_core::db_fetch_all!(db, (i64, i64, String, String), "SELECT app_id, account_id, account_type, workspace_id FROM github_account_claims ORDER BY app_id, account_id").expect("snapshot ordered stable account owners");
        let owners = trakkt_core::db_fetch_all!(db, (i64, i64, i64, String), "SELECT installation_id, app_id, account_id, workspace_id FROM github_installation_claims ORDER BY installation_id").expect("snapshot ordered historical installation owners");
        let links = trakkt_core::db_fetch_all!(
            db,
            (String, String, String),
            "SELECT link_id, installation_id, workspace_id FROM github_links ORDER BY link_id"
        )
        .expect("snapshot ordered historical links");
        let rules = trakkt_core::db_fetch_all!(db, (String, String, String, bool, String, bool), "SELECT rule_id, workspace_id, trigger_event, close_intent_required, target_status_category, enabled FROM github_transition_rules ORDER BY rule_id").expect("snapshot ordered transition rules");
        let sync_count = trakkt_core::db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM sync_log")
            .expect("snapshot sync log");
        (
            installations.len() as i64,
            accounts.len() as i64,
            owners.len() as i64,
            rules.len() as i64,
            sync_count,
            serde_json::json!({"installations":installations,"accounts":accounts,"owners":owners,"links":links,"rules":rules}),
        )
    }

    fn oauth_state(url: &str) -> String {
        let url = reqwest::Url::parse(url).expect("parse direct installation OAuth URL");
        assert!(url.query_pairs().any(|(key, value)| key == "code_challenge_method" && value == "S256"));
        url.query_pairs().find(|(key, _)| key == "state").expect("fresh direct authorization state").1.into_owned()
    }

    trakkt_core::dual_backend_test! {
        async fn github_direct_install_authorization_binds_only_confirmed_admin_workspace(db) {
            seed(db).await;
            let github = controlled(Scenario::default()).await;
            let before = snapshot(db).await;
            let states = trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM github_connection_states").expect("initial state count");
            assert!(start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-b",123).await.is_err());
            assert!(start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-a",0).await.is_err());
            assert_eq!(states,trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM github_connection_states").expect("unauthorized starts create no states"));
            let url = start_direct_connection(db,&github.client,&KEY,"admin-b","workspace-b",123).await.expect("confirm direct install for explicit workspace");
            let state = oauth_state(&url);
            assert_eq!(before,snapshot(db).await,"confirmation starts OAuth without connection mutation");
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await);
            let destination=complete_connection_with_delivery(db,&github.client,&KEY,"admin-b",&state,"valid",None).await.expect("verified direct installation");
            assert_eq!(destination,"workspace-b","completion returns state-bound destination");
            let row=crate::schema::get_installation_by_github_id(db,123).await.expect("read direct binding").expect("direct binding exists");
            assert_eq!(row.workspace_id,"workspace-b");
            let bound=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-b",&state,"valid").await.is_err());
            assert_eq!(bound,snapshot(db).await,"replay never mutates");
            let foreign=oauth_state(&start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-a",123).await.expect("untrusted foreign candidate may request OAuth"));
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&foreign,"valid").await.is_err());
            assert_eq!(bound,snapshot(db).await,"verified foreign ownership remains unchanged");
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_direct_install_reconnect_and_forged_candidate_fail_closed(db) {
            seed(db).await;
            let github = controlled(Scenario::default()).await;
            let invalid=oauth_state(&start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-a",999).await.expect("untrusted candidate starts verification only"));
            let before=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&invalid,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await);
            let good=oauth_state(&start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-a",123).await.expect("direct candidate"));
            complete_connection(db,&github.client,&KEY,"admin-a",&good,"valid").await.expect("initial verified direct binding");
            let row=crate::schema::get_installation_by_github_id(db,123).await.expect("read connection").expect("connection exists");
            crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await.expect("disconnect retained connection");
            let before=snapshot(db).await;
            let reconnect=oauth_state(&start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-a",123).await.expect("own retained candidate chooses generation-bound reconnect"));
            assert_eq!(before,snapshot(db).await);
            complete_connection(db,&github.client,&KEY,"admin-a",&reconnect,"valid").await.expect("verified direct reconnect");
            let after=crate::schema::get_installation_by_github_id(db,123).await.expect("read restored connection").expect("restored connection exists");
            assert_eq!(row.installation_id,after.installation_id);
            assert!(after.disconnected_at.is_none());
            let stale=oauth_state(&start_direct_connection(db,&github.client,&KEY,"admin-a","workspace-a",123).await.expect("start generation bound direct reconnect"));
            crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await.expect("newer disconnect");
            let disconnected=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&stale,"valid").await.is_err());
            assert_eq!(disconnected,snapshot(db).await,"old direct reconnect cannot undo newer disconnect");
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_authorization_persists_only_verified_original_workspace(db) {
            seed(db).await;
            let github=controlled(Scenario::default()).await;
            let state=begin(db,&github.client,"admin-a","workspace-a").await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-b",&state,"valid").await.is_err());
            complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.expect("persist fully verified paginated GitHub installation");
            let row=crate::schema::get_installation_for_workspace(db,"workspace-a").await.expect("load original workspace connection").expect("original workspace connected");
            assert_eq!(row.github_installation_id,123);
            assert_eq!(row.target_repos.as_deref(),Some("[\"example/repo\"]"));
            assert!(crate::schema::get_installation_for_workspace(db,"workspace-b").await.expect("load other active workspace").is_none());
            let before=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await);
            let wrong=controlled(Scenario {account_id:66,..Scenario::default()}).await;
            let url=start_for_test(db,&wrong.client,&KEY,"admin-a","workspace-a",false).await.expect("start reconnect bound to the current account");
            let wrong_state=reqwest::Url::parse(&url).expect("parse mismatched-account OAuth URL").query_pairs().find(|(k,_)|k=="state").expect("wrong-account state").1.into_owned();
            assert!(complete_connection(db,&wrong.client,&KEY,"admin-a",&wrong_state,"valid").await.is_err(),"a different authorized account cannot replace the workspace connection");
            assert_eq!(before,snapshot(db).await);
            let url=start_for_test(db,&github.client,&KEY,"admin-a","workspace-a",false).await.expect("reconnect directly without uninstall");
            assert!(url.contains("/login/oauth/authorize"));
            let state=reqwest::Url::parse(&url).expect("parse reconnect URL").query_pairs().find(|(k,_)|k=="state").expect("reconnect state").1.into_owned();
            complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.expect("idempotent verified reconnect");
            assert_eq!(crate::schema::get_installation_for_workspace(db,"workspace-a").await.expect("reload reconnect").expect("reconnected").installation_id,row.installation_id);
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_authorization_failures_leave_binding_unchanged(db) {
            seed(db).await;
            for scenario in [Scenario{inaccessible:true,..Scenario::default()},Scenario{wrong_app:true,..Scenario::default()},Scenario{revoked:true,..Scenario::default()},Scenario{fail_page:true,..Scenario::default()},Scenario{account_type:"User",account_id:88,..Scenario::default()}] {
                let github=controlled(scenario).await;
                let state=begin(db,&github.client,"admin-a","workspace-a").await;
                let before=snapshot(db).await;
                assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err());
                assert_eq!(before,snapshot(db).await,"failed GitHub user verification must not persist connection, claims, rules or sync");
            }
            let github=controlled(Scenario::default()).await;
            let state=begin(db,&github.client,"admin-a","workspace-a").await;
            let before=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"invalid-code").await.is_err());
            assert_eq!(before,snapshot(db).await);
            trakkt_core::db_execute!(db,"UPDATE github_connection_states SET expires_at = 0 WHERE state_hash = $1",hash(&state)).expect("expire authorization state");
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await);
            let state=begin(db,&github.client,"admin-a","workspace-a").await;
            trakkt_core::db_execute!(db,"UPDATE workspace_users SET role = 'workspace_user' WHERE user_id = 'admin-a'").expect("revoke workspace administrator during flow");
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await);
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_authorization_atomic_claim_race_and_sync_rollback(db) {
            seed(db).await;
            let github=controlled(Scenario::default()).await;
            let a=begin(db,&github.client,"admin-a","workspace-a").await;
            let b=begin(db,&github.client,"admin-b","workspace-b").await;
            trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts(db).await;
            let before=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&a,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await,"sync failure rolls back all ownership claims and binding");
            let phase=trakkt_core::db_fetch_scalar!(db,String,"SELECT phase FROM github_connection_states WHERE state_hash = $1",hash(&a)).expect("read rolled-back state");
            assert_eq!(phase,"oauth","sync failure rolls back state consumption");
            trakkt_core::test_helpers::dual_backend::clear_sync_log_rejection(db).await;
            trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts_of_type(db,"GITHUB_TRANSITION_RULE").await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&a,"valid").await.is_err());
            assert_eq!(before,snapshot(db).await,"later rule sync failure rolls back the connection and its earlier accepted sync entry");
            trakkt_core::test_helpers::dual_backend::clear_sync_log_rejection(db).await;
            let (left,right)=tokio::join!(complete_connection(db,&github.client,&KEY,"admin-a",&a,"valid"),complete_connection(db,&github.client,&KEY,"admin-b",&b,"valid"));
            assert_eq!(usize::from(left.is_ok())+usize::from(right.is_ok()),1,"one workspace wins the atomic ownership race");
            assert_eq!(snapshot(db).await.0,1);
            let (winner,user,state)=if left.is_ok(){("workspace-a","admin-a",a)}else{("workspace-b","admin-b",b)};
            assert!(complete_connection(db,&github.client,&KEY,user,&state,"valid").await.is_err());
            let row=crate::schema::get_installation_for_workspace(db,winner).await.expect("load winning connection").expect("winning connection exists");
            crate::schema::suspend_installation(db,&row.installation_id).await.expect("disconnect winner");
            let (loser,other)=if winner=="workspace-a"{("workspace-b","admin-b")}else{("workspace-a","admin-a")};
            let state=begin(db,&github.client,other,loser).await;
            let before=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,other,&state,"valid").await.is_err(),"disconnected ownership remains reserved");
            assert_eq!(before,snapshot(db).await);
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_authorization_same_state_race_and_personal_installation(db) {
            seed(db).await;
            let github=controlled(Scenario {account_type:"User",..Scenario::default()}).await;
            let state=begin(db,&github.client,"admin-a","workspace-a").await;
            let (a,b)=tokio::join!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid"),complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid"));
            assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1,"the same state may be consumed only once");
            assert_eq!(snapshot(db).await.0,1);
            let row=crate::schema::get_installation_for_workspace(db,"workspace-a").await.expect("load personal connection").expect("personal GitHub account connected");
            assert_eq!(row.account_type,"User");
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_authorization_legacy_backfill_preserves_metadata_and_reinstall(db) {
            seed(db).await;
            let app=crate::schema::get_github_app(db).await.expect("load test App").expect("App configured");
            let legacy=crate::schema::create_installation(db,"workspace-a",&app.github_app_id,123,"old-login","Organization",Some(&serde_json::json!(["old/repo"]))).await.expect("create legacy suspended installation");
            crate::schema::update_installation_token(db,&legacy.installation_id,"encrypted-cached-fixture","2099-01-01T00:00:00Z").await.expect("seed historical cached token metadata");
            trakkt_core::test_helpers::seed_team(db,"legacy-team","workspace-a","LEG").await.expect("seed linked issue team");
            trakkt_auth::status_service::seed_default_statuses(db,"workspace-a").await.expect("seed linked issue statuses");
            let issue=trakkt_auth::issue_service::create_issue(db,&trakkt_types::models::CreateIssueParams {workspace_id:"workspace-a".into(),team_id:"legacy-team".into(),creator_id:"admin-a".into(),title:"Historical link".into(),description:None,priority:0,assignee_id:None,due_date:None,label_ids:Vec::new(),project_id:None,milestone_id:None,estimate:None},None).await.expect("create issue with a historical GitHub link");
            let link=crate::schema::upsert_link(db,&crate::schema::CreateLinkParams {workspace_id:"workspace-a",issue_id:&issue.issue_id,installation_id:&legacy.installation_id,link_type:"pull_request",github_id:Some(500),github_node_id:None,repo_full_name:"old/repo",ref_identifier:"1",title:Some("Historical pull request"),state:Some("open"),url:"https://github.com/old/repo/pull/1",author_login:Some("old-login"),close_intent:false}).await.expect("seed historical GitHub link");
            crate::schema::suspend_installation(db,&legacy.installation_id).await.expect("suspend legacy installation");
            let metadata=crate::schema::get_installation_by_id(db,&legacy.installation_id).await.expect("read legacy metadata").expect("legacy exists");
            let response: GitHubInstallationDetails=serde_json::from_value(details(&Scenario::default())).expect("decode trusted identity evidence");
            reconcile_legacy_identity(db,123,&response,false).await.expect("dry-run legacy identity");
            assert_eq!(trakkt_core::db_fetch_scalar!(db,Option<i64>,"SELECT github_account_id FROM github_installations WHERE installation_id=$1",&legacy.installation_id).expect("read dry-run identity"),None);
            assert_eq!(snapshot(db).await.1,0,"dry-run rolls back claims");
            reconcile_legacy_identity(db,123,&response,true).await.expect("apply historical identity without reconnecting");
            let after=crate::schema::get_installation_by_id(db,&legacy.installation_id).await.expect("read annotated legacy metadata").expect("annotated legacy exists");
            assert_eq!(after.github_account_id,Some(55));
            assert_eq!(after.access_token_encrypted,metadata.access_token_encrypted);
            assert_eq!(after.suspended_at,metadata.suspended_at);
            assert_eq!(after.target_repos,metadata.target_repos);
            assert!(!after.is_active(),"identity annotation must not reconnect or trust old credentials");
            // A second unresolved historical workspace must be independently repairable.
            let other=crate::schema::create_installation(db,"workspace-b",&app.github_app_id,789,"other","Organization",None).await.expect("create second legacy connection");
            let mut other_response: GitHubInstallationDetails=serde_json::from_value(details(&Scenario::default())).expect("decode second trusted evidence");
            other_response.id=789;other_response.account.id=66;
            reconcile_legacy_identity(db,789,&other_response,true).await.expect("independent second legacy backfill");
            assert_eq!(trakkt_core::db_fetch_scalar!(db,Option<i64>,"SELECT github_account_id FROM github_installations WHERE installation_id=$1",&other.installation_id).expect("read second identity"),Some(66));
            let before=snapshot(db).await;
            other_response.account.id=55;
            assert!(reconcile_legacy_identity(db,789,&other_response,true).await.is_err(),"trusted evidence cannot overwrite existing identity/ownership");
            assert_eq!(snapshot(db).await,before);
            let github=controlled(Scenario {installation_id:456,..Scenario::default()}).await;
            assert!(github.client.get_installation_details(123).await.is_err(),"deleted old installation is unavailable to the App");
            let url=start_for_test(db,&github.client,&KEY,"admin-a","workspace-a",true).await.expect("start same-account reinstall");
            let state=reqwest::Url::parse(&url).expect("parse reinstall setup").query_pairs().find(|(k,_)|k=="state").expect("reinstall setup state").1.into_owned();
            let url=advance_setup(db,&github.client,&KEY,"admin-a",&state,456,"install").await.expect("advance reinstall");
            let state=reqwest::Url::parse(&url).expect("parse reinstall OAuth").query_pairs().find(|(k,_)|k=="state").expect("reinstall OAuth state").1.into_owned();
            complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.expect("bind verified same-account reinstall");
            let installed=crate::schema::get_installation_by_github_id(db,456).await.expect("read reinstall binding").expect("reinstall exists");
            assert_ne!(installed.installation_id,legacy.installation_id,"reinstall creates a separate immutable row");
            assert_eq!(crate::schema::get_installation_by_id(db,&legacy.installation_id).await.expect("read preserved historical installation").expect("historical row exists").github_installation_id,123);
            assert_eq!(installed.github_installation_id,456);
            let links=crate::schema::list_links_for_issue(db,&issue.issue_id).await.expect("read historical links after reinstall");
            assert_eq!(links.len(),1);
            assert_eq!(links[0].link_id,link.link_id);
            assert_eq!(links[0].installation_id,legacy.installation_id);
            assert_eq!(trakkt_core::db_fetch_scalar!(db,String,"SELECT workspace_id FROM github_installation_claims WHERE installation_id=123").expect("read historical installation owner"),"workspace-a");
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_authorization_known_reconnect_ignores_unrelated_unknown_identity(db) {
            seed(db).await;
            let app=crate::schema::get_github_app(db).await.expect("load configured App").expect("App exists");
            let a=crate::schema::create_installation(db,"workspace-a",&app.github_app_id,123,"example","Organization",None).await.expect("seed known workspace installation");
            crate::schema::create_installation(db,"workspace-b",&app.github_app_id,789,"unknown","Organization",None).await.expect("seed unrelated unknown historical installation");
            let response: GitHubInstallationDetails=serde_json::from_value(details(&Scenario::default())).expect("decode authenticated old installation evidence");
            reconcile_legacy_identity(db,123,&response,true).await.expect("annotate A while B remains unresolved");
            let github=controlled(Scenario::default()).await;
            for _ in 0..2 {
                let url=start_for_test(db,&github.client,&KEY,"admin-a","workspace-a",false).await.expect("start known exact-installation reconnect");
                let state=reqwest::Url::parse(&url).expect("parse known reconnect URL").query_pairs().find(|(k,_)|k=="state").expect("known reconnect state").1.into_owned();
                complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.expect("known reconnect must not depend on unrelated B backfill");
            }
            assert_eq!(crate::schema::get_installation_for_workspace(db,"workspace-a").await.expect("reload known installation").expect("A remains connected").installation_id,a.installation_id);
            let github=controlled(Scenario {installation_id:456,..Scenario::default()}).await;
            let url=start_for_test(db,&github.client,&KEY,"admin-a","workspace-a",true).await.expect("start reinstall requiring a new installation claim");
            let state=reqwest::Url::parse(&url).expect("parse blocked reinstall setup").query_pairs().find(|(k,_)|k=="state").expect("blocked reinstall setup state").1.into_owned();
            let url=advance_setup(db,&github.client,&KEY,"admin-a",&state,456,"install").await.expect("advance blocked reinstall");
            let state=reqwest::Url::parse(&url).expect("parse blocked reinstall OAuth").query_pairs().find(|(k,_)|k=="state").expect("blocked reinstall OAuth state").1.into_owned();
            let before=snapshot(db).await;
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err(),"new installation ownership must still wait for unknown historical identity");
            assert_eq!(snapshot(db).await,before);
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_multiple_accounts_keep_immutable_history_rules_and_ownership(db) {
            seed(db).await;
            let personal = controlled(Scenario { account_type:"User", ..Scenario::default() }).await;
            let org = controlled(Scenario { installation_id:456, account_id:66, ..Scenario::default() }).await;
            // Organization first; the sibling test exercises personal first.
            add_account(db,&org.client,456).await;
            let org_row=crate::schema::get_installation_by_github_id(db,456).await.expect("read organization installation").expect("organization connected");
            let rules=crate::schema::list_transition_rules(db,"workspace-a").await.expect("read workspace shared rules");
            crate::schema::update_transition_rule_enabled(db,&rules[0].rule_id,"workspace-a",false).await.expect("customize shared rule before adding personal account");
            let before=crate::schema::list_transition_rules(db,"workspace-a").await.expect("snapshot custom rules");
            add_account(db,&personal.client,123).await;
            let rows=crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("list multiple connections");
            assert_eq!(rows.len(),2);
            assert!(trakkt_core::db_execute!(db,"UPDATE github_installations SET github_installation_id = 999 WHERE installation_id = $1",&org_row.installation_id).is_err(),"database rejects immutable installation ID replacement");
            assert!(trakkt_core::db_execute!(db,"UPDATE github_installations SET workspace_id = 'workspace-b' WHERE installation_id = $1",&org_row.installation_id).is_err(),"database rejects silent ownership transfer");
            assert!(rows.iter().all(crate::schema::GitHubInstallation::is_active));
            let personal_row=crate::schema::get_installation_by_github_id(db,123).await.expect("read personal installation").expect("personal connected");
            reconnect_account(db,&personal.client,&personal_row).await;
            reconnect_account(db,&org.client,&org_row).await;
            let renamed_org=controlled(Scenario {installation_id:456,account_id:66,account_login:"renamed-organization",..Scenario::default()}).await;
            reconnect_account(db,&renamed_org.client,&org_row).await;
            let renamed=crate::schema::get_installation_by_id(db,&org_row.installation_id).await.expect("read stable renamed account").expect("renamed account retained");
            assert_eq!(renamed.account_login,"renamed-organization");assert_eq!(renamed.github_account_id,Some(66));assert_eq!(renamed.github_installation_id,456);
            assert_eq!(crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("list idempotent reconnects").len(),2);
            assert_eq!(serde_json::to_value(before).expect("encode custom rules"),serde_json::to_value(crate::schema::list_transition_rules(db,"workspace-a").await.expect("reload unchanged shared rules")).expect("encode final shared rules"));
            assert_eq!(crate::schema::get_installation_by_github_id(db,456).await.expect("reload organization").expect("organization retained").installation_id,org_row.installation_id);
            assert!(start_connection(db,&org.client,&KEY,"admin-b","workspace-b",Some(&org_row.installation_id),false).await.is_err(),"foreign ID never chooses or reconnects another workspace's card");
            crate::schema::disconnect_installation(db,&personal_row.installation_id,"workspace-a").await.expect("disconnect only personal card");
            crate::schema::suspend_installation(db,&personal_row.installation_id).await.expect("GitHub suspends disconnected card");
            crate::schema::unsuspend_installation(db,&personal_row.installation_id).await.expect("GitHub unsuspends without local reconnect");
            let disconnected=crate::schema::get_installation_by_id(db,&personal_row.installation_id).await.expect("reload disconnected card").expect("disconnected card kept");
            assert!(disconnected.disconnected_at.is_some());assert!(!disconnected.is_active());
            assert!(crate::schema::get_installation_by_id(db,&org_row.installation_id).await.expect("reload other active card").expect("organization kept").is_active());
            let stale_url=start_connection(db,&personal.client,&KEY,"admin-a","workspace-a",Some(&disconnected.installation_id),false).await.expect("start authorization before a newer disconnect");
            let stale_state=reqwest::Url::parse(&stale_url).expect("parse stale reconnect state").query_pairs().find(|(k,_)|k=="state").expect("stale reconnect state").1.into_owned();
            crate::schema::disconnect_installation(db,&personal_row.installation_id,"workspace-a").await.expect("newer disconnect invalidates pending reconnect");
            assert!(complete_connection(db,&personal.client,&KEY,"admin-a",&stale_state,"valid").await.is_err(),"a late callback cannot undo a newer local disconnect");
            reconnect_account(db,&personal.client,&disconnected).await;
            assert!(crate::schema::get_installation_by_id(db,&personal_row.installation_id).await.expect("reload verified reconnect").expect("personal kept").is_active());
        }
    }
    trakkt_core::dual_backend_test! {
        async fn github_multiple_accounts_personal_first_and_same_workspace_callback_race(db) {
            seed(db).await;
            let personal=controlled(Scenario {account_type:"User",..Scenario::default()}).await;
            let org=controlled(Scenario {installation_id:456,account_id:66,..Scenario::default()}).await;
            add_account(db,&personal.client,123).await;
            let initial=crate::schema::get_installation_by_github_id(db,123).await.expect("read first personal account").expect("personal connected");
            let mut states=Vec::new();
            for _ in 0..2 {
                let url=start_connection(db,&org.client,&KEY,"admin-a","workspace-a",None,false).await.expect("start concurrent additional account callback");
                let state=reqwest::Url::parse(&url).expect("parse concurrent setup").query_pairs().find(|(k,_)|k=="state").expect("concurrent setup state").1.into_owned();
                let url=advance_setup(db,&org.client,&KEY,"admin-a",&state,456,"install").await.expect("advance concurrent account candidate");
                states.push(reqwest::Url::parse(&url).expect("parse concurrent OAuth").query_pairs().find(|(k,_)|k=="state").expect("concurrent OAuth state").1.into_owned());
            }
            let (a,b)=tokio::join!(complete_connection(db,&org.client,&KEY,"admin-a",&states[0],"valid"),complete_connection(db,&org.client,&KEY,"admin-a",&states[1],"valid"));
            a.expect("first valid callback");b.expect("second callback reconnects exact already-created installation");
            assert_eq!(crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("list atomic account callbacks").len(),2);
            assert_eq!(crate::schema::get_installation_by_github_id(db,123).await.expect("read preserved personal").expect("personal retained").installation_id,initial.installation_id);
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_authorization_add_existing_identity_cannot_undo_disconnect(db) {
            seed(db).await;
            let github = controlled(Scenario::default()).await;
            add_account(db, &github.client, 123).await;
            let row = crate::schema::get_installation_by_github_id(db,123).await.expect("load connection").expect("exists");
            let before = row.clone();
            add_account(db, &github.client, 123).await;
            assert_eq!(crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("reload").expect("exists"), before, "Add rediscovery is idempotent, including generation and credentials");
            let url = start_connection(db,&github.client,&KEY,"admin-a","workspace-a",None,false).await.expect("start unbound Add");
            let state = reqwest::Url::parse(&url).expect("setup URL").query_pairs().find(|(k,_)|k=="state").expect("state").1.into_owned();
            let url = advance_setup(db,&github.client,&KEY,"admin-a",&state,123,"install").await.expect("choose existing identity");
            let state = reqwest::Url::parse(&url).expect("OAuth URL").query_pairs().find(|(k,_)|k=="state").expect("state").1.into_owned();
            crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await.expect("disconnect after Add starts");
            let disconnected = crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("reload").expect("exists");
            assert!(complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid").await.is_err());
            assert_eq!(crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("reload").expect("exists"),disconnected);
            reconnect_account(db,&github.client,&disconnected).await;
            assert!(crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("reload").expect("exists").is_active());
            let newer = controlled(Scenario { installation_id:456, ..Scenario::default() }).await;
            add_account(db,&newer.client,456).await;
            let target = crate::schema::get_installation_by_github_id(db,456).await.expect("target identity").expect("exists");
            crate::schema::disconnect_installation(db,&target.installation_id,"workspace-a").await.expect("disconnect historical target");
            let target = crate::schema::get_installation_by_github_id(db,456).await.expect("target snapshot").expect("exists");
            let url = start_connection(db,&newer.client,&KEY,"admin-a","workspace-a",Some(&row.installation_id),true).await.expect("start reinstall selected predecessor");
            let state = reqwest::Url::parse(&url).expect("setup URL").query_pairs().find(|(k,_)|k=="state").expect("state").1.into_owned();
            let url = advance_setup(db,&newer.client,&KEY,"admin-a",&state,456,"install").await.expect("select different existing historical identity");
            let state = reqwest::Url::parse(&url).expect("OAuth URL").query_pairs().find(|(k,_)|k=="state").expect("state").1.into_owned();
            assert!(complete_connection(db,&newer.client,&KEY,"admin-a",&state,"valid").await.is_err(),"a predecessor's generation cannot authorize reactivation of another existing row");
            assert_eq!(crate::schema::get_installation_by_github_id(db,456).await.expect("target after denied reconnect").expect("exists"),target);
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_mutation_admission_rejects_paused_pr_push_and_status(db) {
            seed(db).await;
            let github = controlled(Scenario::default()).await;
            add_account(db,&github.client,123).await;
            trakkt_core::test_helpers::seed_team(db,"race-team","workspace-a","RAC").await.expect("seed race team");
            trakkt_auth::status_service::seed_default_statuses(db,"workspace-a").await.expect("seed race statuses");
            let issue = trakkt_auth::issue_service::create_issue(db,&trakkt_types::models::CreateIssueParams {workspace_id:"workspace-a".into(),team_id:"race-team".into(),creator_id:"admin-a".into(),title:"Paused GitHub mutation".into(),description:None,priority:0,assignee_id:None,due_date:None,label_ids:Vec::new(),project_id:None,milestone_id:None,estimate:None},None).await.expect("create race issue");
            let reference = format!("RAC-{}", issue.number);
            let pr = serde_json::json!({"repository":{"full_name":"example/repo"},"pull_request":{"number":700,"title":reference,"body":"","head":{"ref":reference},"base":{"ref":"main"},"html_url":"https://github.com/example/repo/pull/700","draft":false,"merged":false}});
            let push = serde_json::json!({"repository":{"full_name":"example/repo"},"ref":format!("refs/heads/{reference}"),"created":true,"commits":[{"id":"abcdef0123456789","message":reference,"url":"https://github.com/example/repo/commit/abcdef0123456789"}]});
            for event in 0..3 {
                for narrow in [false,true] {
                    let row = crate::schema::get_installation_by_github_id(db,123).await.expect("load connection").expect("exists");
                    reconnect_account(db,&github.client,&row).await;
                    let row = crate::schema::get_installation_by_github_id(db,123).await.expect("load fresh generation").expect("exists");
                    if event == 2 {
                        crate::events::process_pull_request(db,&row,"edited",&pr,None).await.expect("seed status PR link");
                    }
                    let links_before = trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM github_links").expect("link count");
                    let activities_before = trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM issue_activities").expect("activity count");
                    let (reached,resume) = crate::schema::mutation_pause::register(&row.installation_id);
                    let process = async {
                        match event {
                            0 => crate::events::process_pull_request(db,&row,"opened",&pr,None).await,
                            1 => crate::events::process_push(db,&row,&push,None).await,
                            _ => crate::transitions::apply_transition_rules(db,&github.client,&row,"workspace-a","opened",&pr,&KEY,None,"http://localhost").await,
                        }
                    };
                    let change = async {
                        reached.notified().await;
                        if narrow {
                            crate::schema::update_installation_repos(db,&row.installation_id,Some(&serde_json::json!([]))).await.expect("narrow access while event awaits pre-write work");
                        } else {
                            crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await.expect("disconnect while event awaits pre-write work");
                        }
                        resume.notify_one();
                    };
                    let (result,()) = tokio::time::timeout(std::time::Duration::from_secs(20),async {tokio::join!(process,change)}).await.expect("paused event completes without deadlock");
                    if event < 2 { assert!(result.is_err(),"stale link mutation must be denied"); } else { result.expect("status handler skips denied admission"); }
                    assert_eq!(trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM github_links").expect("final link count"),links_before);
                    assert_eq!(trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM issue_activities").expect("final activity count"),activities_before);
                    assert_eq!(trakkt_auth::issue_service::get_issue_by_id(db,&issue.issue_id).await.expect("reload issue").expect("exists").status_id,issue.status_id,"stale event cannot transition status");
                }
            }
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_outbound_admission_serializes_lifecycle_and_scope(db) {
            seed(db).await;
            let github = controlled(Scenario::default()).await;
            add_account(db,&github.client,123).await;
            for narrow in [false,true] {
                let row = crate::schema::get_installation_by_github_id(db,123).await.expect("load row").expect("exists");
                reconnect_account(db,&github.client,&row).await;
                let row = crate::schema::get_installation_by_github_id(db,123).await.expect("fresh row").expect("exists");
                let admission = crate::schema::admit_outbound(db,&row,"example/repo").await.expect("admit outbound before lifecycle mutation");
                let empty = serde_json::json!([]);
                let change = async {
                    if narrow {
                        crate::schema::update_installation_repos(db,&row.installation_id,Some(&empty)).await
                    } else {
                        crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await
                    }
                };
                let mut change = Box::pin(change);
                // Poll the actual lifecycle operation while admission is held.
                // It must remain pending until the admitted effect releases
                // its transaction, on both backends.
                assert!(tokio::time::timeout(std::time::Duration::from_millis(100),&mut change).await.is_err(),"lifecycle/scope cannot commit through an admitted outbound request");
                admission.commit().await.expect("finish admitted outbound effect");
                tokio::time::timeout(std::time::Duration::from_secs(10),change).await.expect("waiting mutation completes after admission release").expect("lifecycle mutation commits");
                assert!(crate::schema::admit_outbound(db,&row,"example/repo").await.is_err(),"old generation cannot admit another effect after lifecycle commit");
                let current = crate::schema::get_installation_by_github_id(db,123).await.expect("reload current row").expect("exists");
                assert!(!current.allows_repository("example/repo"));
                assert!(crate::schema::admit_outbound(db,&current,"example/repo").await.is_err(),"current denied scope cannot admit an effect either");
            }
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_authorization_workspace_lock_allows_admitted_link_foreign_key(db) {
            seed(db).await;
            let github = controlled(Scenario::default()).await;
            add_account(db,&github.client,123).await;
            trakkt_core::test_helpers::seed_team(db,"lock-team","workspace-a","LCK").await.expect("seed lock team");
            trakkt_auth::status_service::seed_default_statuses(db,"workspace-a").await.expect("seed lock statuses");
            let issue = trakkt_auth::issue_service::create_issue(db,&trakkt_types::models::CreateIssueParams {workspace_id:"workspace-a".into(),team_id:"lock-team".into(),creator_id:"admin-a".into(),title:"Admitted link during callback".into(),description:None,priority:0,assignee_id:None,due_date:None,label_ids:Vec::new(),project_id:None,milestone_id:None,estimate:None},None).await.expect("create lock issue");
            let row = crate::schema::get_installation_by_github_id(db,123).await.expect("load lock row").expect("exists");
            let url = start_connection(db,&github.client,&KEY,"admin-a","workspace-a",Some(&row.installation_id),false).await.expect("start generation-bound reconnect");
            let state = reqwest::Url::parse(&url).expect("OAuth URL").query_pairs().find(|(k,_)|k=="state").expect("state").1.into_owned();
            let mut admitted = crate::schema::admit_outbound(db,&row,"example/repo").await.expect("admitted mutation holds installation lock");
            let pause = if db.is_postgres() {Some(crate::schema::mutation_pause::register(&format!("workspace-lock-{}",hash(&state))))} else {None};
            let callback = complete_connection(db,&github.client,&KEY,"admin-a",&state,"valid");
            let write = async {
                if let Some((reached,_)) = &pause { reached.notified().await; }
                // PostgreSQL callback is now paused with its workspace lock
                // held. This FK insert must acquire KEY SHARE compatibly,
                // before releasing the installation lock it will need next.
                let link_id = uuid::Uuid::new_v4().to_string();
                trakkt_core::tx_execute!(&mut admitted,
                    "INSERT INTO github_links (link_id,workspace_id,issue_id,installation_id,link_type,repo_full_name,ref_identifier,url) VALUES ($1,'workspace-a',$2,$3,'branch','example/repo','lock-order','https://github.com/example/repo/tree/lock-order')",
                    &link_id,&issue.issue_id,&row.installation_id).expect("admitted FK write is compatible with callback workspace lock");
                admitted.commit().await.expect("commit admitted link before callback installation lock");
                if let Some((_,resume)) = &pause { resume.notify_one(); }
            };
            let (callback,()) = tokio::time::timeout(std::time::Duration::from_secs(20),async {tokio::join!(callback,write)}).await.expect("callback and admitted link finish without inverse-lock deadlock");
            callback.expect("verified reconnect follows admitted mutation");
            assert_eq!(crate::schema::list_links_for_issue(db,&issue.issue_id).await.expect("read committed link").len(),1);
            assert!(crate::schema::get_installation_by_github_id(db,123).await.expect("read verified row").expect("exists").is_active());
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_multiple_accounts_selected_empty_token_generation_and_sync_rollback(db) {
            seed(db).await;
            let empty=controlled(Scenario { empty_repositories:true, ..Scenario::default() }).await;
            add_account(db,&empty.client,123).await;
            let row=crate::schema::get_installation_by_github_id(db,123).await.expect("load empty selected installation").expect("connected selected empty");
            assert_eq!(row.repository_selection,"selected");assert_eq!(row.target_repos.as_deref(),Some("[]"));assert!(!row.allows_repository("example/repo"));
            assert!(crate::schema::cache_installation_token(db,&row,"fixture-cache","2099-01-01T00:00:00Z").await.expect("cache token in unchanged generation"));
            let cached=crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("read token snapshot").expect("row exists");
            for mutation in 0..6 {
                let before=crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("snapshot ordered installation rows");
                let sync_before=trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM sync_log").expect("snapshot sync rows");
                trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts_of_type(db,"GITHUB_INSTALLATION").await;
                let result=match mutation {
                    0=>crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await,
                    1=>crate::schema::suspend_installation(db,&row.installation_id).await,
                    2=>crate::schema::unsuspend_installation(db,&row.installation_id).await,
                    3=>crate::schema::uninstall_installation(db,&row.installation_id).await,
                    4=>crate::schema::update_installation_repos(db,&row.installation_id,None).await,
                    _=>crate::schema::apply_repository_delta(db,&row.installation_id,"selected",&["example/repo".into()],&[]).await,
                };
                assert!(result.is_err(),"mutation {mutation} must fail when durable settings invalidation fails");
                assert_eq!(before,crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("reload ordered rolled back installation rows"));
                assert_eq!(sync_before,trakkt_core::db_fetch_scalar!(db,i64,"SELECT COUNT(*) FROM sync_log").expect("read rolled back sync rows"));
                trakkt_core::test_helpers::dual_backend::clear_sync_log_rejection(db).await;
            }
            crate::schema::update_installation_repos(db,&row.installation_id,None).await.expect("all repositories scope");
            let all=crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("read all scope").expect("all connection");
            assert_eq!(all.repository_selection,"all");assert!(all.target_repos.is_none());assert!(all.allows_repository("any/repo"));assert!(all.access_token_encrypted.is_none());
            assert!(!crate::schema::cache_installation_token(db,&cached,"late-old-token","2099-01-01T00:00:00Z").await.expect("reject obsolete in-flight cache"));
            crate::schema::update_installation_repos(db,&row.installation_id,Some(&serde_json::json!([]))).await.expect("return to selected empty");
            let first_added=vec!["first/repo".into()];let second_added=vec!["second/repo".into()];
            let (left,right)=tokio::join!(crate::schema::apply_repository_delta(db,&row.installation_id,"selected",&first_added,&[]),crate::schema::apply_repository_delta(db,&row.installation_id,"selected",&second_added,&[]));
            left.expect("first concurrent repository delta");right.expect("second concurrent repository delta");
            let merged=crate::schema::get_installation_by_id(db,&row.installation_id).await.expect("read serialized deltas").expect("delta connection");
            let merged_repos:Vec<String>=serde_json::from_str(merged.target_repos.as_deref().expect("selected deltas retain a JSON array")).expect("decode backend repository JSON");
            assert_eq!(merged_repos,vec!["first/repo".to_string(),"second/repo".to_string()]);
            let before_reconcile=crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("snapshot before failed scope quarantine");
            trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts_of_type(db,"GITHUB_INSTALLATION").await;
            assert!(crate::schema::begin_repository_reconciliation(db,&row.installation_id,None).await.is_err());
            assert_eq!(before_reconcile,crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("scope quarantine sync failure rolls back credentials and epoch"));
            trakkt_core::test_helpers::dual_backend::clear_sync_log_rejection(db).await;
            let generation=crate::schema::begin_repository_reconciliation(db,&row.installation_id,None).await.expect("quarantine while fetching current GitHub access");
            let pending=crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("snapshot pending reconciliation");
            assert!(pending[0].repository_scope_pending);assert!(!pending[0].is_active());
            trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts_of_type(db,"GITHUB_INSTALLATION").await;
            assert!(crate::schema::finish_repository_reconciliation(db,&row.installation_id,generation,None,None).await.is_err());
            assert_eq!(pending,crate::schema::list_installations_for_workspace(db,"workspace-a").await.expect("scope finish sync failure retains pending state"));
            trakkt_core::test_helpers::dual_backend::clear_sync_log_rejection(db).await;
            crate::schema::disconnect_installation(db,&row.installation_id,"workspace-a").await.expect("disconnect invalidates pending repository fetch");
            assert!(!crate::schema::finish_repository_reconciliation(db,&row.installation_id,generation,None,None).await.expect("stale scope fetch must not expand to all repositories"));
            let rules=crate::schema::list_transition_rules(db,"workspace-a").await.expect("read rollback rule");
            trakkt_core::test_helpers::dual_backend::reject_sync_log_inserts_of_type(db,"GITHUB_TRANSITION_RULE").await;
            assert!(crate::schema::update_transition_rule_enabled(db,&rules[0].rule_id,"workspace-a",false).await.is_err());
            assert!(crate::schema::list_transition_rules(db,"workspace-a").await.expect("read preserved rule")[0].enabled);
            trakkt_core::test_helpers::dual_backend::clear_sync_log_rejection(db).await;
        }
    }

    trakkt_core::dual_backend_test! {
        async fn github_multiple_connections_migration_preserves_legacy_credentials_and_rules(db) {
            seed(db).await;
            let app=crate::schema::get_github_app(db).await.expect("read legacy App").expect("legacy App exists");
            let all=crate::schema::create_installation(db,"workspace-a",&app.github_app_id,123,"legacy-personal","User",None).await.expect("seed old all-repositories connection");
            let empty=crate::schema::create_installation(db,"workspace-b",&app.github_app_id,456,"legacy-org","Organization",Some(&serde_json::json!([]))).await.expect("seed selected-empty legacy connection");
            crate::schema::update_installation_token(db,&all.installation_id,"preserved-encrypted-legacy-credential","2099-01-01T00:00:00Z").await.expect("seed credential that must be retained but quarantined");
            crate::schema::suspend_installation(db,&empty.installation_id).await.expect("seed ambiguous old local/GitHub suspension");
            crate::schema::seed_default_transition_rules(db,"workspace-a").await.expect("seed legacy shared rules");
            let rules=crate::schema::list_transition_rules(db,"workspace-a").await.expect("read old shared rule");
            crate::schema::update_transition_rule_enabled(db,&rules[0].rule_id,"workspace-a",false).await.expect("preserve customized rule toggle");
            let rules_before=serde_json::to_value(crate::schema::list_transition_rules(db,"workspace-a").await.expect("snapshot customized rules")).expect("encode customized rules");
            // Reconstruct the preceding schema, then run the exact paired migration
            // against existing rows instead of only testing empty-database startup.
            if db.is_postgres() {
                trakkt_core::db_execute!(db,"DROP TRIGGER github_installations_immutable_identity ON github_installations").expect("remove new Postgres trigger for migration fixture");
                trakkt_core::db_execute!(db,"DROP FUNCTION protect_github_installation_identity()").expect("remove new Postgres identity function");
            } else { trakkt_core::db_execute!(db,"DROP TRIGGER github_installations_immutable_identity").expect("remove new SQLite trigger for migration fixture"); }
            trakkt_core::db_execute!(db,"ALTER TABLE github_connection_states DROP COLUMN expected_token_generation").expect("restore preceding connection state schema");
            trakkt_core::db_execute!(db,"DROP INDEX github_installations_workspace_list").expect("remove new settings index");
            for column in ["disconnected_at","uninstalled_at","authorization_verified_at","repository_selection","token_generation","repository_scope_pending"] {
                trakkt_core::db_execute!(db,&format!("ALTER TABLE github_installations DROP COLUMN {column}")).expect("restore preceding installation schema");
            }
            trakkt_core::db_execute!(db,"CREATE UNIQUE INDEX github_installations_account_owner ON github_installations(github_app_id,github_account_id) WHERE github_account_id IS NOT NULL").expect("restore preceding single-account uniqueness");
            let migration=if db.is_postgres() {include_str!("../../../apps/server/migrations/20261010010000_github_multiple_connections.sql")}else{include_str!("../../../apps/server/migrations-sqlite/20261010010000_github_multiple_connections.sql")};
            trakkt_core::db_with_pool!(db,|pool| sqlx::raw_sql(migration).execute(pool).await.map(|_| ())).expect("apply exact paired migration to populated legacy schema");
            let migrated_all=crate::schema::get_installation_by_id(db,&all.installation_id).await.expect("read migrated all-repositories row").expect("old installation preserved");
            assert_eq!(migrated_all.github_installation_id,123);assert_eq!(migrated_all.repository_selection,"all");assert!(migrated_all.target_repos.is_none());
            assert_eq!(migrated_all.access_token_encrypted.as_deref(),Some("preserved-encrypted-legacy-credential"));assert!(!migrated_all.is_active());assert!(migrated_all.authorization_verified_at.is_none());
            let migrated_empty=crate::schema::get_installation_by_id(db,&empty.installation_id).await.expect("read migrated selected-empty row").expect("empty installation preserved");
            assert_eq!(migrated_empty.target_repos.as_deref(),Some("[]"));assert_eq!(migrated_empty.repository_selection,"selected");assert_eq!(migrated_empty.disconnected_at,migrated_empty.suspended_at);
            crate::schema::unsuspend_installation(db,&empty.installation_id).await.expect("unsuspend migrated legacy row");
            assert!(!crate::schema::get_installation_by_id(db,&empty.installation_id).await.expect("read quarantined unsuspend").expect("legacy retained").is_active());
            assert_eq!(rules_before,serde_json::to_value(crate::schema::list_transition_rules(db,"workspace-a").await.expect("read untouched migrated shared rules")).expect("encode migrated rules"));
        }
    }

    #[tokio::test]
    async fn missing_oauth_and_forged_setup_fail_closed() {
        let db = trakkt_core::test_helpers::test_pool()
            .await
            .expect("isolated GitHub authorization SQLite");
        seed(&db).await;
        let mut github = controlled(Scenario::default()).await;
        let state = begin(&db, &github.client, "admin-a", "workspace-a").await;
        let before = snapshot(&db).await;
        assert!(
            complete_connection(&db, &github.client, &KEY, "admin-a", "", "valid")
                .await
                .is_err()
        );
        assert!(
            complete_connection(
                &db,
                &github.client,
                &KEY,
                "admin-a",
                &random_secret().expect("generate forged state"),
                "valid"
            )
            .await
            .is_err()
        );
        let setup_url = start_for_test(&db, &github.client, &KEY, "admin-a", "workspace-a", false)
            .await
            .expect("start candidate-ID forgery test");
        let setup_state = reqwest::Url::parse(&setup_url)
            .expect("parse forgery setup URL")
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("forgery setup state")
            .1
            .into_owned();
        assert!(
            advance_setup(
                &db,
                &github.client,
                &KEY,
                "admin-a",
                &setup_state,
                999,
                "update"
            )
            .await
            .is_err()
        );
        let forged_url = advance_setup(
            &db,
            &github.client,
            &KEY,
            "admin-a",
            &setup_state,
            999,
            "install",
        )
        .await
        .expect("setup can select a candidate but never authorize it");
        let forged_state = reqwest::Url::parse(&forged_url)
            .expect("parse forged candidate authorization URL")
            .query_pairs()
            .find(|(k, _)| k == "state")
            .expect("forged candidate OAuth state")
            .1
            .into_owned();
        assert!(
            complete_connection(&db, &github.client, &KEY, "admin-a", &forged_state, "valid")
                .await
                .is_err(),
            "an App-accessible or supplied ID missing from user installations must not bind"
        );
        assert_eq!(before, snapshot(&db).await);
        github.client.oauth = None;
        assert!(
            complete_connection(&db, &github.client, &KEY, "admin-a", &state, "valid")
                .await
                .is_err()
        );
        assert!(
            start_for_test(&db, &github.client, &KEY, "admin-a", "workspace-a", false)
                .await
                .is_err()
        );
        assert_eq!(before, snapshot(&db).await);
    }
}
