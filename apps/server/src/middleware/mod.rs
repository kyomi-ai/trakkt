// SPDX-License-Identifier: AGPL-3.0-or-later

//! Middleware stack — CORS, security headers, transparent access-token auto-refresh.

pub mod auth_refresh;

pub use auth_refresh::auth_refresh_middleware;

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request},
    middleware::Next,
    response::Response,
};
use tower_http::cors::{AllowHeaders, CorsLayer};

/// Build the CORS layer.
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers(AllowHeaders::mirror_request())
        .allow_credentials(false)
}

/// Security headers middleware.
pub async fn security_headers(
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-xss-protection"),
        HeaderValue::from_static("1; mode=block"),
    );

    response
}
