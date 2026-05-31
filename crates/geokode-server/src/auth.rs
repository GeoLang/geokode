//! JWT authentication middleware.

use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};

/// JWT claims.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub role: String,
}

/// Auth middleware — extracts and validates JWT from Authorization header.
/// If `GEOKODE_JWT_SECRET` is not set, auth is disabled (open access).
pub async fn auth_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    let secret = std::env::var("GEOKODE_JWT_SECRET").ok();

    // If no secret configured, allow all requests (development mode)
    let Some(secret) = secret else {
        return Ok(next.run(request).await);
    };

    // Health and metrics endpoints are always public
    if request.uri().path() == "/health"
        || request.uri().path() == "/healthz"
        || request.uri().path() == "/readyz"
        || request.uri().path() == "/metrics"
    {
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let _claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(next.run(request).await)
}
