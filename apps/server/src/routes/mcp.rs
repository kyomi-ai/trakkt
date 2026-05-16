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
use sha2::{Digest, Sha256};

use crate::state::AppState;

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
// Resolved MCP identity — from JWT or API token
// ─────────────────────────────────────────────────────────────────────────────

/// Resolved identity for MCP tool calls.
///
/// Populated from either a JWT `AuthUser` (OAuth 2.0) or a raw Bearer API
/// token (legacy). Personal mode injects a local context.
struct McpAuth {
    workspace_id: String,
    user_id: String,
    scopes: Vec<String>,
}

impl McpAuth {
    fn has_scope(&self, scope: &str) -> bool {
        self.scopes.is_empty() || self.scopes.iter().any(|s| s == scope)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Legacy Bearer API token auth (fallback)
// ─────────────────────────────────────────────────────────────────────────────

/// Internal row type for the api_tokens lookup query.
#[derive(sqlx::FromRow)]
struct ApiTokenLookupRow {
    user_id: String,
    workspace_id: Option<String>,
    scopes: Option<String>,
}

/// Try to authenticate the request via JWT (OAuth 2.0) or legacy API token.
/// Returns `None` if neither succeeds.
///
/// JWT is tried first because it produces a richer context (workspace context,
/// user active/verified checks). The API token path is a simple hash lookup —
/// kept for backward compatibility with existing API tokens.
async fn resolve_mcp_auth(
    headers: &HeaderMap,
    state: &AppState,
) -> Option<McpAuth> {
    // Extract the raw token from Authorization header
    let auth_header = headers.get("authorization")?.to_str().ok()?;
    let token = auth_header.strip_prefix("Bearer ")?;

    // 1. Try JWT validation first (OAuth 2.0 path).
    //    If the token is a valid JWT, resolve user + workspace from the database
    //    using the same logic as the AuthUser extractor.
    if let Ok(decoded) = trakkt_auth::jwt::validate_token(token, &state.config.jwt_secret) {
        let user_id = decoded
            .claims
            .extra
            .get("user_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| decoded.claims.sub.clone());

        // Verify user exists and is active
        let user = trakkt_auth::user_service::get_user_by_id(&state.db, &user_id)
            .await
            .ok()??;

        if !user.active {
            return None;
        }

        // Get workspace_id from JWT claims, fall back to user's workspace context
        let workspace_id = decoded
            .claims
            .extra
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or({
                // Synchronous fallback not possible here — but we can try
                // the database lookup below if JWT didn't include workspace_id.
                None
            });

        let workspace_id = match workspace_id {
            Some(ws_id) => ws_id,
            None => {
                // Fall back to user's first workspace
                let ctx = trakkt_auth::user_service::get_user_workspace_context(
                    &state.db,
                    &user_id,
                )
                .await
                .ok()??;
                ctx.0.workspace_id
            }
        };

        return Some(McpAuth {
            workspace_id,
            user_id,
            scopes: vec![], // JWT users have full access
        });
    }

    // 2. Legacy API token path (SHA-256 hash lookup)
    authenticate_bearer_token(token, &state.db).await
}

/// Validate a raw Bearer token against the `api_tokens` table (legacy path).
///
/// Hashes the plaintext token with SHA-256 and looks it up. Returns `None`
/// if the token is invalid/expired/revoked.
async fn authenticate_bearer_token(
    token: &str,
    db: &trakkt_core::DbPool,
) -> Option<McpAuth> {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let token_hash = format!("{:x}", hasher.finalize());

    let is_pg = db.is_postgres();
    let bt = trakkt_core::sql_compat::bool_true(is_pg);

    let sql = format!(
        "SELECT user_id, workspace_id, scopes FROM api_tokens \
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

    Some(McpAuth {
        workspace_id,
        user_id: row.user_id,
        scopes,
    })
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
    if !state.config.is_personal() && resolve_mcp_auth(&headers, &state).await.is_none() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Not authenticated"}))).into_response();
    }

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
        let workspace_id = match resolve_mcp_auth(&headers, &state).await {
            Some(auth) => auth.workspace_id,
            None => "anonymous".to_string(),
        };
        let session_id = state.mcp_sessions.create_session(&workspace_id).await;
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
    if resolve_mcp_auth(&headers, &state).await.is_none() && !state.config.is_personal() {
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
        McpAuth {
            workspace_id: "workspace-local".to_string(),
            user_id: "user-local".to_string(),
            scopes: vec![],
        }
    } else {
        match resolve_mcp_auth(headers, state).await {
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

    // Scope enforcement — all from registry
    let required_scope = registry_tool_scope(tool_name).unwrap_or("");
    if !required_scope.is_empty() && !auth.has_scope(required_scope) {
        return JsonRpcResponse::error(
            id,
            -32001,
            format!("Token lacks required scope: {required_scope}"),
        );
    }

    // Dispatch — all through registry
    let result = dispatch_registry_tool(tool_name, arguments, &auth, state).await;

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

/// Look up the required scope for a tool by name.
///
/// Returns `None` if the tool is not in the registry.
fn registry_tool_scope(name: &str) -> Option<&'static str> {
    trakkt_api::all_operations()
        .into_iter()
        .find(|op| op.name == name)
        .map(|op| op.scope)
}

/// Dispatch a tool call through the shared API handler.
///
/// Builds an [`ApiCtx`] from the MCP auth context, calls the handler, and
/// serializes the result to a pretty-printed JSON string.
async fn dispatch_registry_tool(
    name: &str,
    arguments: serde_json::Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let ops = trakkt_api::all_operations();
    let op = ops
        .into_iter()
        .find(|o| o.name == name)
        .ok_or_else(|| trakkt_core::Error::BadRequest(format!("Unknown registry tool: {name}")))?;

    let ctx = trakkt_api::ApiCtx::from_bearer(
        auth.workspace_id.clone(),
        auth.user_id.clone(),
        &state.db,
        &state.ws_manager,
    );

    let result = (op.handler)(ctx, arguments).await.map_err(|e| match e {
        trakkt_api::ApiError::NotFound(msg) => trakkt_core::Error::NotFound(msg),
        trakkt_api::ApiError::BadRequest(msg) => trakkt_core::Error::BadRequest(msg),
        trakkt_api::ApiError::Unauthorized(msg) => trakkt_core::Error::Forbidden(msg),
        trakkt_api::ApiError::Forbidden(msg) => trakkt_core::Error::Forbidden(msg),
        trakkt_api::ApiError::Conflict(msg) => trakkt_core::Error::Conflict(msg),
        trakkt_api::ApiError::Internal(msg) => trakkt_core::Error::Internal(msg),
    })?;

    serde_json::to_string_pretty(&result).map_err(trakkt_core::Error::from)
}
