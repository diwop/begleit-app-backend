use axum::{body::Body, extract::Request, http::StatusCode, middleware::Next, response::Response};
use std::sync::Arc;
use tracing::warn;

/// Middleware mapping incoming API calls to an expected API Key present in `X-API-KEY`.
pub async fn validate_api_key(
    axum::extract::State(expected_key): axum::extract::State<Arc<String>>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(key) = req.headers().get("x-api-key") {
        if let Ok(key_str) = key.to_str() {
            if key_str == expected_key.as_str() {
                return Ok(next.run(req).await);
            }
        }
    }

    warn!("Blocked an unauthorized API call missing or carrying an incorrect X-API-KEY header.");
    Err(StatusCode::UNAUTHORIZED)
}
