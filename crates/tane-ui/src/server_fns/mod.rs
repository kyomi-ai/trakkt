// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions — typed RPC that replaces REST API calls.

pub mod auth;
pub mod context;
pub mod ownership;
pub mod profile;
pub mod security;
pub mod sidebar;
pub mod team;
pub mod workspace;

/// State provided to server functions via Leptos context.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct ServerContext {
    pub db: tane_core::DbPool,
    pub config: std::sync::Arc<tane_core::Config>,
    pub auth_state: tane_auth::middleware::AuthState,
    pub encryption_key: Option<std::sync::Arc<[u8; 32]>>,
    pub kv: Option<tane_core::KVPool>,
    pub redis: Option<tane_core::RedisPool>,
    pub webauthn: Option<std::sync::Arc<webauthn_rs::Webauthn>>,
    pub ws_manager: Option<tane_auth::websocket::WebSocketManager>,
    pub mcp_sessions: Option<tane_auth::mcp_session_manager::MCPSessionManager>,
}

/// Extract the authenticated user from the Axum request.
#[cfg(feature = "ssr")]
pub(crate) async fn extract_auth() -> Result<tane_auth::middleware::AuthUser, leptos::prelude::ServerFnError> {
    let ctx = extract_context()?;
    match leptos_axum::extract_with_state::<tane_auth::middleware::AuthUser, _>(&ctx.auth_state).await {
        Ok(auth) => Ok(auth),
        Err(e) => {
            leptos::prelude::expect_context::<leptos_axum::ResponseOptions>()
                .set_status(axum::http::StatusCode::UNAUTHORIZED);
            Err(leptos::prelude::ServerFnError::new(format!("Authentication required: {e}")))
        }
    }
}

/// Extract the server context from Leptos context.
#[cfg(feature = "ssr")]
pub(crate) fn extract_context() -> Result<ServerContext, leptos::prelude::ServerFnError> {
    leptos::prelude::use_context::<ServerContext>().ok_or_else(|| {
        tracing::error!("Server context not available");
        leptos::prelude::ServerFnError::new("Server context not available")
    })
}

/// Get workspace_id from the auth user, or error.
#[cfg(feature = "ssr")]
pub(crate) fn workspace_id(auth: &tane_auth::middleware::AuthUser) -> Result<&str, leptos::prelude::ServerFnError> {
    auth.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| {
            tracing::error!("Workspace context required");
            leptos::prelude::ServerFnError::new("Workspace context required")
        })
}

/// Bundles the three values every authenticated server function needs.
#[cfg(feature = "ssr")]
pub(crate) struct AuthenticatedContext {
    pub auth: tane_auth::middleware::AuthUser,
    pub ctx: ServerContext,
    pub ws_id: String,
}

#[cfg(feature = "ssr")]
impl AuthenticatedContext {
    pub(crate) async fn extract() -> Result<Self, leptos::prelude::ServerFnError> {
        let auth = extract_auth().await?;
        let ctx = extract_context()?;
        let ws_id = workspace_id(&auth)?.to_string();
        Ok(Self { auth, ctx, ws_id })
    }

    pub(crate) fn db(&self) -> &tane_core::DbPool {
        &self.ctx.db
    }
}

/// Extension trait for error conversion.
#[cfg(feature = "ssr")]
pub(crate) trait IntoServerFnError<T> {
    fn into_sfn(self) -> Result<T, leptos::prelude::ServerFnError>;
}

#[cfg(feature = "ssr")]
impl<T, E: std::fmt::Display> IntoServerFnError<T> for Result<T, E> {
    fn into_sfn(self) -> Result<T, leptos::prelude::ServerFnError> {
        self.map_err(|e| {
            tracing::error!(error = %e, "server function error");
            leptos::prelude::ServerFnError::new(e.to_string())
        })
    }
}
