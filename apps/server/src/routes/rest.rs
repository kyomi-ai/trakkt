// SPDX-License-Identifier: AGPL-3.0-or-later

//! REST API surface — `/api/v1` routes.
//!
//! Each handler is a thin wrapper: authenticate, check scope, extract params,
//! call the shared API handler, return JSON. No business logic lives here.
//!
//! ## Authentication
//!
//! Two auth methods are supported (tried in order):
//!
//! 1. **JWT (OAuth 2.0)** — `Authorization: Bearer <jwt>`.
//! 2. **API token (legacy)** — `Authorization: Bearer <trakkt-...>`, SHA-256
//!    hash lookup against `api_tokens`.
//!
//! In personal mode, auth is bypassed — a local user context is injected.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::api::{issues, ApiCtx, ApiError};
use crate::state::AppState;

// ─────────────────────────────────────────────────────────────────────────────
// Auth — duplicated from routes/mcp.rs, will be deduplicated in Phase 4
// ─────────────────────────────────────────────────────────────────────────────

/// Resolved identity for REST requests.
///
/// Mirrors `McpAuth` in `routes/mcp.rs`. Phase 4 will extract a shared auth
/// module that both surfaces consume, eliminating this duplication.
struct RestAuth {
    workspace_id: String,
    user_id: String,
    scopes: Vec<String>,
}

impl RestAuth {
    fn has_scope(&self, scope: &str) -> bool {
        self.scopes.is_empty() || self.scopes.iter().any(|s| s == scope)
    }
}

/// Internal row type for the `api_tokens` lookup query.
///
/// Duplicated from `routes/mcp.rs` — will be deduplicated in Phase 4.
#[derive(sqlx::FromRow)]
struct ApiTokenLookupRow {
    user_id: String,
    workspace_id: Option<String>,
    scopes: Option<String>,
}

/// Try to authenticate via JWT (OAuth 2.0) or legacy API token.
///
/// Logic mirrors `resolve_mcp_auth` in `routes/mcp.rs`. Phase 4 will extract
/// this into a shared `api::auth` module.
async fn resolve_rest_auth(headers: &HeaderMap, state: &AppState) -> Option<RestAuth> {
    let auth_header = headers.get("authorization")?.to_str().ok()?;
    let token = auth_header.strip_prefix("Bearer ")?;

    // 1. Try JWT validation first (OAuth 2.0 path).
    if let Ok(decoded) = trakkt_auth::jwt::validate_token(token, &state.config.jwt_secret) {
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

        return Some(RestAuth {
            workspace_id,
            user_id,
            scopes: vec![], // JWT users have full access
        });
    }

    // 2. Legacy API token path (SHA-256 hash lookup).
    authenticate_bearer_token(token, &state.db).await
}

/// Validate a raw Bearer token against the `api_tokens` table (legacy path).
///
/// Mirrors `authenticate_bearer_token` in `routes/mcp.rs`.
async fn authenticate_bearer_token(
    token: &str,
    db: &trakkt_core::DbPool,
) -> Option<RestAuth> {
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

    Some(RestAuth {
        workspace_id,
        user_id: row.user_id,
        scopes,
    })
}

/// Authenticate the request, returning `RestAuth` or an HTTP 401 error.
///
/// In personal mode, returns a local user context without token validation.
async fn authenticate(headers: &HeaderMap, state: &AppState) -> Result<RestAuth, RestError> {
    if state.config.is_personal() {
        return Ok(RestAuth {
            workspace_id: "workspace-local".to_string(),
            user_id: "user-local".to_string(),
            scopes: vec![],
        });
    }

    resolve_rest_auth(headers, state).await.ok_or_else(|| {
        RestError(ApiError::Unauthorized(
            "Authentication required. Provide a valid Bearer token in the Authorization header."
                .to_string(),
        ))
    })
}

/// Check that the resolved auth has the required scope.
fn check_scope(auth: &RestAuth, scope: &str) -> Result<(), RestError> {
    if auth.has_scope(scope) {
        Ok(())
    } else {
        Err(RestError(ApiError::Forbidden(format!(
            "Missing required scope: {scope}"
        ))))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error mapping — ApiError → HTTP response
// ─────────────────────────────────────────────────────────────────────────────

/// Newtype wrapper that converts [`ApiError`] into an Axum response.
struct RestError(ApiError);

impl From<ApiError> for RestError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Route handlers — thin wrappers around shared API handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /issues` — list issues with optional query-string filters.
async fn list_issues_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<trakkt_types::api::ListIssuesApiParams>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager);
    let result = issues::list_issues(&ctx, params).await?;
    Ok(Json(result))
}

/// `GET /issues/search` — search issues by text query.
async fn search_issues_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<trakkt_types::api::SearchIssuesApiParams>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager);
    let result = issues::search_issues(&ctx, params).await?;
    Ok(Json(result))
}

/// `GET /issues/{identifier}` — get a single issue by team-scoped identifier.
async fn get_issue_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager);
    let params = trakkt_types::api::GetIssueApiParams {
        issue_identifier: Some(identifier),
        team_key: None,
        issue_number: None,
    };
    let result = issues::get_issue(&ctx, params).await?;
    Ok(Json(result))
}

/// `POST /issues` — create a new issue from JSON body.
async fn create_issue_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<trakkt_types::api::CreateIssueApiParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager);
    let result = issues::create_issue(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// `PATCH /issues/{identifier}` — update an existing issue from JSON body.
async fn update_issue_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(mut params): Json<trakkt_types::api::UpdateIssueApiParams>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager);
    params.issue_identifier = Some(identifier);
    let result = issues::update_issue(&ctx, params).await?;
    Ok(Json(result))
}

/// `DELETE /issues/{identifier}` — permanently delete an issue.
async fn delete_issue_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager);
    let params = trakkt_types::api::DeleteIssueApiParams {
        issue_identifier: Some(identifier),
        team_key: None,
        issue_number: None,
    };
    let result = issues::delete_issue(&ctx, params).await?;
    Ok(Json(result))
}

// ─────────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────────

/// Build the REST API router, mounted at `/api/v1`.
pub fn rest_router() -> Router<AppState> {
    Router::new()
        .route("/issues", get(list_issues_handler).post(create_issue_handler))
        .route("/issues/search", get(search_issues_handler))
        .route(
            "/issues/{identifier}",
            get(get_issue_handler)
                .patch(update_issue_handler)
                .delete(delete_issue_handler),
        )
}
