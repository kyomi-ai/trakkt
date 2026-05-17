// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions — typed RPC that replaces REST API calls.

pub mod activities;
pub mod auth;
pub mod comments;
pub mod context;
pub mod favorites;
pub mod issues;
pub mod labels;
pub mod notifications;
pub mod ownership;
pub mod profile;
pub mod projects;
pub mod relations;
pub mod security;
pub mod sidebar;
pub mod statuses;
pub mod team;
pub mod teams;
pub mod views;
pub mod watchers;
pub mod workspace;

/// State provided to server functions via Leptos context.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct ServerContext {
    pub db: trakkt_core::DbPool,
    pub config: std::sync::Arc<trakkt_core::Config>,
    pub auth_state: trakkt_auth::middleware::AuthState,
    pub encryption_key: Option<std::sync::Arc<[u8; 32]>>,
    pub kv: Option<trakkt_core::KVPool>,
    pub redis: Option<trakkt_core::RedisPool>,
    pub webauthn: Option<std::sync::Arc<webauthn_rs::Webauthn>>,
    pub ws_manager: Option<trakkt_auth::websocket::WebSocketManager>,
    pub mcp_sessions: Option<trakkt_auth::mcp_session_manager::MCPSessionManager>,
}

#[cfg(feature = "ssr")]
impl ServerContext {
    pub(crate) fn webauthn(&self) -> Result<&std::sync::Arc<webauthn_rs::Webauthn>, leptos::prelude::ServerFnError> {
        self.webauthn.as_ref().ok_or_else(|| leptos::prelude::ServerFnError::new("WebAuthn not configured"))
    }

    pub(crate) fn kv(&self) -> Result<trakkt_core::KVPool, leptos::prelude::ServerFnError> {
        self.kv.clone().ok_or_else(|| leptos::prelude::ServerFnError::new("KV store not available"))
    }
}

/// Extract the authenticated user from the Axum request.
#[cfg(feature = "ssr")]
pub(crate) async fn extract_auth() -> Result<trakkt_auth::middleware::AuthUser, leptos::prelude::ServerFnError> {
    let ctx = extract_context()?;
    match leptos_axum::extract_with_state::<trakkt_auth::middleware::AuthUser, _>(&ctx.auth_state).await {
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
pub(crate) fn workspace_id(auth: &trakkt_auth::middleware::AuthUser) -> Result<&str, leptos::prelude::ServerFnError> {
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
    pub auth: trakkt_auth::middleware::AuthUser,
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

    pub(crate) fn db(&self) -> &trakkt_core::DbPool {
        &self.ctx.db
    }

    pub(crate) fn api_ctx(&self) -> trakkt_api::ApiCtx<'_> {
        trakkt_api::ApiCtx::from_leptos(
            self.ws_id.clone(),
            self.auth.user_id.clone(),
            self.db(),
            self.ctx.ws_manager.as_ref(),
        )
    }
}

/// Bundles headers, KV pool, client IP, and device info — the common preamble
/// for auth server functions that need request context beyond just `ServerContext`.
#[cfg(feature = "ssr")]
pub(crate) struct AuthFlowContext {
    pub ctx: ServerContext,
    pub kv: trakkt_core::KVPool,
    pub ip: String,
    pub device: trakkt_auth::token_service::DeviceInfo,
}

#[cfg(feature = "ssr")]
impl AuthFlowContext {
    pub(crate) async fn extract() -> Result<Self, leptos::prelude::ServerFnError> {
        let ctx = extract_context()?;
        let headers: axum::http::HeaderMap = leptos_axum::extract()
            .await
            .map_err(|e| leptos::prelude::ServerFnError::new(format!("Failed to extract headers: {e}")))?;
        let kv = ctx.kv()?;
        let ip = auth::extract_client_ip(&headers);
        let device = auth::extract_device_info(&headers);
        Ok(Self { ctx, kv, ip, device })
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
