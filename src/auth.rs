use crate::api::ErrorResponse;
use axum::{
    body::Body,
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;
use tracing::warn;

/// Middleware validating incoming API calls against an expected Bearer API Key in `Authorization: Bearer <api-key>`.
pub async fn validate_api_key(
    axum::extract::State(expected_key): axum::extract::State<Arc<String>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(auth_header) = req.headers().get(AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if token == expected_key.as_str() {
                    return next.run(req).await;
                }
            }
        }
    }

    warn!("Blocked an unauthorized API call missing or carrying an incorrect Authorization Bearer token.");
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse::new(
            "UNAUTHORIZED",
            "Required authentication is missing or invalid.",
        )),
    )
        .into_response()
}
