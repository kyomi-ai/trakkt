// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub App API client for Trakkt.
//!
//! Handles JWT signing for App-level authentication, installation access token
//! acquisition, and GitHub API calls (comments, PR details, issue close).
//!
//! The client does NOT perform any database operations — callers handle
//! token caching and persistence.

pub mod authorization;
pub mod events;
pub mod patterns;
pub mod schema;
pub mod transitions;
pub mod webhook;

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use trakkt_core::Error;

// ─── GitHub API response types ──────────────────────────────────────────────

/// An installation access token returned by GitHub's API.
#[derive(Debug, Clone, Deserialize)]
pub struct InstallationToken {
    /// The token string used as a Bearer token for API calls.
    pub token: String,
    /// ISO 8601 expiration timestamp from GitHub.
    pub expires_at: String,
}

/// Installation details returned by GitHub's App API.
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubInstallationDetails {
    pub id: u64,
    pub account: GitHubAccount,
    pub app_id: u64,
    pub target_type: String,
    pub permissions: serde_json::Value,
    pub events: Vec<String>,
    pub repository_selection: String,
    pub suspended_at: Option<String>,
}

/// A GitHub account (organization or user) that installed the App.
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAccount {
    pub id: u64,
    pub login: String,
    #[serde(rename = "type")]
    pub account_type: String,
    pub avatar_url: Option<String>,
}

/// A GitHub repository accessible to an installation.
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRepository {
    pub full_name: String,
    pub name: String,
    pub private: bool,
}

/// Wrapper for the paginated list-repos endpoint response.
#[derive(Debug, Clone, Deserialize)]
struct ListReposResponse {
    pub total_count: usize,
    pub repositories: Vec<GitHubRepository>,
}

/// A GitHub pull request.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub merged: Option<bool>,
    pub html_url: String,
    pub head: PullRequestHead,
    pub user: Option<GitHubUser>,
}

/// The head (source) branch of a pull request.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestHead {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

/// A GitHub user (minimal representation).
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubUser {
    pub login: String,
}

// ─── JWT claims for GitHub App authentication ───────────────────────────────

/// JWT claims for GitHub App authentication.
///
/// Per GitHub's spec, the JWT must include:
/// - `iss`: the App ID (as a string)
/// - `iat`: issued-at minus 60s for clock drift
/// - `exp`: iat + 600s (10 minute maximum)
#[derive(Debug, Serialize, Deserialize)]
struct GitHubAppClaims {
    iss: String,
    iat: i64,
    exp: i64,
}

// ─── GitHubClient ───────────────────────────────────────────────────────────

const GITHUB_API_BASE: &str = "https://api.github.com";

/// HTTP client for the GitHub API, authenticated as a GitHub App.
pub struct GitHubClient {
    http: reqwest::Client,
    authorization_http: reqwest::Client,
    app_id: u64,
    private_key: Vec<u8>,
    app_name: String,
    oauth: Option<authorization::OAuthConfig>,
    api_base: String,
    oauth_base: String,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("app_id", &self.app_id)
            .field("app_name", &self.app_name)
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

impl GitHubClient {
    /// Create a new client from app credentials.
    ///
    /// Validates that the provided PEM bytes can be parsed as an RSA private key.
    pub fn new(app_id: u64, private_key_pem: &[u8], app_name: &str) -> trakkt_core::Result<Self> {
        // Validate the PEM key can be parsed — fail fast on invalid credentials.
        EncodingKey::from_rsa_pem(private_key_pem)
            .map_err(|e| Error::Internal(format!("invalid RSA private key PEM: {e}")))?;

        let http = reqwest::Client::builder()
            .user_agent(app_name)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Internal(format!("failed to build HTTP client: {e}")))?;

        let authorization_http = reqwest::Client::builder()
            .user_agent(app_name)
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Internal("failed to build GitHub authorization client".into()))?;

        Ok(Self {
            http,
            authorization_http,
            app_id,
            private_key: private_key_pem.to_vec(),
            app_name: app_name.to_string(),
            oauth: None,
            api_base: GITHUB_API_BASE.into(),
            oauth_base: "https://github.com".into(),
        })
    }

    /// Isolated HTTP fixture client. Never available in production-only builds.
    #[cfg(feature = "test-helpers")]
    pub fn for_test_endpoint(
        app_id: u64,
        key_pem: &[u8],
        endpoint: &str,
    ) -> trakkt_core::Result<Self> {
        let url = reqwest::Url::parse(endpoint)
            .map_err(|_| Error::BadRequest("Invalid fixture endpoint".into()))?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        {
            return Err(Error::Forbidden(
                "GitHub fixtures require a loopback HTTP endpoint".into(),
            ));
        }
        let mut client = Self::new(app_id, key_pem, "isolated-github-fixture")?;
        client.http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|error| Error::Internal(format!("Cannot create fixture client: {error}")))?;
        client.api_base = endpoint.trim_end_matches('/').into();
        client.oauth_base = endpoint.trim_end_matches('/').into();
        Ok(client)
    }

    /// Generate a JWT for App-level API access (10 min expiry).
    ///
    /// Used to authenticate as the App itself (not as an installation).
    /// The JWT is short-lived and should be generated fresh for each request.
    pub fn app_jwt(&self) -> trakkt_core::Result<String> {
        let now = chrono::Utc::now().timestamp();
        let iat = now - 60; // 60 seconds in the past for clock drift
        let exp = iat + 600; // 10 minute maximum per GitHub spec

        let claims = GitHubAppClaims {
            iss: self.app_id.to_string(),
            iat,
            exp,
        };

        let header = Header::new(Algorithm::RS256);
        let key = EncodingKey::from_rsa_pem(&self.private_key)
            .map_err(|e| Error::Internal(format!("RSA key encoding failed: {e}")))?;

        jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| Error::Internal(format!("JWT signing failed: {e}")))
    }

    /// Request a fresh installation access token from GitHub.
    ///
    /// Callers are responsible for caching/persisting the token.
    pub async fn request_installation_token(
        &self,
        installation_id: u64,
    ) -> trakkt_core::Result<InstallationToken> {
        let api_base = &self.api_base;
        let jwt = self.app_jwt()?;
        let url = format!("{api_base}/app/installations/{installation_id}/access_tokens");

        let response = self
            .http
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("Bearer {jwt}"))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("GitHub API request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_github_error(status.as_u16(), &url, response).await);
        }

        response.json::<InstallationToken>().await.map_err(|e| {
            Error::Internal(format!("failed to parse installation token response: {e}"))
        })
    }

    /// Post a comment on a GitHub issue or PR.
    pub async fn create_comment(
        &self,
        token: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> trakkt_core::Result<()> {
        let api_base = &self.api_base;
        let url = format!("{api_base}/repos/{repo}/issues/{number}/comments");

        let response = self
            .http
            .post(&url)
            .headers(api_headers(token))
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("GitHub API request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_github_error(status.as_u16(), &url, response).await);
        }

        Ok(())
    }

    /// Close a GitHub issue.
    pub async fn close_issue(
        &self,
        token: &str,
        repo: &str,
        number: u64,
    ) -> trakkt_core::Result<()> {
        let api_base = &self.api_base;
        let url = format!("{api_base}/repos/{repo}/issues/{number}");

        let response = self
            .http
            .patch(&url)
            .headers(api_headers(token))
            .json(&serde_json::json!({ "state": "closed" }))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("GitHub API request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_github_error(status.as_u16(), &url, response).await);
        }

        Ok(())
    }

    /// Get PR details.
    pub async fn get_pull_request(
        &self,
        token: &str,
        repo: &str,
        number: u64,
    ) -> trakkt_core::Result<PullRequest> {
        let api_base = &self.api_base;
        let url = format!("{api_base}/repos/{repo}/pulls/{number}");

        let response = self
            .http
            .get(&url)
            .headers(api_headers(token))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("GitHub API request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_github_error(status.as_u16(), &url, response).await);
        }

        response
            .json::<PullRequest>()
            .await
            .map_err(|e| Error::Internal(format!("failed to parse pull request response: {e}")))
    }

    /// Get installation details from GitHub (requires App JWT auth).
    pub async fn get_installation_details(
        &self,
        installation_id: u64,
    ) -> trakkt_core::Result<GitHubInstallationDetails> {
        let api_base = &self.api_base;
        let jwt = self.app_jwt()?;
        let url = format!("{api_base}/app/installations/{installation_id}");

        let response = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("Bearer {jwt}"))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("GitHub API request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_github_error(status.as_u16(), &url, response).await);
        }

        response
            .json::<GitHubInstallationDetails>()
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "failed to parse installation details response: {e}"
                ))
            })
    }

    /// List accessible repositories for an installation (requires installation token).
    pub async fn list_installation_repos(
        &self,
        token: &str,
    ) -> trakkt_core::Result<Vec<GitHubRepository>> {
        let mut repositories = Vec::new();
        for page in 1..=1000 {
            let url = format!(
                "{}/installation/repositories?per_page=100&page={page}",
                self.api_base
            );
            let response = self
                .http
                .get(&url)
                .headers(api_headers(token))
                .send()
                .await
                .map_err(|error| {
                    Error::Internal(format!("GitHub repository request failed: {error}"))
                })?;
            let status = response.status();
            if !status.is_success() {
                return Err(map_github_error(status.as_u16(), &url, response).await);
            }
            let parsed: ListReposResponse = response.json().await.map_err(|error| {
                Error::Internal(format!("Invalid GitHub repository response: {error}"))
            })?;
            let count = parsed.repositories.len();
            repositories.extend(parsed.repositories);
            if repositories.len() >= parsed.total_count {
                return Ok(repositories);
            }
            if count == 0 {
                return Err(Error::Internal(
                    "Incomplete GitHub repository pagination; reconnect to refresh access".into(),
                ));
            }
        }
        Err(Error::Internal(
            "GitHub repository pagination exceeded safety limit; reconnect to refresh access"
                .into(),
        ))
    }

    /// Accessor for the app name (used in User-Agent).
    pub fn app_name(&self) -> &str {
        &self.app_name
    }
}

// ─── Configuration helper ───────────────────────────────────────────────────

/// Load GitHub App configuration from environment variables.
///
/// Returns `None` if `GITHUB_APP_ID` is not set (GitHub integration disabled).
///
/// Environment variables:
/// - `GITHUB_APP_ID` (required for integration to be enabled)
/// - `GITHUB_APP_PRIVATE_KEY_PATH` (path to PEM file)
/// - `GITHUB_APP_NAME` (defaults to "trakkt")
pub fn from_env() -> Option<GitHubClient> {
    match client_from_env() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "Invalid GitHub App configuration, integration disabled");
            None
        }
    }
}

fn client_from_env() -> trakkt_core::Result<Option<GitHubClient>> {
    let app_id = match std::env::var("GITHUB_APP_ID") {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| Error::Internal("GITHUB_APP_ID must be a positive integer".into()))?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(_) => {
            return Err(Error::Internal(
                "GITHUB_APP_ID must be valid Unicode".into(),
            ));
        }
    };
    if app_id == 0 || i64::try_from(app_id).is_err() {
        return Err(Error::Internal(
            "GITHUB_APP_ID must be a positive signed 64-bit integer".into(),
        ));
    }
    let key_path = std::env::var("GITHUB_APP_PRIVATE_KEY_PATH")
        .map_err(|_| Error::Internal("GITHUB_APP_PRIVATE_KEY_PATH is required".into()))?;
    let private_key = std::fs::read(&key_path)
        .map_err(|e| Error::Internal(format!("Failed to read GitHub App private key: {e}")))?;
    let app_name = std::env::var("GITHUB_APP_NAME").unwrap_or_else(|_| "trakkt".into());
    let mut client = GitHubClient::new(app_id, &private_key, &app_name)?;
    client.oauth = authorization::OAuthConfig::from_env()?;
    Ok(Some(client))
}

/// Initialize the optional GitHub integration and persist its encrypted credentials.
///
/// An absent app ID disables integration. Once an ID is supplied, incomplete or
/// invalid credentials are errors so startup cannot silently disable automation.
pub async fn initialize_from_env(
    db: &trakkt_core::DbPool,
    encryption_key: &[u8; 32],
) -> trakkt_core::Result<Option<GitHubClient>> {
    let Some(client) = client_from_env()? else {
        return Ok(None);
    };
    let webhook_secret = std::env::var("GITHUB_WEBHOOK_SECRET")
        .map_err(|_| Error::Internal("GITHUB_WEBHOOK_SECRET is required".into()))?;
    ensure_configured(db, &client, &webhook_secret, encryption_key).await?;
    if let Some(oauth) = &client.oauth {
        let secret = trakkt_auth::encryption::encrypt(&oauth.client_secret, encryption_key)?;
        trakkt_core::db_execute!(
            db,
            "UPDATE github_apps SET client_id = $1, client_secret_encrypted = $2 WHERE app_id = $3",
            &oauth.client_id,
            &secret,
            client.app_id as i64
        )?;
    }
    Ok(Some(client))
}

async fn ensure_configured(
    db: &trakkt_core::DbPool,
    client: &GitHubClient,
    webhook_secret: &str,
    encryption_key: &[u8; 32],
) -> trakkt_core::Result<()> {
    let app_id = i64::try_from(client.app_id)
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            Error::Internal("GITHUB_APP_ID must be a positive signed 64-bit integer".into())
        })?;
    if webhook_secret.trim().is_empty() {
        return Err(Error::Internal(
            "GITHUB_WEBHOOK_SECRET must not be empty".into(),
        ));
    }
    if client.app_name.trim().is_empty() {
        return Err(Error::Internal("GITHUB_APP_NAME must not be empty".into()));
    }
    let existing = schema::get_github_app(db).await?;
    if let Some(ref app) = existing
        && app.app_id != app_id
    {
        return Err(Error::Internal(
            "GITHUB_APP_ID differs from the configured app; configure the existing app or explicitly migrate its configuration".into(),
        ));
    }
    let pem = std::str::from_utf8(&client.private_key)
        .map_err(|_| Error::Internal("GitHub App private key PEM must be UTF-8".into()))?;
    let private_key_encrypted = trakkt_auth::encryption::encrypt(pem, encryption_key)?;
    let webhook_secret_encrypted =
        trakkt_auth::encryption::encrypt(webhook_secret, encryption_key)?;
    match existing {
        Some(app) => {
            schema::update_github_app_credentials(
                db,
                &app.github_app_id,
                &client.app_name,
                &private_key_encrypted,
                &webhook_secret_encrypted,
            )
            .await?
        }
        None => {
            let empty_secret = trakkt_auth::encryption::encrypt("", encryption_key)?;
            schema::create_github_app(
                db,
                app_id,
                &client.app_name,
                "",
                &empty_secret,
                &private_key_encrypted,
                &webhook_secret_encrypted,
            )
            .await?;
        }
    }
    Ok(())
}

// ─── Internal helpers ───────────────────────────────────────────────────────

/// Build the standard headers for GitHub API calls using an installation token.
fn api_headers(token: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Accept",
        "application/vnd.github+json"
            .parse()
            .expect("valid header value"),
    );
    headers.insert(
        "X-GitHub-Api-Version",
        "2022-11-28".parse().expect("valid header value"),
    );
    headers.insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid header value"),
    );
    headers
}

/// Map a GitHub API error response to the appropriate `trakkt_core::Error` variant.
async fn map_github_error(status: u16, url: &str, _response: reqwest::Response) -> Error {
    // GitHub error bodies can include request data. Never put credential-bearing
    // responses in logs or user-visible errors.
    tracing::warn!(url = %url, status, "GitHub API request rejected");
    match status {
        404 => Error::NotFound("GitHub resource not found".into()),
        401 => Error::Unauthorized("GitHub authentication failed".into()),
        403 => Error::Forbidden("GitHub API access denied".into()),
        _ => Error::Internal(format!("GitHub API error (status {status})")),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation};
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use std::sync::LazyLock;

    struct TestKeyPair {
        private_pem: Vec<u8>,
        public_pem: Vec<u8>,
    }

    static TEST_KEYS: LazyLock<TestKeyPair> = LazyLock::new(|| {
        let mut rng = rand_core::OsRng;
        let private_key =
            RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate test RSA key");
        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("failed to encode private key")
            .as_bytes()
            .to_vec();
        let public_key = private_key.to_public_key();
        let public_pem =
            rsa::pkcs8::EncodePublicKey::to_public_key_pem(&public_key, LineEnding::LF)
                .expect("failed to encode public key")
                .as_bytes()
                .to_vec();
        TestKeyPair {
            private_pem,
            public_pem,
        }
    });

    #[tokio::test]
    async fn configuration_bootstrap_preserves_identity_and_rotates_credentials() {
        configuration_bootstrap_roundtrip("sqlite::memory:").await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_GITHUB_DATABASE_URL pointing to an isolated PostgreSQL database"]
    async fn configuration_bootstrap_preserves_identity_and_rotates_credentials_postgres() {
        let url = std::env::var("TEST_GITHUB_DATABASE_URL")
            .expect("TEST_GITHUB_DATABASE_URL must name an isolated PostgreSQL test database");
        configuration_bootstrap_roundtrip(&url).await;
    }

    async fn configuration_bootstrap_roundtrip(url: &str) {
        let db = trakkt_core::DbPool::connect(url)
            .await
            .expect("opening bootstrap test database");
        let key = [7; 32];
        let client = GitHubClient::new(12345, &TEST_KEYS.private_pem, "first-app")
            .expect("constructing bootstrap client");
        ensure_configured(&db, &client, "first-webhook-secret", &key)
            .await
            .expect("bootstrapping app configuration");
        let first = schema::get_github_app(&db)
            .await
            .expect("reading bootstrapped app")
            .expect("bootstrap must persist app");
        assert_eq!(
            trakkt_auth::encryption::decrypt(&first.private_key_encrypted, &key)
                .expect("decrypting stored PEM")
                .as_bytes(),
            TEST_KEYS.private_pem
        );
        assert_eq!(
            trakkt_auth::encryption::decrypt(&first.webhook_secret_encrypted, &key)
                .expect("decrypting stored webhook secret"),
            "first-webhook-secret"
        );
        assert_eq!(
            trakkt_auth::encryption::decrypt(&first.client_secret_encrypted, &key)
                .expect("decrypting OAuth placeholder"),
            ""
        );
        assert_ne!(first.webhook_secret_encrypted, "first-webhook-secret");
        let oauth_secret = trakkt_auth::encryption::encrypt("existing-oauth-secret", &key)
            .expect("encrypting existing OAuth credential");
        trakkt_core::db_execute!(
            &db,
            "UPDATE github_apps SET client_id = $1, client_secret_encrypted = $2",
            "existing-client",
            &oauth_secret
        )
        .expect("seeding existing OAuth settings");
        ensure_configured(&db, &client, "first-webhook-secret", &key)
            .await
            .expect("restarting app configuration");
        let restarted = schema::get_github_app(&db)
            .await
            .expect("reading restarted app")
            .expect("restarted app exists");
        assert_eq!(first.github_app_id, restarted.github_app_id);
        let mut rng = rand_core::OsRng;
        let rotated_pem = RsaPrivateKey::new(&mut rng, 2048)
            .expect("generating rotated RSA key")
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encoding rotated RSA key");
        let rotated = GitHubClient::new(12345, rotated_pem.as_bytes(), "renamed-app")
            .expect("constructing rotated client");
        ensure_configured(&db, &rotated, "rotated-webhook-secret", &key)
            .await
            .expect("rotating configuration");
        let current = schema::get_github_app(&db)
            .await
            .expect("reading rotated app")
            .expect("rotated app exists");
        assert_eq!(first.github_app_id, current.github_app_id);
        assert_eq!(current.app_name, "renamed-app");
        assert_eq!(current.client_id, "existing-client");
        assert_eq!(current.client_secret_encrypted, oauth_secret);
        assert_eq!(
            trakkt_auth::encryption::decrypt(&current.private_key_encrypted, &key)
                .expect("decrypting rotated PEM"),
            rotated_pem.as_str()
        );
        assert_eq!(
            trakkt_auth::encryption::decrypt(&current.webhook_secret_encrypted, &key)
                .expect("decrypting rotated webhook secret"),
            "rotated-webhook-secret"
        );
        let mismatched = GitHubClient::new(54321, &TEST_KEYS.private_pem, "other-app")
            .expect("constructing mismatched app client");
        ensure_configured(&db, &mismatched, "other-secret", &key)
            .await
            .expect_err("changing app identity must be rejected");
        let unchanged = schema::get_github_app(&db)
            .await
            .expect("reading unchanged app")
            .expect("original app still exists");
        assert_eq!(unchanged.app_id, 12345);
        assert_eq!(
            unchanged.webhook_secret_encrypted,
            current.webhook_secret_encrypted
        );
        ensure_configured(&db, &client, "  ", &key)
            .await
            .expect_err("empty webhook secret must be rejected");
        let empty_name = GitHubClient::new(12345, &TEST_KEYS.private_pem, "")
            .expect("constructing client with an empty app name");
        ensure_configured(&db, &empty_name, "valid-secret", &key)
            .await
            .expect_err("empty app name must be rejected");
        let invalid_id = GitHubClient::new(0, &TEST_KEYS.private_pem, "test-app")
            .expect("constructing client with a zero app ID");
        ensure_configured(&db, &invalid_id, "valid-secret", &key)
            .await
            .expect_err("zero app ID must be rejected");
    }

    #[tokio::test]
    async fn installation_selected_repositories_and_token_roundtrip() {
        installation_roundtrip("sqlite::memory:").await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_GITHUB_DATABASE_URL pointing to an isolated PostgreSQL database"]
    async fn installation_selected_repositories_and_token_roundtrip_postgres() {
        let url = std::env::var("TEST_GITHUB_DATABASE_URL")
            .expect("TEST_GITHUB_DATABASE_URL must name an isolated PostgreSQL test database");
        installation_roundtrip(&url).await;
    }

    async fn installation_roundtrip(url: &str) {
        let db = trakkt_core::DbPool::connect(url)
            .await
            .expect("opening migrated installation test database");
        let owner_id = uuid::Uuid::new_v4().to_string();
        let workspace_id = uuid::Uuid::new_v4().to_string();
        trakkt_core::db_execute!(
            &db,
            "INSERT INTO users (user_id, email) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            &owner_id,
            "github-installation-test@example.test"
        )
        .expect("creating installation workspace owner");
        trakkt_core::db_execute!(
            &db,
            "INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            &workspace_id,
            &owner_id
        ).expect("creating installation workspace");
        let key = [9; 32];
        let client = GitHubClient::new(12345, &TEST_KEYS.private_pem, "test-app")
            .expect("constructing installation test client");
        ensure_configured(&db, &client, "test-webhook-secret", &key)
            .await
            .expect("configuring installation test app");
        let app = schema::get_github_app(&db)
            .await
            .expect("reading installation test app")
            .expect("configured app exists");
        let selected_repos = serde_json::json!(["example/trakkt", "example/docs"]);
        let installation = schema::create_installation(
            &db,
            &workspace_id,
            &app.github_app_id,
            67890,
            "example",
            "Organization",
            Some(&selected_repos),
        )
        .await
        .expect("creating installation with selected repositories");
        let encrypted_token = trakkt_auth::encryption::encrypt("test-installation-token", &key)
            .expect("encrypting installation token");
        let expiry = "2030-01-02T03:04:05Z";
        schema::update_installation_token(
            &db,
            &installation.installation_id,
            &encrypted_token,
            expiry,
        )
        .await
        .expect("persisting installation token and expiry");
        let by_workspace = schema::get_installation_for_workspace(&db, &workspace_id)
            .await
            .expect("looking up selected-repository installation by workspace")
            .expect("workspace installation exists");
        let by_github_id = schema::get_installation_by_github_id(&db, 67890)
            .await
            .expect("looking up selected-repository installation by GitHub ID")
            .expect("GitHub installation exists");
        let by_local_id = schema::get_installation_by_id(&db, &installation.installation_id)
            .await
            .expect("looking up selected-repository installation by local ID")
            .expect("local installation exists");
        for retrieved in [installation, by_workspace, by_github_id, by_local_id] {
            let repos: serde_json::Value = serde_json::from_str(
                retrieved
                    .target_repos
                    .as_deref()
                    .expect("selected repositories are stored"),
            )
            .expect("decoding stored selected repositories");
            assert_eq!(repos, selected_repos);
            assert_eq!(retrieved.workspace_id, workspace_id);
            assert_eq!(retrieved.github_installation_id, 67890);
            assert_eq!(retrieved.github_app_id, app.github_app_id);
        }
        let cached = schema::get_installation_by_github_id(&db, 67890)
            .await
            .expect("reading cached installation credentials")
            .expect("installation exists");
        assert_eq!(
            trakkt_auth::encryption::decrypt(
                cached
                    .access_token_encrypted
                    .as_deref()
                    .expect("token is persisted"),
                &key,
            )
            .expect("decrypting cached installation token"),
            "test-installation-token"
        );
        let stored_expiry = crate::transitions::parse_token_expiry(
            cached
                .token_expires_at
                .as_deref()
                .expect("token expiry is persisted"),
        )
        .expect("parsing stored installation token expiry");
        assert_eq!(
            stored_expiry,
            chrono::DateTime::parse_from_rfc3339(expiry)
                .expect("parsing expected installation token expiry")
        );
    }

    #[test]
    fn jwt_generation_produces_valid_token() {
        let client = GitHubClient::new(12345, &TEST_KEYS.private_pem, "test-app")
            .expect("constructing a client from the generated RSA private key PEM");
        let jwt = client
            .app_jwt()
            .expect("signing a GitHub App JWT with the client's private key");

        // Decode and validate with the public key
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["12345"]);
        // Allow some clock skew for test stability
        validation.leeway = 120;

        let key = DecodingKey::from_rsa_pem(&TEST_KEYS.public_pem)
            .expect("building an RS256 decoding key from the matching public PEM");
        let token_data = jsonwebtoken::decode::<GitHubAppClaims>(&jwt, &key, &validation)
            .expect("the signed JWT to verify against the matching public key and issuer 12345");

        assert_eq!(token_data.claims.iss, "12345");
        // exp should be 600 seconds after iat
        assert_eq!(token_data.claims.exp - token_data.claims.iat, 600);
        // iat should be roughly now minus 60 seconds
        let now = chrono::Utc::now().timestamp();
        assert!((token_data.claims.iat - (now - 60)).abs() < 5);
    }

    #[test]
    fn from_env_returns_none_when_vars_not_set() {
        if std::env::var("GITHUB_APP_ID").is_ok() {
            // Skip this test if the env var happens to be set (e.g. dev machine)
            return;
        }
        assert!(from_env().is_none());
    }

    #[test]
    fn new_rejects_invalid_pem_data() {
        let result = GitHubClient::new(12345, b"not-a-valid-pem-key", "test-app");
        assert!(result.is_err());
        let err = result.expect_err("constructing a client from non-PEM bytes must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("invalid RSA private key PEM"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn new_accepts_valid_pem() {
        let result = GitHubClient::new(99999, &TEST_KEYS.private_pem, "my-app");
        assert!(result.is_ok());
        let client = result.expect("constructing a client from a well-formed RSA private key PEM");
        assert_eq!(client.app_name(), "my-app");
    }

    #[test]
    fn jwt_claims_have_correct_timing() {
        let client = GitHubClient::new(42, &TEST_KEYS.private_pem, "timing-test")
            .expect("constructing a client for app id 42 from the generated private key PEM");
        let jwt = client
            .app_jwt()
            .expect("signing a GitHub App JWT whose iat/exp claims this test inspects");

        // Decode with full signature validation using the public key
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = false;
        validation.set_issuer(&["42"]);
        // Allow generous leeway since we're testing timing, not expiry
        validation.leeway = 120;

        let key = DecodingKey::from_rsa_pem(&TEST_KEYS.public_pem)
            .expect("building an RS256 decoding key from the matching public PEM");
        let token_data = jsonwebtoken::decode::<GitHubAppClaims>(&jwt, &key, &validation)
            .expect("the signed JWT to verify against the matching public key and issuer 42");

        let now = chrono::Utc::now().timestamp();
        // iat should be now - 60 (within a few seconds tolerance)
        let expected_iat = now - 60;
        assert!(
            (token_data.claims.iat - expected_iat).abs() < 5,
            "iat {} is not close to expected {}",
            token_data.claims.iat,
            expected_iat
        );
        // exp should be iat + 600
        assert_eq!(token_data.claims.exp, token_data.claims.iat + 600);
    }
}
