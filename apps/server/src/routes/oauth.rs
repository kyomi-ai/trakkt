// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuth 2.0 endpoints for MCP client authentication.
//!
//! TODO: implement OAuth routes

use axum::Router;

use crate::state::AppState;

/// OAuth routes mounted at `/api/v1/oauth`.
pub fn routes() -> Router<AppState> {
    Router::new()
}

/// Well-known discovery routes at root level.
pub fn well_known_routes() -> Router<AppState> {
    Router::new()
}
