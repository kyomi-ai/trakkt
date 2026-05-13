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

use trakkt_auth::{comment_service, issue_service, label_service, relation_service, status_service, team_service};
use trakkt_types::models::{CreateIssueParams, IssueFilters, IssueUpdate};

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

    // Parse scopes from JSON array string
    let scopes: Vec<String> = row
        .scopes
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

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

    // Authenticate all requests except `initialize` (which triggers the OAuth flow).
    // In personal mode, bypass auth entirely.
    if !is_initialize && request.method != "notifications/initialized" && !state.config.is_personal()
        && resolve_mcp_auth(&headers, &state).await.is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
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

async fn handle_sse(_headers: HeaderMap) -> impl IntoResponse {
    (StatusCode::OK, "event: ping\ndata: {}\n\n")
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
    JsonRpcResponse::success(id, json!({
        "tools": [
            {
                "name": "list_issues",
                "description": "List issues in the workspace with optional filters. Returns issues ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "team_id": {
                            "type": "string",
                            "description": "Filter by team ID. Use list_issues without this filter first to discover team_id values from the results."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Filter by team key (e.g. 'TRA'). Alternative to team_id — resolved server-side. team_id takes precedence if both are provided."
                        },
                        "status_id": {
                            "type": "string",
                            "description": "Filter by status ID (e.g. 'workspace-id::backlog'). Use list_statuses to get valid IDs."
                        },
                        "status_category": {
                            "type": "string",
                            "description": "Comma-separated status categories to include: backlog, unstarted, started, completed, cancelled. Example: 'backlog,unstarted' returns issues in either category."
                        },
                        "include_closed": {
                            "type": "boolean",
                            "description": "If true, include completed and cancelled issues. Default is false — these are excluded unless status_id or status_category is set."
                        },
                        "priority": {
                            "type": "integer",
                            "description": "Filter by priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low",
                            "enum": [0, 1, 2, 3, 4]
                        },
                        "assignee": {
                            "type": "string",
                            "description": "Filter by assignee user ID"
                        },
                        "label": {
                            "type": "string",
                            "description": "Filter by label ID"
                        },
                        "search": {
                            "type": "string",
                            "description": "Search text to match against issue titles"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of issues to return (default: 50, max: 100)",
                            "minimum": 1,
                            "maximum": 100
                        }
                    }
                }
            },
            {
                "name": "get_issue",
                "description": "Get a single issue by its team-scoped identifier (e.g. 'TRA-35'), including full details (description, labels, assignee, creator) and all comments.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "issue_identifier": {
                            "type": "string",
                            "description": "Issue identifier in 'TRA-35' format (team key + number). Preferred over separate team_key/issue_number."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Team key (e.g. 'TRA'). Required if issue_identifier is not provided."
                        },
                        "issue_number": {
                            "type": "integer",
                            "description": "Issue number within the team. Required if issue_identifier is not provided."
                        }
                    }
                }
            },
            {
                "name": "create_issue",
                "description": "Create a new issue in the workspace. Specify team_id or team_key to assign to a specific team, otherwise uses the default team. Starts in 'backlog' status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Issue title (required)"
                        },
                        "team_id": {
                            "type": "string",
                            "description": "Team ID to assign the issue to. If omitted, uses the default team."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Team key (e.g. 'TRA') as alternative to team_id. Resolved server-side. team_id takes precedence if both are provided."
                        },
                        "description": {
                            "type": "string",
                            "description": "Markdown description of the issue"
                        },
                        "priority": {
                            "type": "integer",
                            "description": "Priority level: 0=none (default), 1=urgent, 2=high, 3=medium, 4=low",
                            "enum": [0, 1, 2, 3, 4]
                        },
                        "labels": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Array of label IDs to attach to the issue"
                        },
                        "assignee": {
                            "type": "string",
                            "description": "User ID to assign the issue to"
                        },
                        "due_date": {
                            "type": "string",
                            "description": "Due date in ISO 8601 format (YYYY-MM-DD)"
                        }
                    },
                    "required": ["title"]
                }
            },
            {
                "name": "update_issue",
                "description": "Update an existing issue. Only provided fields are changed; omitted fields remain unchanged. Set a field to null to clear it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "issue_identifier": {
                            "type": "string",
                            "description": "Issue identifier in 'TRA-35' format (team key + number). Preferred over separate team_key/issue_number."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Team key (e.g. 'TRA'). Required if issue_identifier is not provided. Also used as the target team when moving an issue (use move_to_team_id or move_to_team_key instead)."
                        },
                        "issue_number": {
                            "type": "integer",
                            "description": "Issue number within the team. Required if issue_identifier is not provided."
                        },
                        "title": {
                            "type": "string",
                            "description": "New title for the issue"
                        },
                        "description": {
                            "type": ["string", "null"],
                            "description": "New markdown description, or null to clear"
                        },
                        "status_id": {
                            "type": "string",
                            "description": "New status ID. Use list_statuses to get valid IDs."
                        },
                        "priority": {
                            "type": "integer",
                            "description": "New priority: 0=none, 1=urgent, 2=high, 3=medium, 4=low",
                            "enum": [0, 1, 2, 3, 4]
                        },
                        "assignee": {
                            "type": ["string", "null"],
                            "description": "User ID to assign, or null to unassign"
                        },
                        "labels": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Replace all labels with this list of label IDs"
                        },
                        "due_date": {
                            "type": ["string", "null"],
                            "description": "Due date in ISO 8601 format, or null to clear"
                        },
                        "move_to_team_id": {
                            "type": "string",
                            "description": "Team ID to move the issue to (reassigns team and renumbers)"
                        },
                        "move_to_team_key": {
                            "type": "string",
                            "description": "Team key to move the issue to (e.g. 'ENG'). Alternative to move_to_team_id."
                        }
                    }
                }
            },
            {
                "name": "add_comment",
                "description": "Add a comment to an issue. Comments support markdown formatting.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "issue_identifier": {
                            "type": "string",
                            "description": "Issue identifier in 'TRA-35' format (team key + number). Preferred over separate team_key/issue_number."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Team key (e.g. 'TRA'). Required if issue_identifier is not provided."
                        },
                        "issue_number": {
                            "type": "integer",
                            "description": "Issue number within the team. Required if issue_identifier is not provided."
                        },
                        "body": {
                            "type": "string",
                            "description": "Markdown body of the comment"
                        }
                    },
                    "required": ["body"]
                }
            },
            {
                "name": "list_labels",
                "description": "List all labels in the workspace, ordered alphabetically by name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "create_label",
                "description": "Create a new label in the workspace.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Label name (must be unique within the workspace)"
                        },
                        "color": {
                            "type": "string",
                            "description": "Hex color code (e.g. '#FF5733' or 'FF5733')"
                        }
                    },
                    "required": ["name", "color"]
                }
            },
            {
                "name": "delete_issue",
                "description": "Delete an issue by its team-scoped identifier (e.g. 'TRA-35'). This permanently removes the issue and all associated comments and labels.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "issue_identifier": {
                            "type": "string",
                            "description": "Issue identifier in 'TRA-35' format (team key + number). Preferred over separate team_key/issue_number."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Team key (e.g. 'TRA'). Required if issue_identifier is not provided."
                        },
                        "issue_number": {
                            "type": "integer",
                            "description": "Issue number within the team. Required if issue_identifier is not provided."
                        }
                    }
                }
            },
            {
                "name": "search_issues",
                "description": "Search for issues by text query. Matches against issue titles. Returns results ordered by priority (urgent first), then by creation date (newest first). By default, completed and cancelled issues are excluded — pass include_closed=true to include them.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search text to match against issue titles"
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Optional team key (e.g. 'TRA') to scope search to a specific team."
                        },
                        "include_closed": {
                            "type": "boolean",
                            "description": "If true, include completed and cancelled issues in results. Default is false."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 20, max: 100)",
                            "minimum": 1,
                            "maximum": 100
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "list_statuses",
                "description": "List all statuses in the workspace, grouped by category (backlog, unstarted, started, completed, cancelled). Returns both global and optionally team-specific statuses.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "team_id": {
                            "type": "string",
                            "description": "Optional team ID to include team-specific statuses"
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Optional team key (e.g. 'TRA') as alternative to team_id. team_id takes precedence if both are provided."
                        }
                    }
                }
            },
            {
                "name": "list_teams",
                "description": "List all teams in the workspace, ordered alphabetically by name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "add_relation",
                "description": "Add a relation between two issues. Currently supports 'blocks' relation type (directional: source blocks target).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source_issue": {
                            "type": "string",
                            "description": "Source issue identifier in 'TRA-35' format (the issue that blocks)"
                        },
                        "target_issue": {
                            "type": "string",
                            "description": "Target issue identifier in 'TRA-35' format (the issue that is blocked)"
                        },
                        "relation_type": {
                            "type": "string",
                            "description": "Relation type. Currently only 'blocks' is supported.",
                            "enum": ["blocks"]
                        }
                    },
                    "required": ["source_issue", "target_issue", "relation_type"]
                }
            },
            {
                "name": "remove_relation",
                "description": "Remove a relation between two issues by its relation ID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "relation_id": {
                            "type": "string",
                            "description": "The relation ID to remove"
                        }
                    },
                    "required": ["relation_id"]
                }
            },
            {
                "name": "list_issue_relations",
                "description": "List all relations for an issue (both directions — blocks and blocked-by).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "issue_identifier": {
                            "type": "string",
                            "description": "Issue identifier in 'TRA-35' format (team key + number). Preferred over separate team_key/issue_number."
                        },
                        "team_key": {
                            "type": "string",
                            "description": "Team key (e.g. 'TRA'). Required if issue_identifier is not provided."
                        },
                        "issue_number": {
                            "type": "integer",
                            "description": "Issue number within the team. Required if issue_identifier is not provided."
                        }
                    }
                }
            }
        ]
    }))
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

    // Scope enforcement: map each tool to its required scope.
    let required_scope = match tool_name {
        "list_issues" | "get_issue" | "search_issues" => "issues:read",
        "create_issue" | "update_issue" | "delete_issue" => "issues:write",
        "add_comment" => "comments:write",
        "list_labels" => "labels:read",
        "create_label" => "labels:write",
        "list_statuses" => "issues:read",
        "list_teams" => "teams:read",
        "add_relation" | "remove_relation" => "issues:write",
        "list_issue_relations" => "issues:read",
        _ => "",
    };
    if !required_scope.is_empty() && !auth.has_scope(required_scope) {
        return JsonRpcResponse::error(
            id,
            -32001,
            format!("Token lacks required scope: {required_scope}"),
        );
    }

    let result = match tool_name {
        "list_issues" => tool_list_issues(&arguments, &auth, state).await,
        "get_issue" => tool_get_issue(&arguments, &auth, state).await,
        "create_issue" => tool_create_issue(&arguments, &auth, state).await,
        "update_issue" => tool_update_issue(&arguments, &auth, state).await,
        "delete_issue" => tool_delete_issue(&arguments, &auth, state).await,
        "add_comment" => tool_add_comment(&arguments, &auth, state).await,
        "list_labels" => tool_list_labels(&auth, state).await,
        "create_label" => tool_create_label(&arguments, &auth, state).await,
        "search_issues" => tool_search_issues(&arguments, &auth, state).await,
        "list_statuses" => tool_list_statuses(&arguments, &auth, state).await,
        "list_teams" => tool_list_teams(&auth, state).await,
        "add_relation" => tool_add_relation(&arguments, &auth, state).await,
        "remove_relation" => tool_remove_relation(&arguments, &auth, state).await,
        "list_issue_relations" => tool_list_issue_relations(&arguments, &auth, state).await,
        _ => return JsonRpcResponse::error(id, -32602, format!("Unknown tool: {tool_name}")),
    };

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
// Tool implementations
// ─────────────────────────────────────────────────────────────────────────────

/// Helper to extract a required string argument.
fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, trakkt_core::Error> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| trakkt_core::Error::BadRequest(format!("missing required parameter: {key}")))
}

/// Parse an issue identifier like "TRA-35" into (team_key, number).
fn parse_issue_identifier(identifier: &str) -> Option<(String, i32)> {
    let parts: Vec<&str> = identifier.splitn(2, '-').collect();
    if parts.len() == 2 && let Ok(number) = parts[1].parse::<i32>() {
        return Some((parts[0].to_string(), number));
    }
    None
}

/// Resolve `team_key` and `number` from either an `issue_identifier` parameter
/// (e.g. "TRA-35") or explicit `team_key` + `issue_number` parameters.
fn resolve_issue_key_and_number(args: &Value) -> Result<(String, i32), trakkt_core::Error> {
    if let Some(identifier) = args.get("issue_identifier").and_then(|v| v.as_str()) {
        parse_issue_identifier(identifier).ok_or_else(|| {
            trakkt_core::Error::BadRequest(
                "Invalid issue identifier format. Expected 'TRA-35'".to_string(),
            )
        })
    } else {
        let team_key = args
            .get("team_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                trakkt_core::Error::BadRequest(
                    "Either issue_identifier or team_key+issue_number required".to_string(),
                )
            })?;
        let number = args
            .get("issue_number")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| {
                trakkt_core::Error::BadRequest(
                    "Either issue_identifier or team_key+issue_number required".to_string(),
                )
            })? as i32;
        Ok((team_key.to_string(), number))
    }
}

/// Resolve a team_id from either `team_id` or `team_key` arguments.
/// Returns `None` if neither is provided. `team_id` takes precedence.
async fn resolve_team_id_from_args(
    args: &Value,
    db: &trakkt_core::DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<String>> {
    if let Some(id) = args.get("team_id").and_then(|v| v.as_str()) {
        return Ok(Some(id.to_string()));
    }
    if let Some(key) = args.get("team_key").and_then(|v| v.as_str()) {
        let team = team_service::get_team_by_key(db, workspace_id, key)
            .await?
            .ok_or_else(|| trakkt_core::Error::BadRequest(
                format!("No team found with key '{key}'"),
            ))?;
        return Ok(Some(team.team_id));
    }
    Ok(None)
}

/// list_issues — list issues with optional filters.
async fn tool_list_issues(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let limit_raw = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let limit = limit_raw.clamp(1, 100);

    let status_id = args.get("status_id").and_then(|v| v.as_str()).map(String::from);
    let status_categories: Option<Vec<String>> = args
        .get("status_category")
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect());
    let include_closed = args.get("include_closed").and_then(|v| v.as_bool()).unwrap_or(false);

    let exclude_status_categories = if status_id.is_none() && status_categories.is_none() && !include_closed {
        Some(vec!["completed".to_string(), "cancelled".to_string()])
    } else {
        None
    };

    let filters = IssueFilters {
        status_id,
        status_categories,
        exclude_status_categories,
        priority: args.get("priority").and_then(|v| v.as_i64()).map(|v| v as i32),
        assignee_id: args.get("assignee").and_then(|v| v.as_str()).map(String::from),
        label_id: args.get("label").and_then(|v| v.as_str()).map(String::from),
        search: args.get("search").and_then(|v| v.as_str()).map(String::from),
        limit: Some(limit),
        offset: None,
    };

    let team_id = resolve_team_id_from_args(args, &state.db, &auth.workspace_id).await?;
    let issues = issue_service::list_issues(&state.db, &auth.workspace_id, team_id.as_deref(), &filters).await?;
    serde_json::to_string_pretty(&issues).map_err(trakkt_core::Error::from)
}

/// get_issue — get a single issue with details and comments.
async fn tool_get_issue(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let (team_key, number) = resolve_issue_key_and_number(args)?;

    let issue = issue_service::get_issue(&state.db, &auth.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("issue {team_key}-{number} not found")))?;

    let comments = comment_service::list_comments(&state.db, &issue.issue_id).await?;

    let result = json!({
        "issue": issue,
        "comments": comments
    });
    serde_json::to_string_pretty(&result).map_err(trakkt_core::Error::from)
}

/// create_issue — create a new issue in the specified or default team.
async fn tool_create_issue(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let title = arg_str(args, "title")?;

    let resolved_team_id = match resolve_team_id_from_args(args, &state.db, &auth.workspace_id).await? {
        Some(id) => id,
        None => {
            let default_team = team_service::get_default_team(&state.db, &auth.workspace_id).await?;
            default_team.team_id
        }
    };

    let label_ids: Vec<String> = args
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let params = CreateIssueParams {
        workspace_id: auth.workspace_id.clone(),
        team_id: resolved_team_id,
        creator_id: auth.user_id.clone(),
        title: title.to_string(),
        description: args.get("description").and_then(|v| v.as_str()).map(String::from),
        priority: args.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        assignee_id: args.get("assignee").and_then(|v| v.as_str()).map(String::from),
        due_date: args.get("due_date").and_then(|v| v.as_str()).map(String::from),
        label_ids,
        project_id: args.get("project_id").and_then(|v| v.as_str()).map(String::from),
        milestone_id: args.get("milestone_id").and_then(|v| v.as_str()).map(String::from),
        parent_issue_id: args.get("parent_issue_id").and_then(|v| v.as_str()).map(String::from),
    };

    let issue = issue_service::create_issue(&state.db, &params, Some(&state.ws_manager)).await?;
    serde_json::to_string_pretty(&issue).map_err(trakkt_core::Error::from)
}

/// Resolve a target team_id for "move to team" from `move_to_team_id` or
/// `move_to_team_key` arguments. Returns `None` if neither is provided.
async fn resolve_move_team_id(
    args: &Value,
    db: &trakkt_core::DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<String>> {
    if let Some(id) = args.get("move_to_team_id").and_then(|v| v.as_str()) {
        return Ok(Some(id.to_string()));
    }
    if let Some(key) = args.get("move_to_team_key").and_then(|v| v.as_str()) {
        let team = team_service::get_team_by_key(db, workspace_id, key)
            .await?
            .ok_or_else(|| trakkt_core::Error::BadRequest(
                format!("No team found with key '{key}'"),
            ))?;
        return Ok(Some(team.team_id));
    }
    Ok(None)
}

/// update_issue — update fields on an existing issue.
async fn tool_update_issue(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let (team_key, number) = resolve_issue_key_and_number(args)?;

    // Resolve "move to team" separately from the identifying team_key.
    let move_team_id = resolve_move_team_id(args, &state.db, &auth.workspace_id).await?;

    // Build the IssueUpdate from provided fields. Absent keys mean "no change".
    // JSON null means "clear the field" (maps to Some(None) for double-Option fields).
    let updates = IssueUpdate {
        title: args.get("title").and_then(|v| v.as_str()).map(String::from),
        description: args.get("description").map(|v| {
            v.as_str().map(String::from)
        }),
        status_id: args.get("status_id").and_then(|v| v.as_str()).map(String::from),
        priority: args.get("priority").and_then(|v| v.as_i64()).map(|v| v as i32),
        assignee_id: args.get("assignee").map(|v| {
            v.as_str().map(String::from)
        }),
        due_date: args.get("due_date").map(|v| {
            v.as_str().map(String::from)
        }),
        project_id: args.get("project_id").map(|v| {
            v.as_str().map(String::from)
        }),
        milestone_id: args.get("milestone_id").map(|v| {
            v.as_str().map(String::from)
        }),
        parent_issue_id: args.get("parent_issue_id").map(|v| {
            v.as_str().map(String::from)
        }),
        sort_order: args.get("sort_order").map(|v| {
            v.as_f64()
        }),
        team_id: move_team_id,
    };

    let issue = issue_service::update_issue(
        &state.db,
        &auth.workspace_id,
        &team_key,
        number,
        &updates,
        Some(&auth.user_id),
        Some(&state.ws_manager),
    )
    .await?;

    // If labels were provided, replace them on the issue.
    if let Some(label_values) = args.get("labels").and_then(|v| v.as_array()) {
        let label_ids: Vec<String> = label_values
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        issue_service::set_issue_labels(
            &state.db,
            &issue.issue_id,
            &label_ids,
            Some(&state.ws_manager),
        )
        .await?;
    }

    // Re-fetch with full details after label update.
    // Use get_issue_by_id since the team/number may have changed on team reassignment.
    let updated = issue_service::get_issue_by_id(&state.db, &issue.issue_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("issue {team_key}-{number} not found")))?;

    serde_json::to_string_pretty(&updated).map_err(trakkt_core::Error::from)
}

/// delete_issue — permanently delete an issue.
async fn tool_delete_issue(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let (team_key, number) = resolve_issue_key_and_number(args)?;

    issue_service::delete_issue(
        &state.db,
        &auth.workspace_id,
        &team_key,
        number,
        Some(&state.ws_manager),
    )
    .await?;

    Ok(format!("Issue {team_key}-{number} deleted"))
}

/// add_comment — add a comment to an issue.
async fn tool_add_comment(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let (team_key, number) = resolve_issue_key_and_number(args)?;
    let body = arg_str(args, "body")?;

    // Resolve issue_id from team-scoped identifier.
    let issue = issue_service::get_issue(&state.db, &auth.workspace_id, &team_key, number)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("issue {team_key}-{number} not found")))?;

    let comment = comment_service::create_comment(
        &state.db,
        &issue.issue_id,
        &auth.user_id,
        body,
        None, // no parent — top-level comment
        Some(&state.ws_manager),
    )
    .await?;

    serde_json::to_string_pretty(&comment).map_err(trakkt_core::Error::from)
}

/// list_labels — list all labels in the workspace.
async fn tool_list_labels(
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let labels = label_service::list_labels(&state.db, &auth.workspace_id).await?;
    serde_json::to_string_pretty(&labels).map_err(trakkt_core::Error::from)
}

/// list_teams — list all teams in the workspace.
async fn tool_list_teams(
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let teams = team_service::list_teams(&state.db, &auth.workspace_id).await?;
    serde_json::to_string_pretty(&teams).map_err(trakkt_core::Error::from)
}

/// create_label — create a new label.
async fn tool_create_label(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let name = arg_str(args, "name")?;
    let color = arg_str(args, "color")?;

    let label = label_service::create_label(
        &state.db,
        &auth.workspace_id,
        name,
        color,
        None, // team_id — MCP creates workspace-scoped labels
        Some(&state.ws_manager),
    )
    .await?;

    serde_json::to_string_pretty(&label).map_err(trakkt_core::Error::from)
}

/// search_issues — search issues by title text.
async fn tool_search_issues(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let query = arg_str(args, "query")?;
    let limit_raw = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
    let limit = limit_raw.clamp(1, 100);
    let include_closed = args.get("include_closed").and_then(|v| v.as_bool()).unwrap_or(false);

    let team_id = resolve_team_id_from_args(args, &state.db, &auth.workspace_id).await?;

    let exclude_status_categories = if !include_closed {
        Some(vec!["completed".to_string(), "cancelled".to_string()])
    } else {
        None
    };

    let filters = IssueFilters {
        search: Some(query.to_string()),
        exclude_status_categories,
        limit: Some(limit),
        ..Default::default()
    };

    let issues = issue_service::list_issues(&state.db, &auth.workspace_id, team_id.as_deref(), &filters).await?;
    serde_json::to_string_pretty(&issues).map_err(trakkt_core::Error::from)
}

/// list_statuses — list all statuses in the workspace.
async fn tool_list_statuses(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let team_id = resolve_team_id_from_args(args, &state.db, &auth.workspace_id).await?;
    let statuses = status_service::list_statuses(&state.db, &auth.workspace_id, team_id.as_deref()).await?;
    serde_json::to_string_pretty(&statuses).map_err(trakkt_core::Error::from)
}

/// add_relation — create a relation between two issues.
async fn tool_add_relation(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let source = arg_str(args, "source_issue")?;
    let target = arg_str(args, "target_issue")?;
    let relation_type = arg_str(args, "relation_type")?;

    // Resolve source issue identifier to issue_id.
    let (source_key, source_num) = parse_issue_identifier(source)
        .ok_or_else(|| trakkt_core::Error::BadRequest(
            "Invalid source_issue format. Expected 'TRA-35'".to_string(),
        ))?;
    let source_issue = issue_service::get_issue(
        &state.db, &auth.workspace_id, &source_key, source_num,
    ).await?
        .ok_or_else(|| trakkt_core::Error::NotFound(
            format!("Source issue {source} not found"),
        ))?;

    // Resolve target issue identifier to issue_id.
    let (target_key, target_num) = parse_issue_identifier(target)
        .ok_or_else(|| trakkt_core::Error::BadRequest(
            "Invalid target_issue format. Expected 'TRA-35'".to_string(),
        ))?;
    let target_issue = issue_service::get_issue(
        &state.db, &auth.workspace_id, &target_key, target_num,
    ).await?
        .ok_or_else(|| trakkt_core::Error::NotFound(
            format!("Target issue {target} not found"),
        ))?;

    let relation = relation_service::create_relation(
        &state.db,
        &auth.workspace_id,
        &source_issue.issue_id,
        &target_issue.issue_id,
        relation_type,
        Some(&auth.user_id),
        Some(&state.ws_manager),
    ).await?;

    serde_json::to_string_pretty(&relation).map_err(trakkt_core::Error::from)
}

/// remove_relation — delete a relation by its ID.
async fn tool_remove_relation(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let relation_id = arg_str(args, "relation_id")?;

    relation_service::delete_relation(
        &state.db,
        relation_id,
        &auth.workspace_id,
        Some(&state.ws_manager),
    ).await?;

    Ok("Relation removed successfully".to_string())
}

/// list_issue_relations — list all relations for an issue (both directions).
async fn tool_list_issue_relations(
    args: &Value,
    auth: &McpAuth,
    state: &AppState,
) -> trakkt_core::Result<String> {
    let (team_key, number) = resolve_issue_key_and_number(args)?;

    let issue = issue_service::get_issue(
        &state.db, &auth.workspace_id, &team_key, number,
    ).await?
        .ok_or_else(|| trakkt_core::Error::NotFound(
            format!("Issue {team_key}-{number} not found"),
        ))?;

    let relations = relation_service::list_relations_for_issue(
        &state.db,
        &issue.issue_id,
        &auth.workspace_id,
    ).await?;

    serde_json::to_string_pretty(&relations).map_err(trakkt_core::Error::from)
}
