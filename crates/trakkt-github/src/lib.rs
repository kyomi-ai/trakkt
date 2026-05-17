// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub App API client for Trakkt.
//!
//! Handles JWT signing for App-level authentication, installation access token
//! acquisition, and GitHub API calls (comments, PR details, issue close).
//!
//! The client does NOT perform any database operations — callers handle
//! token caching and persistence.

pub mod events;
pub mod patterns;
pub mod schema;
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
    app_id: u64,
    private_key: Vec<u8>,
    app_name: String,
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
            .build()
            .map_err(|e| Error::Internal(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            http,
            app_id,
            private_key: private_key_pem.to_vec(),
            app_name: app_name.to_string(),
        })
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
        let jwt = self.app_jwt()?;
        let url = format!(
            "{GITHUB_API_BASE}/app/installations/{installation_id}/access_tokens"
        );

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

        response
            .json::<InstallationToken>()
            .await
            .map_err(|e| Error::Internal(format!("failed to parse installation token response: {e}")))
    }

    /// Post a comment on a GitHub issue or PR.
    pub async fn create_comment(
        &self,
        token: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> trakkt_core::Result<()> {
        let url = format!("{GITHUB_API_BASE}/repos/{repo}/issues/{number}/comments");

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
        let url = format!("{GITHUB_API_BASE}/repos/{repo}/issues/{number}");

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
        let url = format!("{GITHUB_API_BASE}/repos/{repo}/pulls/{number}");

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
    let app_id_str = std::env::var("GITHUB_APP_ID").ok()?;
    let app_id: u64 = match app_id_str.parse() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(
                app_id = %app_id_str,
                error = %e,
                "GITHUB_APP_ID is not a valid u64, GitHub integration disabled"
            );
            return None;
        }
    };

    let key_path = match std::env::var("GITHUB_APP_PRIVATE_KEY_PATH") {
        Ok(path) => path,
        Err(_) => {
            tracing::warn!("GITHUB_APP_PRIVATE_KEY_PATH not set, GitHub integration disabled");
            return None;
        }
    };

    let private_key_pem = match std::fs::read(&key_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(
                path = %key_path,
                error = %e,
                "failed to read GitHub App private key file, GitHub integration disabled"
            );
            return None;
        }
    };

    let app_name = std::env::var("GITHUB_APP_NAME").unwrap_or_else(|_| "trakkt".to_string());

    match GitHubClient::new(app_id, &private_key_pem, &app_name) {
        Ok(client) => Some(client),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to create GitHubClient, GitHub integration disabled"
            );
            None
        }
    }
}

// ─── Internal helpers ───────────────────────────────────────────────────────

/// Build the standard headers for GitHub API calls using an installation token.
fn api_headers(token: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Accept",
        "application/vnd.github+json".parse().expect("valid header value"),
    );
    headers.insert(
        "X-GitHub-Api-Version",
        "2022-11-28".parse().expect("valid header value"),
    );
    headers.insert(
        "Authorization",
        format!("Bearer {token}").parse().expect("valid header value"),
    );
    headers
}

/// Map a GitHub API error response to the appropriate `trakkt_core::Error` variant.
async fn map_github_error(
    status: u16,
    url: &str,
    response: reqwest::Response,
) -> Error {
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<failed to read response body>".to_string());

    match status {
        404 => {
            tracing::warn!(url = %url, body = %body, "GitHub API returned 404");
            Error::NotFound(format!("GitHub resource not found: {url}"))
        }
        401 => {
            tracing::warn!(url = %url, body = %body, "GitHub API returned 401");
            Error::Unauthorized("GitHub authentication failed".to_string())
        }
        403 => {
            tracing::warn!(url = %url, body = %body, "GitHub API returned 403");
            Error::Forbidden("GitHub API access denied".to_string())
        }
        _ => {
            tracing::warn!(
                url = %url,
                status = status,
                body = %body,
                "GitHub API returned unexpected error"
            );
            Error::Internal(format!(
                "GitHub API error (status {status}): {body}"
            ))
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation};
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::RsaPrivateKey;
    use std::sync::LazyLock;

    struct TestKeyPair {
        private_pem: Vec<u8>,
        public_pem: Vec<u8>,
    }

    static TEST_KEYS: LazyLock<TestKeyPair> = LazyLock::new(|| {
        let mut rng = rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048)
            .expect("failed to generate test RSA key");
        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("failed to encode private key")
            .as_bytes()
            .to_vec();
        let public_key = private_key.to_public_key();
        let public_pem = rsa::pkcs8::EncodePublicKey::to_public_key_pem(&public_key, LineEnding::LF)
            .expect("failed to encode public key")
            .as_bytes()
            .to_vec();
        TestKeyPair { private_pem, public_pem }
    });

    #[test]
    fn jwt_generation_produces_valid_token() {
        let client = GitHubClient::new(12345, &TEST_KEYS.private_pem, "test-app").unwrap();
        let jwt = client.app_jwt().unwrap();

        // Decode and validate with the public key
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["12345"]);
        // Allow some clock skew for test stability
        validation.leeway = 120;

        let key = DecodingKey::from_rsa_pem(&TEST_KEYS.public_pem).unwrap();
        let token_data =
            jsonwebtoken::decode::<GitHubAppClaims>(&jwt, &key, &validation).unwrap();

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
        let err = result.unwrap_err();
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
        let client = result.unwrap();
        assert_eq!(client.app_name(), "my-app");
    }

    #[test]
    fn jwt_claims_have_correct_timing() {
        let client = GitHubClient::new(42, &TEST_KEYS.private_pem, "timing-test").unwrap();
        let jwt = client.app_jwt().unwrap();

        // Decode with full signature validation using the public key
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = false;
        validation.set_issuer(&["42"]);
        // Allow generous leeway since we're testing timing, not expiry
        validation.leeway = 120;

        let key = DecodingKey::from_rsa_pem(&TEST_KEYS.public_pem).unwrap();
        let token_data =
            jsonwebtoken::decode::<GitHubAppClaims>(&jwt, &key, &validation).unwrap();

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
