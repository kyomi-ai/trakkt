// SPDX-License-Identifier: AGPL-3.0-or-later

//! trakkt-server — Axum HTTP server for the Trakkt backend.

pub mod leptos_frontend;
pub mod middleware;
pub mod routes;
pub mod state;

use std::net::SocketAddr;

use axum::Router;
use tower::Layer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::trace::TraceLayer;

use trakkt_core::db::DbPool;
use trakkt_core::sql_compat;
use trakkt_core::{db_execute, db_fetch_scalar};

/// Auto-provision a local user and workspace for personal (desktop) mode.
pub async fn auto_provision_personal_mode(db: &DbPool) -> Result<(), trakkt_core::Error> {
    let is_pg = db.is_postgres();

    let user_count: i64 = db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM users")?;
    if user_count > 0 {
        return Ok(());
    }

    let now = sql_compat::now(is_pg);
    let bool_true = sql_compat::bool_true(is_pg);

    let user_sql = format!(
        "INSERT INTO users (user_id, email, name, verified, active, created_at, updated_at) \
         VALUES ($1, $2, $3, {bool_true}, {bool_true}, {now}, {now})"
    );
    db_execute!(db, &user_sql, "user-local", "local@localhost", "Local User")?;

    let workspace_sql = format!(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id, status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, {now}, {now})"
    );
    db_execute!(
        db, &workspace_sql,
        "workspace-local", "My Workspace", "user-local", "active"
    )?;

    let membership_sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
         VALUES ($1, $2, $3, {bool_true}, {now})"
    );
    db_execute!(db, &membership_sql, "workspace-local", "user-local", "workspace_admin")?;

    // Create default team for personal mode
    let team_sql = format!(
        "INSERT INTO teams (team_id, workspace_id, name, key, created_at) \
         VALUES ($1, $2, $3, $4, {now})"
    );
    db_execute!(db, &team_sql, "team-local", "workspace-local", "Default", "TRK")?;

    // Add user-local as team lead of the default team
    trakkt_auth::team_service::add_team_member(db, "team-local", "user-local", "lead", "workspace-local").await?;

    // Seed default statuses so issues can use status_id FK.
    trakkt_auth::status_service::seed_default_statuses(db, "workspace-local").await?;

    tracing::info!("Personal mode: auto-provisioned local user, workspace, and default team");
    Ok(())
}

/// Build the axum Router with all core routes and middleware.
pub fn build_router(state: state::AppState) -> Router {
    use axum::extract::FromRef;

    Router::new()
        .route("/api/health", axum::routing::get(health_handler))
        .route("/health", axum::routing::get(health_handler))
        .nest("/api/v1/auth", routes::auth_token::routes())
        .nest("/api/v1", routes::rest::rest_router())
        .nest("/api/v1/teams", routes::team_icon::routes())
        .nest("/webhooks", routes::billing::routes())
        .nest("/mcp", routes::mcp::routes())
        .merge(routes::oauth::well_known_routes())
        .nest("/api/v1/oauth", routes::oauth::routes())
        .route("/ws/{user_id}", axum::routing::get(routes::websocket::ws_handler))
        // Leptos server functions
        .route("/leptos-api/{*fn_name}", axum::routing::post({
            let server_ctx = trakkt_ui::server_fns::ServerContext {
                db: state.db.clone(),
                config: state.config.clone(),
                auth_state: trakkt_auth::middleware::AuthState::from_ref(&state),
                encryption_key: Some(state.encryption_key.clone()),
                kv: Some(state.kv.clone()),
                redis: state.redis.clone(),
                webauthn: Some(state.webauthn.clone()),
                ws_manager: Some(state.ws_manager.clone()),
                mcp_sessions: Some(state.mcp_sessions.clone()),
            };
            move |req: axum::http::Request<axum::body::Body>| {
                let ctx = server_ctx.clone();
                async move {
                    leptos_axum::handle_server_fns_with_context(
                        move || {
                            leptos::prelude::provide_context(ctx.clone());
                        },
                        req,
                    )
                    .await
                }
            }
        }))
        .route("/login", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .route("/signup", axum::routing::get(leptos_frontend::serve_leptos_shell))
        .fallback_service(
            leptos_frontend::static_files_service()
                .not_found_service(tower::service_fn(|_req| async {
                    Ok::<_, std::convert::Infallible>(leptos_frontend::serve().await)
                })),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_refresh_middleware,
        ))
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::security_headers))
        .layer(middleware::cors_layer())
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024))
}

async fn health_handler() -> &'static str {
    "ok"
}

/// Wrap a completed Router with path normalization and connect-info extraction.
pub fn wrap_service(
    router: Router,
) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
    tower_http::normalize_path::NormalizePath<Router>,
    SocketAddr,
> {
    let app = NormalizePathLayer::trim_trailing_slash().layer(router);
    axum::ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<SocketAddr>(app)
}
