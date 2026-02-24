/// Bearer token authentication middleware for the admin API.
///
/// When an API token is configured (via --api-token), all /api/* requests
/// must include `Authorization: Bearer <token>`. Static files and WebSocket
/// upgrades are exempt so the dashboard remains accessible.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::Response,
};

/// Shared token state — None means auth is disabled (open access).
#[derive(Clone)]
pub struct ApiToken(pub Option<String>);

/// Axum middleware: reject /api/* requests without a valid bearer token.
pub async fn require_auth(
    token: axum::extract::Extension<ApiToken>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected = &token.0;

    // If no token configured, allow everything
    let expected = match expected {
        Some(t) => t,
        None => return Ok(next.run(req).await),
    };

    let path = req.uri().path();

    // Static files and WebSocket upgrades are exempt
    if !path.starts_with("/api/") {
        return Ok(next.run(req).await);
    }

    // Check Authorization header
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.starts_with("Bearer ") => {
            let provided = &value[7..];
            if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
                Ok(next.run(req).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Constant-time comparison to prevent timing attacks on the token.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
