// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team icon endpoints — upload, serve, and delete custom team icons.
//!
//! - `POST   /api/teams/:team_id/icon` — upload a custom icon (multipart)
//! - `GET    /api/teams/:team_id/icon` — serve the stored icon bytes
//! - `DELETE /api/teams/:team_id/icon` — remove the custom icon

use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};

use trakkt_auth::middleware::AuthUser;
use trakkt_auth::team_service;

use crate::state::AppState;

/// Maximum allowed icon file size: 50 KB.
const MAX_ICON_SIZE: usize = 51_200;

fn validate_magic_bytes(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"<?xml") || data.starts_with(b"<svg") {
        Some("image/svg+xml")
    } else {
        None
    }
}

/// Build the team-icon sub-router mounted at `/api/teams`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{team_id}/icon", get(get_icon))
        .route("/{team_id}/icon", post(upload_icon))
        .route("/{team_id}/icon", delete(delete_icon))
}

// ---------------------------------------------------------------------------
// POST /api/teams/:team_id/icon
// ---------------------------------------------------------------------------

async fn upload_icon(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(team_id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, trakkt_core::Error> {
    // Resolve the team to get workspace_id and verify it exists.
    let team = team_service::get_team(&state.db, &team_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("team {team_id} not found")))?;

    // Verify the user belongs to the team's workspace.
    if auth.workspace.workspace_id.as_deref() != Some(&team.workspace_id) {
        return Err(trakkt_core::Error::Forbidden(
            "not a member of this workspace".into(),
        ));
    }

    // Read the "icon" field from the multipart form with streaming size limit.
    let mut icon_data: Option<(Vec<u8>, String)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| trakkt_core::Error::BadRequest(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name != "icon" {
            continue;
        }

        // Stream chunks with a running size check to reject oversized uploads early.
        let mut buf = Vec::with_capacity(MAX_ICON_SIZE);
        let mut stream = field;
        while let Some(chunk) = stream
            .chunk()
            .await
            .map_err(|e| trakkt_core::Error::BadRequest(format!("failed to read file: {e}")))?
        {
            if buf.len() + chunk.len() > MAX_ICON_SIZE {
                return Err(trakkt_core::Error::BadRequest(format!(
                    "file too large (max {} bytes)",
                    MAX_ICON_SIZE
                )));
            }
            buf.extend_from_slice(&chunk);
        }

        // Validate actual file content via magic bytes — don't trust the client-declared MIME.
        let mime = validate_magic_bytes(&buf).ok_or_else(|| {
            trakkt_core::Error::BadRequest(
                "unrecognized file format. Allowed: SVG, PNG, JPEG".into(),
            )
        })?;

        icon_data = Some((buf, mime.to_string()));
        break;
    }

    let (data, mime) = icon_data
        .ok_or_else(|| trakkt_core::Error::BadRequest("missing 'icon' field in form".into()))?;

    let updated_team = team_service::upload_team_icon(
        &state.db,
        &team_id,
        &team.workspace_id,
        &data,
        &mime,
        Some(&state.ws_manager),
    )
    .await?;

    Ok(Json(updated_team))
}

// ---------------------------------------------------------------------------
// GET /api/teams/:team_id/icon
// ---------------------------------------------------------------------------

async fn get_icon(
    State(state): State<AppState>,
    Path(team_id): Path<String>,
) -> Result<Response, trakkt_core::Error> {
    let icon = team_service::get_team_icon_data(&state.db, &team_id).await?;

    match icon {
        Some((data, mime)) => {
            let mut headers = HeaderMap::new();
            if let Ok(val) = HeaderValue::from_str(&mime) {
                headers.insert("content-type", val);
            }
            headers.insert(
                "cache-control",
                HeaderValue::from_static("public, max-age=3600"),
            );
            Ok((StatusCode::OK, headers, data).into_response())
        }
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

// ---------------------------------------------------------------------------
// DELETE /api/teams/:team_id/icon
// ---------------------------------------------------------------------------

async fn delete_icon(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(team_id): Path<String>,
) -> Result<impl IntoResponse, trakkt_core::Error> {
    // Resolve the team to get workspace_id.
    let team = team_service::get_team(&state.db, &team_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("team {team_id} not found")))?;

    // Verify workspace membership.
    if auth.workspace.workspace_id.as_deref() != Some(&team.workspace_id) {
        return Err(trakkt_core::Error::Forbidden(
            "not a member of this workspace".into(),
        ));
    }

    team_service::delete_team_icon(
        &state.db,
        &team_id,
        &team.workspace_id,
        Some(&state.ws_manager),
    )
    .await?;

    Ok(StatusCode::OK)
}
