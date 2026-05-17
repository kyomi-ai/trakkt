// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared application state passed to all axum handlers.

use axum::extract::FromRef;
use trakkt_auth::attachment_storage::AttachmentStorage;
use trakkt_auth::mcp_session_manager::MCPSessionManager;
use trakkt_auth::middleware::AuthState;
use trakkt_auth::websocket::WebSocketManager;
use webauthn_rs::Webauthn;

/// Application-wide shared state.
#[derive(Clone)]
pub struct AppState {
    pub db: trakkt_core::DbPool,
    pub kv: trakkt_core::KVPool,
    pub redis: Option<trakkt_core::RedisPool>,
    pub config: std::sync::Arc<trakkt_core::Config>,
    pub encryption_key: std::sync::Arc<[u8; 32]>,
    pub webauthn: std::sync::Arc<Webauthn>,
    pub ws_manager: WebSocketManager,
    pub mcp_sessions: MCPSessionManager,
    pub attachment_storage: std::sync::Arc<dyn AttachmentStorage>,
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        AuthState {
            jwt_secret: state.config.jwt_secret.clone(),
            db: state.db.clone(),
            is_personal: state.config.is_personal(),
        }
    }
}
