// SPDX-License-Identifier: AGPL-3.0-or-later

//! Application constants — hardcoded defaults matching shared/constants.toml from Kyomi.
//!
//! TODO: port constants.toml loading from Kyomi. For now, all values are inlined.

use std::sync::OnceLock;

static CONSTANTS: OnceLock<Constants> = OnceLock::new();

/// Load constants (no-op — uses embedded defaults).
pub fn load_with_fallback() -> &'static Constants {
    get()
}

/// Get the global constants instance.
pub fn get() -> &'static Constants {
    CONSTANTS.get_or_init(Constants::default)
}

#[derive(Debug, Clone)]
pub struct Constants {
    pub jwt: JwtConstants,
    pub cookies: CookieConstants,
    pub redis: RedisConstants,
    pub rate_limits: RateLimitConstants,
    pub workspace: WorkspaceConstants,
}

impl Default for Constants {
    fn default() -> Self {
        Self {
            jwt: JwtConstants::default(),
            cookies: CookieConstants::default(),
            redis: RedisConstants::default(),
            rate_limits: RateLimitConstants::default(),
            workspace: WorkspaceConstants::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JwtConstants {
    pub access_token_expire_minutes: i64,
    pub refresh_token_expire_days: i64,
    pub refresh_token_prefix: String,
    pub refresh_token_grace_period_seconds: i64,
    pub email_verification_expire_hours: i64,
}

impl Default for JwtConstants {
    fn default() -> Self {
        Self {
            access_token_expire_minutes: 15,
            refresh_token_expire_days: 7,
            refresh_token_prefix: "rt_".into(),
            refresh_token_grace_period_seconds: 30,
            email_verification_expire_hours: 24,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CookieConstants {
    pub access_token_name: String,
    pub refresh_token_name: String,
    pub samesite: String,
    pub path: String,
    pub secure: bool,
    pub httponly: bool,
}

impl Default for CookieConstants {
    fn default() -> Self {
        let secure = std::env::var("FRONTEND_URL")
            .map(|u| u.starts_with("https://"))
            .unwrap_or(true);
        Self {
            access_token_name: "access_token".into(),
            refresh_token_name: "refresh_token".into(),
            samesite: if secure { "Strict" } else { "Lax" }.into(),
            path: "/".into(),
            secure,
            httponly: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RedisConstants {
    pub key_prefixes: RedisKeyPrefixes,
    pub ttls: RedisTtls,
}

impl Default for RedisConstants {
    fn default() -> Self {
        Self {
            key_prefixes: RedisKeyPrefixes::default(),
            ttls: RedisTtls::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RedisKeyPrefixes {
    pub rate_limit_ip: String,
    pub rate_limit_user: String,
    pub oauth_state: String,
    pub pending_signup: String,
    pub pending_terms: String,
    pub webauthn_challenge: String,
    pub totp_setup: String,
    pub recovery_session: String,
}

impl Default for RedisKeyPrefixes {
    fn default() -> Self {
        Self {
            rate_limit_ip: "rl:ip:{ip}:{endpoint}".into(),
            rate_limit_user: "rl:user:{user_id}:{endpoint}".into(),
            oauth_state: "oauth:{provider}:state:{state}".into(),
            pending_signup: "signup:pending:{token}".into(),
            pending_terms: "terms:pending:{token}".into(),
            webauthn_challenge: "webauthn:challenge:{challenge_id}".into(),
            totp_setup: "totp:setup:{user_id}".into(),
            recovery_session: "recovery:session:{session_id}".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RedisTtls {
    pub oauth_state: u64,
    pub pending_signup: u64,
    pub webauthn_challenge: u64,
    pub totp_setup: u64,
    pub recovery_session: u64,
}

impl Default for RedisTtls {
    fn default() -> Self {
        Self {
            oauth_state: 300,
            pending_signup: 3600,
            webauthn_challenge: 300,
            totp_setup: 600,
            recovery_session: 900,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConstants {
    pub login: RateLimitBucket,
    pub register: RateLimitBucket,
    pub refresh: RateLimitBucket,
    pub api_call: RateLimitBucket,
}

impl Default for RateLimitConstants {
    fn default() -> Self {
        Self {
            login: RateLimitBucket { ip_capacity: 10, user_capacity: 5, window_seconds: 300 },
            register: RateLimitBucket { ip_capacity: 5, user_capacity: 3, window_seconds: 3600 },
            refresh: RateLimitBucket { ip_capacity: 30, user_capacity: 10, window_seconds: 60 },
            api_call: RateLimitBucket { ip_capacity: 100, user_capacity: 50, window_seconds: 60 },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitBucket {
    pub ip_capacity: u32,
    pub user_capacity: u32,
    pub window_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct WorkspaceConstants {
    pub roles: WorkspaceRoles,
}

impl Default for WorkspaceConstants {
    fn default() -> Self {
        Self {
            roles: WorkspaceRoles::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceRoles {
    pub admin: String,
    pub user: String,
}

impl Default for WorkspaceRoles {
    fn default() -> Self {
        Self {
            admin: "workspace_admin".into(),
            user: "workspace_user".into(),
        }
    }
}
