// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared auth resolution for MCP and REST surfaces.
//!
//! Both the MCP JSON-RPC and REST transports authenticate via the same two
//! methods (tried in order):
//!
//! 1. **JWT (OAuth 2.0)** — `Authorization: Bearer <jwt>`.
//! 2. **API token (legacy)** — `Authorization: Bearer <trakkt-...>`, SHA-256
//!    hash lookup against `api_tokens`.
//!
//! In personal mode, callers bypass this module entirely and inject a local
//! user context directly.

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};
use trakkt_types::enums::ActionSource;

use crate::state::AppState;

/// Resolved identity from Bearer token authentication.
///
/// Shared by MCP and REST surfaces. Personal mode callers construct this
/// directly with local workspace/user IDs.
pub struct ResolvedAuth {
    pub workspace_id: String,
    pub user_id: String,
    pub scopes: Vec<String>,
    pub action_source: ActionSource,
    pub action_source_label: Option<String>,
}

impl ResolvedAuth {
    /// Check whether this auth context grants the given scope.
    ///
    /// An empty scopes list means "full access" (JWT users).
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.is_empty() || self.scopes.iter().any(|s| s == scope)
    }
}

/// Internal row type for the `api_tokens` lookup query.
#[derive(sqlx::FromRow)]
struct ApiTokenLookupRow {
    user_id: String,
    workspace_id: Option<String>,
    scopes: Option<String>,
    name: String,
}

/// Try to authenticate via JWT (OAuth 2.0) or legacy API token.
///
/// Returns `None` if neither method succeeds. Checks in order:
/// 1. `Authorization: Bearer <token>` header (JWT or API token)
/// 2. `access_token` cookie (session JWT from browser)
pub async fn resolve_auth(headers: &HeaderMap, state: &AppState) -> Option<ResolvedAuth> {
    let token = extract_token(headers)?;

    // 1. Try JWT validation first (OAuth 2.0 path).
    if let Ok(decoded) = trakkt_auth::jwt::validate_token(&token, &state.config.jwt_secret) {
        let user_id = decoded
            .claims
            .extra
            .get("user_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| decoded.claims.sub.clone());

        // Verify user exists and is active.
        let user = trakkt_auth::user_service::get_user_by_id(&state.db, &user_id)
            .await
            .ok()??;

        if !user.active {
            return None;
        }

        // Get workspace_id from JWT claims, fall back to user's workspace context.
        let workspace_id = decoded
            .claims
            .extra
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let workspace_id = match workspace_id {
            Some(ws_id) => ws_id,
            None => {
                let ctx = trakkt_auth::user_service::get_user_workspace_context(
                    &state.db,
                    &user_id,
                )
                .await
                .ok()??;
                ctx.0.workspace_id
            }
        };

        // Check if this is an OAuth agent token (has client_name in extra claims).
        let (action_source, action_source_label) = match decoded
            .claims
            .extra
            .get("client_name")
            .and_then(|v| v.as_str())
        {
            Some(name) => (ActionSource::Agent, Some(name.to_string())),
            None => (ActionSource::User, None),
        };

        return Some(ResolvedAuth {
            workspace_id,
            user_id,
            scopes: vec![], // JWT users have full access
            action_source,
            action_source_label,
        });
    }

    // 2. Legacy API token path (SHA-256 hash lookup).
    authenticate_bearer_token(&token, &state.db).await
}

/// Validate a raw Bearer token against the `api_tokens` table (legacy path).
///
/// Hashes the plaintext token with SHA-256 and looks it up. Returns `None`
/// if the token is invalid/expired/revoked.
async fn authenticate_bearer_token(
    token: &str,
    db: &trakkt_core::DbPool,
) -> Option<ResolvedAuth> {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let token_hash = format!("{:x}", hasher.finalize());

    let is_pg = db.is_postgres();
    let bt = trakkt_core::sql_compat::bool_true(is_pg);

    let sql = format!(
        "SELECT user_id, workspace_id, scopes, name FROM api_tokens \
         WHERE token_hash = $1 AND active = {bt} \
         AND (expires_at IS NULL OR expires_at > $2)"
    );
    let now = chrono::Utc::now();

    let row: Option<ApiTokenLookupRow> = trakkt_core::db_fetch_optional!(
        db,
        ApiTokenLookupRow,
        &sql,
        &token_hash,
        &now
    )
    .ok()?;

    let row = row?;

    let scopes: Vec<String> = match row.scopes.as_deref() {
        None => vec![],
        Some(s) => match serde_json::from_str::<Vec<String>>(s) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "api_token has malformed scopes JSON; treating as empty");
                vec![]
            }
        },
    };

    // Resolve workspace_id: prefer token's workspace_id, fall back to first
    // workspace the user belongs to.
    let workspace_id = match row.workspace_id {
        Some(ws_id) => ws_id,
        None => {
            let ws: Option<(String,)> = trakkt_core::db_fetch_optional!(
                db,
                (String,),
                "SELECT workspace_id FROM workspace_users WHERE user_id = $1 LIMIT 1",
                &row.user_id
            )
            .ok()?;
            ws?.0
        }
    };

    Some(ResolvedAuth {
        workspace_id,
        user_id: row.user_id,
        scopes,
        action_source: ActionSource::Api,
        action_source_label: Some(row.name),
    })
}

/// Extract a bearer token from the Authorization header or `access_token` cookie.
fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth_header) = headers.get("authorization") {
        let value = auth_header.to_str().ok()?;
        if let Some(token) = value.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }

    let cookie_name = &trakkt_core::constants::get().cookies.access_token_name;
    let cookie_prefix = format!("{cookie_name}=");
    if let Some(cookie_header) = headers.get("cookie") {
        let cookies = cookie_header.to_str().ok()?;
        for cookie in cookies.split(';') {
            let cookie = cookie.trim();
            if let Some(token) = cookie.strip_prefix(&cookie_prefix) {
                return Some(token.to_string());
            }
        }
    }

    None
}
