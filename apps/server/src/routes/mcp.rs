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
    // Authenticate in non-personal mode, exactly as the two siblings on this
    // route do — `handle_post` above and `handle_sse` at `:206`. Personal mode
    // (single-user desktop, no login) bypasses auth in all three. The resolved
    // value is bound rather than discarded because the ownership check below
    // needs the caller's workspace; that is `handle_post`'s shape, and it costs
    // the same one `resolve_auth` call `handle_sse` makes.
    let resolved_auth = if state.config.is_personal() {
        None // Personal mode bypasses auth entirely
    } else {
        let auth = auth_shared::resolve_auth(&headers, &state).await;
        if auth.is_none() {
            return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "Not authenticated"}))).into_response();
        }
        auth
    };

    if let Some(session_id) = headers.get(MCP_SESSION_ID_HEADER).and_then(|v| v.to_str().ok()) {
        // Authentication alone would still let any authenticated user of any
        // other workspace terminate this session, so ownership is checked too.
        // `create_session` records the workspace `initialize` authenticated as
        // (`crates/trakkt-auth/src/mcp_session_manager.rs:70-77`) and
        // `validate_session` reads it back, so the comparison costs one KV read.
        // A legitimate client is unaffected: it only ever deletes the session id
        // `initialize` handed it, which carries that client's own workspace.
        //
        // The refusal answers 204, not 403/404. An unknown session id already
        // answers 204 — `validate_session` returns `None`, the chain
        // short-circuits, and `remove_session` finds nothing to remove — so
        // answering differently here would tell an authenticated caller whether
        // a given session id exists in a workspace they are not in. No
        // legitimate client can observe the difference.
        //
        // Personal mode has no `ResolvedAuth` to compare against and skips this,
        // exactly as it skips the gate above.
        if let Some(auth) = &resolved_auth
            && let Some(session_workspace) = state.mcp_sessions.validate_session(session_id).await
            && session_workspace != auth.workspace_id
        {
            tracing::warn!(
                session_id,
                caller_workspace = %auth.workspace_id,
                "refused MCP session termination requested from another workspace"
            );
            return StatusCode::NO_CONTENT.into_response();
        }
        state.mcp_sessions.remove_session(session_id).await;
    }
    StatusCode::NO_CONTENT.into_response()
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

/// Rewrite a schemars-generated JSON Schema into the OpenAI function-calling
/// subset that most models' native tool-calling reliably accepts. Some models
/// (e.g. MiMo, Qwen-class) cannot bind a tool whose parameter schema uses
/// draft-2020-12 constructs and silently fall back to emitting the call as
/// `<function=...><parameter=...>` text — which arrives at the server with no
/// arguments.
///
/// Normalizations applied recursively:
/// - flatten nullable unions: `"type": ["string", "null"]` -> `"type": "string"`
/// - drop the `$schema` dialect declaration
/// - drop non-standard `format` hints (e.g. `int64`) that strict validators reject
fn sanitize_schema_for_tool_calling(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("$schema");
            map.remove("format");
            // Flatten nullable unions: `"type": ["string", "null"]` -> `"string"`.
            let flattened = map
                .get("type")
                .and_then(Value::as_array)
                .and_then(|types| types.iter().find(|t| t.as_str() != Some("null")).cloned());
            if let Some(non_null) = flattened {
                map.insert("type".to_string(), non_null);
            }
            for child in map.values_mut() {
                sanitize_schema_for_tool_calling(child);
            }
        }
        Value::Array(arr) => {
            for child in arr.iter_mut() {
                sanitize_schema_for_tool_calling(child);
            }
        }
        _ => {}
    }
}

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    let mut tools = Vec::new();

    for op in trakkt_api::all_operations() {
        let schema = (op.json_schema)();
        let mut schema_value = match serde_json::to_value(&schema) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(tool = %op.name, error = %e, "failed to serialize tool schema");
                continue;
            }
        };
        // Normalize every tool's schema to the OpenAI function-calling subset.
        // schemars emits draft-2020-12 constructs (nullable unions, `$schema`,
        // `int64` formats) that some models' tool-calling layers can't bind —
        // they silently fall back to emitting the call as `<function=...>` text,
        // which reaches the server with no arguments. Verified against
        // MiMo-V2.5-Pro via OpenRouter/OpenCode: with the raw schema the model
        // emits unparseable text; with the sanitized schema it calls natively.
        sanitize_schema_for_tool_calling(&mut schema_value);
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
            action_source: trakkt_types::enums::ActionSource::User,
            action_source_label: None,
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
        auth.action_source,
        auth.action_source_label.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_flattens_nullable_unions_and_strips_dialect() {
        // Mirrors what schemars emits for a params struct with Option<T> fields.
        let mut schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "GetIssueApiParams",
            "type": "object",
            "properties": {
                "issue_identifier": { "type": ["string", "null"] },
                "issue_number": { "type": ["integer", "null"], "format": "int64" },
                "team_key": { "type": ["null", "string"] },
                "body": { "type": "string" }
            },
            "required": ["body"]
        });

        sanitize_schema_for_tool_calling(&mut schema);

        // Dialect declaration dropped.
        assert!(schema.get("$schema").is_none());
        let props = &schema["properties"];
        // Nullable unions flattened to the non-null type (order-independent).
        assert_eq!(props["issue_identifier"]["type"], json!("string"));
        assert_eq!(props["issue_number"]["type"], json!("integer"));
        assert_eq!(props["team_key"]["type"], json!("string"));
        // Non-standard format hint dropped.
        assert!(props["issue_number"].get("format").is_none());
        // Plain required string untouched; required array preserved.
        assert_eq!(props["body"]["type"], json!("string"));
        assert_eq!(schema["required"], json!(["body"]));
    }

    #[test]
    fn sanitize_recurses_into_nested_schemas() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "filters": {
                    "type": ["array", "null"],
                    "items": {
                        "type": "object",
                        "properties": { "value": { "type": ["string", "null"] } }
                    }
                }
            }
        });

        sanitize_schema_for_tool_calling(&mut schema);

        assert_eq!(schema["properties"]["filters"]["type"], json!("array"));
        assert_eq!(
            schema["properties"]["filters"]["items"]["properties"]["value"]["type"],
            json!("string")
        );
    }
}
