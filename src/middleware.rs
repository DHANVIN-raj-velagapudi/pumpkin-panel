// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Cross-cutting HTTP concerns: throttling and response hardening.

use crate::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::net::SocketAddr;

/// Extracts the caller's address, honouring `X-Forwarded-For` only when the
/// operator has said the panel sits behind a trusted proxy. Believing that
/// header unconditionally would let anyone forge their way around the limiter.
pub fn client_key(request: &Request<axum::body::Body>, peer: SocketAddr, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(forwarded) = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return forwarded.to_string();
        }
    }
    peer.ip().to_string()
}

pub async fn rate_limit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // The console socket is long-lived and pushes many frames; limiting it by
    // request count would achieve nothing useful.
    if path.ends_with("/console/ws") {
        return next.run(request).await;
    }

    let key = client_key(&request, peer, state.trust_proxy);
    if !state.limiter.allow(&key) {
        tracing::warn!(client = %key, %path, "rate limited");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "10")],
            axum::Json(json!({ "error": "too many requests, slow down a moment" })),
        )
            .into_response();
    }

    next.run(request).await
}

/// Adds the standard hardening headers to every response.
pub async fn security_headers(request: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // The UI is entirely self-hosted, so everything can be locked to 'self'.
    // React sets element styles via the style attribute, which needs
    // 'unsafe-inline' for styles only, never for scripts.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; \
             script-src 'self'; \
             style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; \
             font-src 'self' data:; \
             connect-src 'self' ws: wss:; \
             object-src 'none'; \
             frame-ancestors 'none'; \
             base-uri 'self'; \
             form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );

    response
}
