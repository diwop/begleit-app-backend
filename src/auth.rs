use crate::api::ErrorResponse;
use crate::web::is_admin_authenticated;
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

/// Shared authentication state holding the expected API key and admin password.
#[derive(Clone)]
pub struct AuthState {
    /// Expected Bearer API key for standard API clients.
    pub api_key: Arc<String>,
    /// Configured admin password allowing admin Bearer tokens or `admin_session` cookies.
    pub admin_password: Arc<String>,
}

/// Middleware validating incoming API calls against an expected Bearer API Key (`Authorization: Bearer <api-key>`)
/// or a valid admin password / session token (`Authorization: Bearer <admin-token>` or `admin_session` cookie).
pub async fn validate_api_key(
    axum::extract::State(state): axum::extract::State<AuthState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(auth_header) = req.headers().get(AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if token == state.api_key.as_str() {
                    return next.run(req).await;
                }
            }
        }
    }

    if is_admin_authenticated(req.headers(), &state.admin_password) {
        return next.run(req).await;
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
