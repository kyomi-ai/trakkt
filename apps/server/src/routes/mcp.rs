// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP Server endpoints — JSON-RPC over Streamable HTTP.
//!
//! Implements the MCP 2025-03-26 spec with domain tools for the Trakkt issue
//! tracker. Tools call the same service layer as the UI, so mutations
//! broadcast via WebSocket to all connected clients.
//!
//! ## Endpoints
//!
//! - `POST /mcp` — JSON-RPC 2.0 request/response (initialize, tools/list, tools/call)
//! - `GET  /mcp` — SSE stream for server-initiated notifications
//! - `DELETE /mcp` — Terminate an MCP session
//!
//! ## Authentication
//!
//! Two auth methods are supported, tried in order:
//!
//! 1. **JWT (OAuth 2.0)** — `Authorization: Bearer <jwt>`. The `AuthUser`
//!    extractor validates the JWT and loads workspace context from the database.
//!    This is the standard path for MCP clients that use OAuth 2.0 (Claude Code,
//!    Cursor).
//!
//! 2. **API token (legacy)** — `Authorization: Bearer <trakkt-...>`. Raw Bearer
//!    tokens created in workspace settings, stored hashed (SHA-256) in the
//!    `api_tokens` table. Kept as a fallback for users with existing API tokens.
//!
//! In personal mode, both are bypassed — a local user context is injected.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::state::AppState;
use super::auth_shared::{self, ResolvedAuth};

const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MCP_SERVER_NAME: &str = "trakkt-mcp";
const MCP_SERVER_VERSION: &str = "0.1.0";
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

// ─────────────────────────────────────────────────────────────────────────────
// JSON-RPC 2.0 types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into() }),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WWW-Authenticate middleware (RFC 6750)
// ─────────────────────────────────────────────────────────────────────────────

/// Response layer that adds `WWW-Authenticate` to 401 responses per RFC 6750.
///
/// MCP clients (and directories like Glama) use this header to discover OAuth
/// endpoints and distinguish "online, requires auth" from "broken/offline".
/// The `resource_metadata` parameter points to the RFC 9728 protected resource
/// metadata endpoint, which in turn references the authorization server.
async fn mcp_www_authenticate_layer(request: Request, next: Next) -> Response {
    // Capture the request's host/scheme before passing ownership to `next`.
    let base_url = {
        let headers = request.headers();
        let scheme = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("https");
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("app.trakkt.dev");
        format!("{scheme}://{host}")
    };

    let mut response = next.run(request).await;

    if response.status() == StatusCode::UNAUTHORIZED {
        let www_auth = format!(
            r#"Bearer realm="OAuth", resource_metadata="{base_url}/.well-known/oauth-protected-resource", error="invalid_token", error_description="Missing or invalid access token""#
        );
        if let Ok(val) = www_auth.parse() {
            response.headers_mut().insert("www-authenticate", val);
        }
    }

    response
}

// ─────────────────────────────────────────────────────────────────────────────
// Routes
// ─────────────────────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(handle_post))
        .route("/", get(handle_sse))
        .route("/", delete(handle_delete))
        .layer(middleware::from_fn(mcp_www_authenticate_layer))
        .route(
            "/.well-known/openid-configuration",
            get(super::oauth::mcp_openid_configuration),
        )
}

// ─────────────────────────────────────────────────────────────────────────────
// POST /mcp — JSON-RPC request handler
// ─────────────────────────────────────────────────────────────────────────────

async fn handle_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> Response {
    let is_initialize = request.method == "initialize";

    // Authenticate ALL requests in non-personal mode. Claude.ai uses the 401
    // on `initialize` to discover that OAuth is required and prompt the user.
    // Letting `initialize` through unauthenticated causes claude.ai to think
    // it's connected and show "no tools available" instead of prompting for auth.
    // Resolve auth once and reuse the result for both the gate check and
    // session creation (avoids a redundant database round-trip).
    let resolved_auth = if state.config.is_personal() {
        None // Personal mode bypasses auth entirely
    } else {
        let auth = auth_shared::resolve_auth(&headers, &state).await;
        if auth.is_none() {
            return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Not authenticated"}))).into_response();
        }
        auth
    };

    let response = match request.method.as_str() {
        "initialize" => handle_initialize(request.id, request.params),
        "notifications/initialized" => {
            return (StatusCode::ACCEPTED, "").into_response();
        }
        "tools/list" => handle_tools_list(request.id),
        "tools/call" => handle_tools_call(request.id, request.params, &headers, &state).await,
        "resources/list" => handle_resources_list(request.id),
        "ping" => JsonRpcResponse::success(request.id, json!({})),
        _ => JsonRpcResponse::error(request.id, -32601, format!("Method not found: {}", request.method)),
    };

    let mut resp_headers = HeaderMap::new();

    if is_initialize {
        let workspace_id = resolved_auth
            .as_ref()
            .map(|a| a.workspace_id.as_str())
            .unwrap_or("anonymous");
        let session_id = state.mcp_sessions.create_session(workspace_id).await;
        if let Ok(val) = session_id.parse() {
            resp_headers.insert(MCP_SESSION_ID_HEADER, val);
        }
    }

    (StatusCode::OK, resp_headers, Json(response)).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// GET /mcp — SSE stream (placeholder)
// ─────────────────────────────────────────────────────────────────────────────

async fn handle_sse(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.config.is_personal() && auth_shared::resolve_auth(&headers, &state).await.is_none() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Not authenticated"}))).into_response();
    }
    (StatusCode::OK, "event: ping\ndata: {}\n\n").into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// DELETE /mcp — session termination
// ─────────────────────────────────────────────────────────────────────────────

async fn handle_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(session_id) = headers.get(MCP_SESSION_ID_HEADER).and_then(|v| v.to_str().ok()) {
        state.mcp_sessions.remove_session(session_id).await;
    }
    StatusCode::NO_CONTENT
}

// ─────────────────────────────────────────────────────────────────────────────
// Method handlers
// ─────────────────────────────────────────────────────────────────────────────

fn handle_initialize(id: Option<Value>, _params: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(id, json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "version": MCP_SERVER_VERSION,
        },
        "capabilities": {
            "tools": { "listChanged": true },
            "resources": { "subscribe": false, "listChanged": false },
        },
    }))
}

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    let mut tools = Vec::new();

    for op in trakkt_api::all_operations() {
        let schema = (op.json_schema)();
        let schema_value = match serde_json::to_value(&schema) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(tool = %op.name, error = %e, "failed to serialize tool schema");
                continue;
            }
        };
        tools.push(json!({
            "name": op.name,
            "description": op.description,
            "inputSchema": schema_value,
        }));
    }

    JsonRpcResponse::success(id, json!({ "tools": tools }))
}

fn handle_resources_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(id, json!({ "resources": [] }))
}

// ─────────────────────────────────────────────────────────────────────────────
// tools/call dispatcher
// ─────────────────────────────────────────────────────────────────────────────

async fn handle_tools_call(
    id: Option<Value>,
    params: Option<Value>,
    headers: &HeaderMap,
    state: &AppState,
) -> JsonRpcResponse {
    let tool_name = params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("unknown");

    let arguments = params
        .as_ref()
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(json!({}));

    // Authenticate — personal mode skips token auth (single-user, no login).
    let auth = if state.config.is_personal() {
        ResolvedAuth {
            workspace_id: "workspace-local".to_string(),
            user_id: "user-local".to_string(),
            scopes: vec![],
        }
    } else {
        match auth_shared::resolve_auth(headers, state).await {
            Some(auth) => auth,
            None => {
                return JsonRpcResponse::error(
                    id,
                    -32001,
                    "Authentication required. Provide a valid Bearer token in the Authorization header.",
                );
            }
        }
    };

    // Build the operations list once — used for both scope lookup and dispatch.
    let ops = trakkt_api::all_operations();

    // Scope enforcement — look up in the pre-built ops list.
    let required_scope = ops
        .iter()
        .find(|op| op.name == tool_name)
        .map(|op| op.scope)
        .unwrap_or("");
    if !required_scope.is_empty() && !auth.has_scope(required_scope) {
        return JsonRpcResponse::error(
            id,
            -32001,
            format!("Token lacks required scope: {required_scope}"),
        );
    }

    // Dispatch through the pre-built ops list.
    let result = dispatch_registry_tool(tool_name, arguments, &auth, state, &ops).await;

    match result {
        Ok(content) => JsonRpcResponse::success(id, json!({
            "content": [{ "type": "text", "text": content }]
        })),
        Err(e) => {
            // Map domain errors to MCP error responses with appropriate codes.
            let (code, message) = match &e {
                trakkt_core::Error::NotFound(msg) => (-32602, msg.clone()),
                trakkt_core::Error::BadRequest(msg) => (-32602, msg.clone()),
                trakkt_core::Error::Forbidden(msg) => (-32001, msg.clone()),
                trakkt_core::Error::Conflict(msg) => (-32602, msg.clone()),
                trakkt_core::Error::TooManyRequests(msg, _) => (-32000, msg.clone()),
                trakkt_core::Error::NotImplemented(msg) => (-32601, msg.clone()),
                trakkt_core::Error::ServiceUnavailable(msg) => (-32000, msg.clone()),
                _ => {
                    tracing::error!(tool = %tool_name, error = %e, "MCP tool call failed");
                    (-32603, format!("Internal error: {e}"))
                }
            };
            JsonRpcResponse::error(id, code, message)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registry-driven tool dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch a tool call through the shared API handler.
///
/// Accepts the pre-built operations list from `handle_tools_call` to avoid
/// redundant `all_operations()` calls. Builds an [`ApiCtx`] from the auth
/// context, calls the handler, and serializes the result to JSON.
async fn dispatch_registry_tool(
    name: &str,
    arguments: serde_json::Value,
    auth: &ResolvedAuth,
    state: &AppState,
    ops: &[trakkt_api::ApiOperation],
) -> trakkt_core::Result<String> {
    let op = ops
        .iter()
        .find(|o| o.name == name)
        .ok_or_else(|| trakkt_core::Error::BadRequest(format!("Unknown registry tool: {name}")))?;

    let ctx = trakkt_api::ApiCtx::from_bearer(
        auth.workspace_id.clone(),
        auth.user_id.clone(),
        &state.db,
        &state.ws_manager,
        &*state.attachment_storage,
        state.github_client.as_deref(),
        Some(&*state.encryption_key),
        &state.config.frontend_url,
    );

    let result = (op.handler)(ctx, arguments).await.map_err(|e| match e {
        trakkt_api::ApiError::NotFound(msg) => trakkt_core::Error::NotFound(msg),
        trakkt_api::ApiError::BadRequest(msg) => trakkt_core::Error::BadRequest(msg),
        trakkt_api::ApiError::Unauthorized(msg) => trakkt_core::Error::Forbidden(msg),
        trakkt_api::ApiError::Forbidden(msg) => trakkt_core::Error::Forbidden(msg),
        trakkt_api::ApiError::Conflict(msg) => trakkt_core::Error::Conflict(msg),
        trakkt_api::ApiError::TooManyRequests(msg) => trakkt_core::Error::TooManyRequests(msg, 0),
        trakkt_api::ApiError::NotImplemented(msg) => trakkt_core::Error::NotImplemented(msg),
        trakkt_api::ApiError::ServiceUnavailable(msg) => trakkt_core::Error::ServiceUnavailable(msg),
        trakkt_api::ApiError::Internal(msg) => trakkt_core::Error::Internal(msg),
    })?;

    serde_json::to_string_pretty(&result).map_err(trakkt_core::Error::from)
}
