// SPDX-License-Identifier: AGPL-3.0-or-later

//! Application configuration loaded from environment variables.

use std::env;

/// Deployment mode for the Tane backend.
///
/// Set via `TANE_MODE` env var. Falls back to `SELF_HOSTED` for backward compat.
/// - `saas`: Multi-tenant hosted service
/// - `self_hosted`: Team server with full auth (password, optional OAuth)
/// - `personal`: Single-user desktop app — zero auth, SQLite, localhost only
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaneMode {
    Saas,
    SelfHosted,
    Personal,
}

impl TaneMode {
    /// Derive the legacy `self_hosted` bool from the mode.
    fn self_hosted(&self) -> bool {
        matches!(self, TaneMode::SelfHosted | TaneMode::Personal)
    }
}

/// Central application configuration.
///
/// Loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL connection string
    pub database_url: String,

    /// Redis connection string.
    ///
    /// When `None`, the server falls back to the in-memory KV store
    /// (suitable for single-instance / self-hosted deployments).
    pub redis_url: Option<String>,

    /// JWT signing secret (HS256)
    pub jwt_secret: String,

    /// AES-256-GCM encryption key for credentials at rest
    pub encryption_key: String,

    /// Server listen port
    pub port: u16,

    /// Self-hosted mode
    pub self_hosted: bool,

    /// Deployment mode. Determines auth strategy, database backend, and UI surface.
    pub mode: TaneMode,

    /// Whether SMTP is configured at startup time.
    pub smtp_configured: bool,

    // ── Auth Methods ─────────────────────────────────────────────────────
    /// Enable passkey (WebAuthn) authentication. Defaults to true.
    pub passkeys_enabled: bool,

    /// Enable password-based authentication. Defaults to true.
    pub password_auth_enabled: bool,

    // ── Google OAuth ────────────────────────────────────────────────────
    /// Google OAuth client ID
    pub google_oauth_client_id: Option<String>,

    /// Google OAuth client secret
    pub google_oauth_client_secret: Option<String>,

    // ── WebAuthn (Passkeys) ─────────────────────────────────────────────
    /// Relying Party ID for WebAuthn (e.g., "localhost" or "tane.ai")
    pub webauthn_rp_id: String,

    /// Relying Party display name
    pub webauthn_rp_name: String,

    // ── Notifications ──────────────────────────────────────────────────
    /// Slack webhook URL for admin notifications (signups, etc.)
    pub slack_feedback_webhook_url: Option<String>,

    /// Support email address (for admin notification emails)
    pub support_email: String,

    // ── Frontend ────────────────────────────────────────────────────────
    /// Frontend URL for constructing callback/redirect URLs
    pub frontend_url: String,

    /// Backend base URL for constructing OAuth redirect URIs
    pub base_url: String,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Panics on missing required variables — fail fast at startup.
    pub fn from_env() -> Self {
        // Determine deployment mode.
        // TANE_MODE takes precedence; fall back to SELF_HOSTED for backward compat.
        let mode = match env::var("TANE_MODE").unwrap_or_default().to_lowercase().as_str() {
            "personal" => TaneMode::Personal,
            "self_hosted" | "selfhosted" => TaneMode::SelfHosted,
            "saas" => TaneMode::Saas,
            _ => {
                // Backward compat: check legacy SELF_HOSTED bool
                if env::var("SELF_HOSTED")
                    .unwrap_or_else(|_| "false".into())
                    .parse()
                    .unwrap_or(false)
                {
                    TaneMode::SelfHosted
                } else {
                    TaneMode::Saas
                }
            }
        };

        let base_url = env::var("BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8003".into());
        let frontend_url = env::var("FRONTEND_URL")
            .unwrap_or_else(|_| base_url.clone());

        Self {
            database_url: required_env("DATABASE_URL"),
            redis_url: env::var("REDIS_URL").ok(),
            jwt_secret: required_env("JWT_SECRET_KEY"),
            encryption_key: required_env("ENCRYPTION_KEY"),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8003".into())
                .parse()
                .expect("PORT must be a valid u16"),
            self_hosted: mode.self_hosted(),
            mode,
            smtp_configured: env::var("SMTP_HOST").is_ok() && env::var("SMTP_USER").is_ok(),
            passkeys_enabled: env::var("PASSKEYS_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()
                .unwrap_or(true),
            password_auth_enabled: env::var("PASSWORD_AUTH_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()
                .unwrap_or(true),
            google_oauth_client_id: env::var("GOOGLE_OAUTH_CLIENT_ID").ok(),
            google_oauth_client_secret: env::var("GOOGLE_OAUTH_CLIENT_SECRET").ok(),
            webauthn_rp_id: env::var("WEBAUTHN_RP_ID").unwrap_or_else(|_| {
                url::Url::parse(&frontend_url)
                    .ok()
                    .and_then(|u| u.host_str().map(String::from))
                    .unwrap_or_else(|| "localhost".into())
            }),
            webauthn_rp_name: env::var("WEBAUTHN_RP_NAME")
                .unwrap_or_else(|_| "Tane".into()),
            slack_feedback_webhook_url: env::var("SLACK_FEEDBACK_WEBHOOK_URL").ok(),
            support_email: env::var("SUPPORT_EMAIL")
                .unwrap_or_else(|_| "support@tane.ai".into()),
            frontend_url,
            base_url,
        }
    }

    /// Load configuration for tests with sensible defaults.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn test_config() -> Self {
        const TEST_ENCRYPTION_KEY_B64: &str = "dGVzdC1hZXMta2V5LWZvci11bml0LXRlc3RzISEhISE=";

        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://tane_test:test@localhost:5434/tane_test".into()),
            redis_url: env::var("REDIS_URL").ok(),
            jwt_secret: env::var("JWT_SECRET_KEY")
                .unwrap_or_else(|_| "test-jwt-secret-not-for-production".into()),
            encryption_key: env::var("ENCRYPTION_KEY")
                .unwrap_or_else(|_| TEST_ENCRYPTION_KEY_B64.into()),
            port: 0,
            self_hosted: false,
            mode: TaneMode::Saas,
            smtp_configured: false,
            passkeys_enabled: true,
            password_auth_enabled: true,
            google_oauth_client_id: Some("test-google-client-id".into()),
            google_oauth_client_secret: Some("test-google-client-secret".into()),
            webauthn_rp_id: "localhost".into(),
            webauthn_rp_name: "Tane Test".into(),
            slack_feedback_webhook_url: None,
            support_email: "test@tane.ai".into(),
            frontend_url: "http://localhost:5173".into(),
            base_url: "http://localhost:8003".into(),
        }
    }
}

impl Config {
    /// Returns true if the server is running in personal (desktop) mode.
    pub fn is_personal(&self) -> bool {
        self.mode == TaneMode::Personal
    }

    /// Returns true if SMTP is configured.
    pub fn smtp_configured(&self) -> bool {
        self.smtp_configured
    }
}

fn required_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| panic!("{key} environment variable is required"))
}
