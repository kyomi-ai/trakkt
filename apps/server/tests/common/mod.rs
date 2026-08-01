// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared fixtures for `trakkt-server` route tests.
//!
//! Route tests drive a real `Router` with a real [`AppState`], so the extractors
//! under test — `AuthUser` above all — run exactly as they do in production
//! rather than against a stand-in. Every field of the state is constructible
//! offline: in-memory SQLite, in-memory KV, local attachment storage, and no
//! Redis, Stripe or GitHub client.

use std::collections::HashMap;
use std::sync::Arc;

use trakkt_auth::mcp_session_manager::MCPSessionManager;
use trakkt_auth::websocket::WebSocketManager;
use trakkt_core::kv_store_memory::InMemoryKVStore;
use trakkt_server::state::AppState;

/// Lifetime of tokens minted by [`access_token`]. Long enough that no test can
/// race the expiry, short enough to stay obviously a test artefact.
const TOKEN_TTL_MINUTES: i64 = 60;

/// An [`AppState`] backed by a fresh, migrated, in-memory database.
///
/// Each call gets its own database, so tests in the same binary never see each
/// other's rows.
pub async fn test_state() -> AppState {
    let db = trakkt_core::test_helpers::test_pool()
        .await
        .expect("in-memory SQLite pool");

    let config = trakkt_core::Config::test_config();

    // In personal mode `AuthUser` returns the local user without looking at the
    // request at all (`crates/trakkt-auth/src/middleware.rs:77`). Every
    // authentication assertion built on this state would then pass with no
    // token and prove nothing, so assert the mode here: if `test_config`'s
    // default ever changes, these tests fail loudly instead of going quietly
    // vacuous.
    assert!(
        !config.is_personal(),
        "route auth tests require a non-personal config — personal mode skips \
         JWT validation entirely and would make every auth assertion vacuous"
    );

    let kv = InMemoryKVStore::new_pool();

    let webauthn = trakkt_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &url::Url::parse(&config.frontend_url).expect("test config frontend_url is a URL"),
    )
    .expect("build webauthn");

    // Reads `attachment_storage`/`attachment_local_path` from the config, which
    // `test_config` points at a local temp directory — no network, no S3.
    let attachment_storage =
        trakkt_auth::attachment_storage::create_storage(&config).expect("local attachment storage");

    AppState {
        ws_manager: WebSocketManager::new(None, db.clone()),
        mcp_sessions: MCPSessionManager::new(kv.clone()),
        // A valid 32-byte AES key, but deliberately not the one `config` names:
        // decoding `config.encryption_key` needs `base64`, which is a regular
        // dependency of this crate and therefore not linkable from an
        // integration test. No route exercised through this fixture encrypts or
        // decrypts anything, so the value is never observed. A test that does
        // reach credential storage must pass the real key instead of this.
        encryption_key: Arc::new([0u8; 32]),
        webauthn: Arc::new(webauthn),
        attachment_storage: Arc::from(attachment_storage),
        config: Arc::new(config),
        db,
        kv,
        redis: None,
        stripe: None,
        github_client: None,
    }
}

/// Mint a real access token for `user_id`, scoped to `workspace_id`.
///
/// Signed with the same secret the state carries, so `AuthUser` validates it by
/// the production path. Both ids must have matching rows in the state's
/// database: the extractor re-reads the user, the workspace and the membership,
/// and drops the workspace context if any of them is missing — which surfaces
/// as a 403 rather than the 200 a positive test expects.
pub fn access_token(state: &AppState, user_id: &str, workspace_id: &str) -> String {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(user_id));
    extra.insert("workspace_id".into(), serde_json::json!(workspace_id));

    trakkt_auth::jwt::create_access_token_str(
        user_id,
        &state.config.jwt_secret,
        TOKEN_TTL_MINUTES,
        extra,
    )
    .expect("mint access token")
}

/// The name of the cookie `AuthUser` falls back to when there is no
/// `Authorization` header, read from the same shared constants the extractor
/// uses (`crates/trakkt-auth/src/middleware.rs:192-206`).
pub fn access_token_cookie_name() -> &'static str {
    &trakkt_core::constants::get().cookies.access_token_name
}
