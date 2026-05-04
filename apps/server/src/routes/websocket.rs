// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket endpoints for real-time communication.
//!
//! TODO: port WebSocket handler from Kyomi

use axum::{
    extract::{Path, State, WebSocketUpgrade},
    response::IntoResponse,
};

use crate::state::AppState;

/// WebSocket upgrade handler.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(_user_id): Path<String>,
    State(_state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|_socket| async {
        // TODO: port WebSocket handler from Kyomi
        tracing::info!("WebSocket connection — handler not yet ported");
    })
}
