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
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use base64::Engine;
use serde_json::json;

use trakkt_api::{activities, attachments, comments, issues, labels, milestones, projects, relations, statuses, teams, ApiCtx, ApiError};
use crate::state::AppState;

use super::auth_shared::{self, ResolvedAuth};

// ─────────────────────────────────────────────────────────────────────────────
// Auth — thin wrapper around shared auth module
// ─────────────────────────────────────────────────────────────────────────────

/// Authenticate the request, returning `ResolvedAuth` or an HTTP 401 error.
///
/// In personal mode, returns a local user context without token validation.
async fn authenticate(headers: &HeaderMap, state: &AppState) -> Result<ResolvedAuth, RestError> {
    if state.config.is_personal() {
        return Ok(ResolvedAuth {
            workspace_id: "workspace-local".to_string(),
            user_id: "user-local".to_string(),
            scopes: vec![],
        });
    }

    auth_shared::resolve_auth(headers, state).await.ok_or_else(|| {
        RestError(ApiError::Unauthorized(
            "Authentication required. Provide a valid Bearer token in the Authorization header."
                .to_string(),
        ))
    })
}

/// Check that the resolved auth has the required scope.
fn check_scope(auth: &ResolvedAuth, scope: &str) -> Result<(), RestError> {
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
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
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
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
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
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
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
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
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
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
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
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::DeleteIssueApiParams {
        issue_identifier: Some(identifier),
        team_key: None,
        issue_number: None,
    };
    let result = issues::delete_issue(&ctx, params).await?;
    Ok(Json(result))
}

// ─── Comments ────────────────────────────────────────────────────────────────

async fn add_comment_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(mut params): Json<trakkt_types::api::AddCommentApiParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "comments:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    params.issue_identifier = Some(identifier);
    let result = comments::add_comment(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// ─── Labels ──────────────────────────────────────────────────────────────────

async fn list_labels_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "labels:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let result = labels::list_labels(&ctx, trakkt_types::api::ListLabelsApiParams {}).await?;
    Ok(Json(result))
}

async fn create_label_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<trakkt_types::api::CreateLabelApiParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "labels:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let result = labels::create_label(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// ─── Teams ───────────────────────────────────────────────────────────────────

async fn list_teams_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "teams:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let result = teams::list_teams(&ctx, trakkt_types::api::ListTeamsApiParams {}).await?;
    Ok(Json(result))
}

// ─── Statuses ────────────────────────────────────────────────────────────────

async fn list_statuses_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<trakkt_types::api::ListStatusesApiParams>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let result = statuses::list_statuses(&ctx, params).await?;
    Ok(Json(result))
}

// ─── Relations ───────────────────────────────────────────────────────────────

async fn add_relation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(mut params): Json<trakkt_types::api::AddRelationApiParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    params.source_issue = Some(identifier);
    let result = relations::add_relation(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn list_relations_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::ListRelationsApiParams {
        issue_identifier: Some(identifier),
        team_key: None,
        issue_number: None,
    };
    let result = relations::list_relations(&ctx, params).await?;
    Ok(Json(result))
}

async fn remove_relation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::RemoveRelationApiParams { relation_id: id };
    let result = relations::remove_relation(&ctx, params).await?;
    Ok(Json(result))
}

// ─── Activities ─────────────────────────────────────────────────────────────

async fn list_issue_activities_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(identifier): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "issues:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::ListIssueActivitiesApiParams {
        issue_identifier: Some(identifier),
        team_key: None,
        issue_number: None,
    };
    let result = activities::list_issue_activities(&ctx, params).await?;
    Ok(Json(result))
}

// ─── Projects ────────────────────────────────────────────────────────────────

async fn list_projects_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let result = projects::list_projects(&ctx, trakkt_types::api::ListProjectsApiParams {}).await?;
    Ok(Json(result))
}

async fn get_project_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::GetProjectApiParams { project_id: id };
    let result = projects::get_project(&ctx, params).await?;
    Ok(Json(result))
}

async fn create_project_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<trakkt_types::api::CreateProjectApiParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let result = projects::create_project(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn update_project_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut params): Json<trakkt_types::api::UpdateProjectApiParams>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    params.project_id = Some(id);
    let result = projects::update_project(&ctx, params).await?;
    Ok(Json(result))
}

async fn delete_project_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::DeleteProjectApiParams { project_id: id };
    let result = projects::delete_project(&ctx, params).await?;
    Ok(Json(result))
}

// ─── Milestones ──────────────────────────────────────────────────────────────

async fn list_milestones_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:read")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::ListMilestonesApiParams { project_id: id };
    let result = milestones::list_milestones(&ctx, params).await?;
    Ok(Json(result))
}

async fn create_milestone_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut params): Json<trakkt_types::api::CreateMilestoneApiParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    params.project_id = Some(id);
    let result = milestones::create_milestone(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn update_milestone_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut params): Json<trakkt_types::api::UpdateMilestoneApiParams>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    params.milestone_id = Some(id);
    let result = milestones::update_milestone(&ctx, params).await?;
    Ok(Json(result))
}

async fn delete_milestone_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "projects:write")?;
    let ctx = ApiCtx::from_bearer(auth.workspace_id, auth.user_id, &state.db, &state.ws_manager, &*state.attachment_storage);
    let params = trakkt_types::api::DeleteMilestoneApiParams { milestone_id: id };
    let result = milestones::delete_milestone(&ctx, params).await?;
    Ok(Json(result))
}

// ─── Attachments ────────────────────────────────────────────────────────────

/// `POST /attachments` — upload a file via multipart form.
///
/// Accepts a multipart form with a "file" field. The handler extracts the file,
/// base64-encodes it, and delegates to the shared upload handler.
async fn upload_attachment_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "attachments:write")?;

    const MAX_ATTACHMENT_SIZE: usize = 10 * 1024 * 1024;

    // Extract the file field from multipart with streaming size check
    let mut file_data: Option<(String, String, Vec<u8>)> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        RestError(ApiError::BadRequest(format!("Multipart parse error: {e}")))
    })? {
        if field.name() == Some("file") {
            let filename = field
                .file_name()
                .unwrap_or("unnamed")
                .to_string();
            let content_type = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();

            let mut buf = Vec::with_capacity(4096);
            let mut stream = field;
            while let Some(chunk) = stream.chunk().await.map_err(|e| {
                RestError(ApiError::BadRequest(format!("Failed to read file field: {e}")))
            })? {
                if buf.len() + chunk.len() > MAX_ATTACHMENT_SIZE {
                    return Err(RestError(ApiError::BadRequest(format!(
                        "File too large (max {} bytes)",
                        MAX_ATTACHMENT_SIZE
                    ))));
                }
                buf.extend_from_slice(&chunk);
            }

            file_data = Some((filename, content_type, buf));
            break;
        }
    }

    let (filename, content_type, bytes) = file_data.ok_or_else(|| {
        RestError(ApiError::BadRequest(
            "Missing 'file' field in multipart form".into(),
        ))
    })?;

    // Base64-encode and build the params
    let content_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let params = trakkt_types::api::UploadAttachmentApiParams {
        content_base64,
        filename,
        content_type,
    };

    let ctx = ApiCtx::from_bearer(
        auth.workspace_id,
        auth.user_id,
        &state.db,
        &state.ws_manager,
        &*state.attachment_storage,
    );
    let result = attachments::upload_attachment(&ctx, params).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// `GET /attachments/{attachment_id}/download` — download a file as raw bytes.
///
/// Returns the file content with appropriate Content-Type and Content-Disposition headers.
async fn download_attachment_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(attachment_id): Path<String>,
) -> Result<Response, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "attachments:read")?;

    let ctx = ApiCtx::from_bearer(
        auth.workspace_id,
        auth.user_id,
        &state.db,
        &state.ws_manager,
        &*state.attachment_storage,
    );
    let params = trakkt_types::api::DownloadAttachmentApiParams { attachment_id };
    let result = attachments::download_attachment(&ctx, params).await?;

    // Extract fields from the handler's JSON response
    let content_type = result
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream");
    let filename = result
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("download");
    let content_base64 = result
        .get("content_base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RestError(ApiError::Internal("Missing content_base64 in response".into())))?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_base64)
        .map_err(|e| RestError(ApiError::Internal(format!("Failed to decode content: {e}"))))?;

    let ascii_safe = filename.replace('\\', "\\\\").replace('"', "\\\"");
    let encoded = percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC);
    let disposition = format!("inline; filename=\"{ascii_safe}\"; filename*=UTF-8''{encoded}");

    let response = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type)
        .header("content-disposition", disposition)
        .header("content-length", bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|e| RestError(ApiError::Internal(format!("Failed to build response: {e}"))))?;

    Ok(response)
}

/// `DELETE /attachments/{attachment_id}` — delete an attachment.
async fn delete_attachment_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(attachment_id): Path<String>,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "attachments:write")?;
    let ctx = ApiCtx::from_bearer(
        auth.workspace_id,
        auth.user_id,
        &state.db,
        &state.ws_manager,
        &*state.attachment_storage,
    );
    let params = trakkt_types::api::DeleteAttachmentApiParams { attachment_id };
    let result = attachments::delete_attachment(&ctx, params).await?;
    Ok(Json(result))
}

/// `GET /attachments` — list all attachments in the workspace.
async fn list_attachments_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RestError> {
    let auth = authenticate(&headers, &state).await?;
    check_scope(&auth, "attachments:read")?;
    let ctx = ApiCtx::from_bearer(
        auth.workspace_id,
        auth.user_id,
        &state.db,
        &state.ws_manager,
        &*state.attachment_storage,
    );
    let result = attachments::list_attachments(
        &ctx,
        trakkt_types::api::ListAttachmentsApiParams {},
    )
    .await?;
    Ok(Json(result))
}

// ─────────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /openapi.json` — OpenAPI 3.1 spec (unauthenticated).
async fn openapi_handler() -> Json<serde_json::Value> {
    Json(trakkt_api::openapi::generate_openapi_spec())
}

/// Build the REST API router, mounted at `/api/v1`.
pub fn rest_router() -> Router<AppState> {
    Router::new()
        // OpenAPI spec (public, no auth)
        .route("/openapi.json", get(openapi_handler))
        // Issues
        .route("/issues", get(list_issues_handler).post(create_issue_handler))
        .route("/issues/search", get(search_issues_handler))
        .route(
            "/issues/{identifier}",
            get(get_issue_handler)
                .patch(update_issue_handler)
                .delete(delete_issue_handler),
        )
        // Comments
        .route("/issues/{identifier}/comments", post(add_comment_handler))
        // Relations
        .route(
            "/issues/{identifier}/relations",
            get(list_relations_handler).post(add_relation_handler),
        )
        .route("/relations/{id}", delete(remove_relation_handler))
        // Activities
        .route("/issues/{identifier}/activities", get(list_issue_activities_handler))
        // Labels
        .route("/labels", get(list_labels_handler).post(create_label_handler))
        // Teams
        .route("/teams", get(list_teams_handler))
        // Statuses
        .route("/statuses", get(list_statuses_handler))
        // Projects
        .route("/projects", get(list_projects_handler).post(create_project_handler))
        .route(
            "/projects/{id}",
            get(get_project_handler)
                .patch(update_project_handler)
                .delete(delete_project_handler),
        )
        // Milestones
        .route(
            "/projects/{id}/milestones",
            get(list_milestones_handler).post(create_milestone_handler),
        )
        .route(
            "/milestones/{id}",
            patch(update_milestone_handler).delete(delete_milestone_handler),
        )
        // Attachments
        .route("/attachments", get(list_attachments_handler).post(upload_attachment_handler))
        .route("/attachments/{attachment_id}/download", get(download_attachment_handler))
        .route("/attachments/{attachment_id}", delete(delete_attachment_handler))
}
