// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team icon endpoints — upload, serve, and delete custom team icons.
//!
//! Mounted at `/api/v1/teams` by `build_router` (`apps/server/src/lib.rs:81`):
//!
//! - `POST   /api/v1/teams/{team_id}/icon` — upload a custom icon (multipart)
//! - `GET    /api/v1/teams/{team_id}/icon` — serve the stored icon bytes
//! - `DELETE /api/v1/teams/{team_id}/icon` — remove the custom icon
//!
//! All three require an authenticated caller who belongs to the team's
//! workspace, and all three scope their database access by `workspace_id`.

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

/// Build the team-icon sub-router. Mounted at `/api/v1/teams` by `build_router`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{team_id}/icon", get(get_icon))
        .route("/{team_id}/icon", post(upload_icon))
        .route("/{team_id}/icon", delete(delete_icon))
}

// ---------------------------------------------------------------------------
// POST /api/v1/teams/{team_id}/icon
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
// GET /api/v1/teams/{team_id}/icon
// ---------------------------------------------------------------------------

/// Serve a team's uploaded icon bytes.
///
/// # Why this endpoint requires authentication
///
/// This read is authenticated and workspace-scoped, structurally identical to
/// `upload_icon` and `delete_icon` above. It previously took no `AuthUser` and
/// passed no workspace id, so anyone holding a team id could fetch that team's
/// uploaded icon bytes.
///
/// Requiring auth does not break the UI: the only consumer renders this as a
/// *same-origin* `<img src="/api/v1/teams/{team_id}/icon">`
/// (`crates/trakkt-ui/src/components/team_icon.rs:163`), so the browser attaches
/// the `access_token` cookie to the image request on its own, and `AuthUser`
/// falls back from the `Authorization: Bearer` header to exactly that cookie
/// (`extract_token`, `crates/trakkt-auth/src/middleware.rs:192-206`).
///
/// The two checks below are both load-bearing and neither subsumes the other:
/// the `AuthUser` extractor proves the caller has *a* valid session, while the
/// workspace comparison proves it is *this team's* workspace. Passing
/// `team.workspace_id` down to `get_team_icon_data` then re-scopes the SQL
/// itself, so the query cannot silently widen back to an unscoped lookup if this
/// handler is edited later.
async fn get_icon(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(team_id): Path<String>,
) -> Result<Response, trakkt_core::Error> {
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

    let icon = team_service::get_team_icon_data(&state.db, &team_id, &team.workspace_id).await?;

    match icon {
        Some((data, mime)) => {
            let mut headers = HeaderMap::new();
            match HeaderValue::from_str(&mime) {
                Ok(val) => {
                    headers.insert("content-type", val);
                }
                // `team_service::upload_team_icon` is the only writer that stores a
                // non-NULL `icon_mime` (the other icon mutations set it to NULL),
                // and its only non-test caller is `upload_icon` above, which takes
                // the value from `validate_magic_bytes` — so every MIME this row
                // can hold today is a valid header value. Reaching this branch
                // therefore means unexpected data, and the response would fall back
                // to browser content-type sniffing, so say so rather than drop it.
                Err(e) => {
                    tracing::warn!(
                        "team {team_id} has an unusable icon_mime {mime:?}, \
                         serving icon without content-type: {e}"
                    );
                }
            }
            // `private`, not `public`: the response is user-scoped now that auth is
            // required, and `public` would let a shared or intermediary cache store
            // one workspace's icon and hand it to a user from another workspace —
            // moving the leak this endpoint just closed somewhere far less visible.
            //
            // max-age=3600: a team's icon only changes through an explicit admin
            // action (`upload_icon` and `delete_icon` here, or the preset picker via
            // `team_service::update_team_icon`), so staleness is rare in practice.
            // Because the cache is now per-browser, the cost of a stale entry is
            // bounded to one user briefly seeing their own workspace's previous icon
            // — not a cross-tenant disclosure.
            headers.insert(
                "cache-control",
                HeaderValue::from_static("private, max-age=3600"),
            );
            Ok((StatusCode::OK, headers, data).into_response())
        }
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

// ---------------------------------------------------------------------------
// DELETE /api/v1/teams/{team_id}/icon
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
