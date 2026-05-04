// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tane Rust backend — entry point.

use tane_core::Config;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

fn main() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(serve());
}

async fn serve() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();

    let config = Config::from_env();
    let port = config.port;

    // Connect to database and run migrations
    let db = tane_core::db::DbPool::connect(&config.database_url)
        .await
        .expect("failed to connect to database");

    // Personal mode: auto-provision local user and workspace on first boot
    if config.is_personal() {
        tane_server::auto_provision_personal_mode(&db)
            .await
            .expect("failed to auto-provision personal mode");
    }

    // KVStore
    let redis_url = config.redis_url.clone();
    let kv = tane_core::create_kv_store(redis_url.as_deref())
        .await
        .expect("failed to initialise KV store");

    // Raw Redis pool (optional)
    let redis: Option<tane_core::RedisPool> = if let Some(ref url) = redis_url {
        match tane_core::redis::create_pool(url).await {
            Ok(pool) => Some(pool),
            Err(e) => {
                tracing::error!(error = %e, "Failed to connect to Redis");
                std::process::exit(1);
            }
        }
    } else {
        tracing::info!("REDIS_URL not set — running in single-instance mode");
        None
    };

    // WebAuthn
    let rp_origin = url::Url::parse(&config.frontend_url)
        .expect("FRONTEND_URL must be a valid URL");
    let webauthn = match tane_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &rp_origin,
    ) {
        Ok(w) => w,
        Err(e) if config.self_hosted => {
            tracing::warn!("WebAuthn unavailable ({e}) — passkeys disabled.");
            let localhost_origin = url::Url::parse("http://localhost").expect("hardcoded");
            tane_auth::webauthn::build_webauthn("localhost", &config.webauthn_rp_name, &localhost_origin)
                .expect("fallback WebAuthn should succeed")
        }
        Err(e) => panic!("failed to build WebAuthn: {e}"),
    };

    // Encryption key (stub — derive from config.encryption_key)
    let encryption_key: [u8; 32] = {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let bytes = STANDARD.decode(&config.encryption_key)
            .expect("ENCRYPTION_KEY must be valid base64");
        let mut key = [0u8; 32];
        let len = bytes.len().min(32);
        key[..len].copy_from_slice(&bytes[..len]);
        key
    };

    // WebSocket manager
    let ws_redis = redis.as_ref().map(|pool| (pool.clone(), redis_url.clone().expect("redis pool implies url")));
    let ws_manager = tane_auth::websocket::WebSocketManager::new(ws_redis, db.clone());

    // MCP session manager
    let mcp_sessions = tane_auth::mcp_session_manager::MCPSessionManager::new(kv.clone());

    let state = tane_server::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis,
        config: Arc::new(config),
        encryption_key: Arc::new(encryption_key),
        webauthn: Arc::new(webauthn),
        ws_manager,
        mcp_sessions,
    };

    // Register Leptos server functions
    tane_ui::register_server_functions();

    let router = tane_server::build_router(state);
    let app = tane_server::wrap_service(router);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");

    eprintln!();
    eprintln!("  Tane Issue Tracker");
    eprintln!("  URL: http://localhost:{port}");
    eprintln!();

    tracing::info!("Tane listening on port {port}");

    axum::serve(listener, app)
        .await
        .expect("server error");
}
