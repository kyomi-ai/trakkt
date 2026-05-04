// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared application state passed to all axum handlers.

use axum::extract::FromRef;
use tane_auth::mcp_session_manager::MCPSessionManager;
use tane_auth::middleware::AuthState;
use tane_auth::websocket::WebSocketManager;
use webauthn_rs::Webauthn;

/// Application-wide shared state.
#[derive(Clone)]
pub struct AppState {
    pub db: tane_core::DbPool,
    pub kv: tane_core::KVPool,
    pub redis: Option<tane_core::RedisPool>,
    pub config: std::sync::Arc<tane_core::Config>,
    pub encryption_key: std::sync::Arc<[u8; 32]>,
    pub webauthn: std::sync::Arc<Webauthn>,
    pub ws_manager: WebSocketManager,
    pub mcp_sessions: MCPSessionManager,
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
