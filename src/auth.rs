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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use tokio::net::TcpListener;

    /// Spawns a minimal router protected by `validate_api_key` with the given `api_key` and `admin_password`.
    async fn spawn_protected_router(api_key: &str, admin_password: &str) -> String {
        let state = AuthState {
            api_key: Arc::new(api_key.to_string()),
            admin_password: Arc::new(admin_password.to_string()),
        };
        let app = Router::new()
            .route("/protected", get(|| async { StatusCode::OK }))
            .route_layer(axum::middleware::from_fn_with_state(
                state,
                validate_api_key,
            ));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// Verifies that `validate_api_key` accepts `X-Admin-Password`, rejects non-Bearer schemes
    /// and empty Bearer tokens, and refuses the unconfigured `"REPLACE_ME"` placeholder.
    #[tokio::test]
    async fn test_validate_api_key_edge_cases() {
        let base_url = spawn_protected_router("api-secret", "custom-admin-pw").await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        // 1. Non-Bearer scheme (`Basic ...`) -> 401
        let res = client
            .get(format!("{base_url}/protected"))
            .header("Authorization", "Basic YWRtaW46YWRtaW4=")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

        // 2. Empty Bearer token (`Bearer `) -> 401
        let res = client
            .get(format!("{base_url}/protected"))
            .header("Authorization", "Bearer ")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

        // 3. Valid `X-Admin-Password` header without `Authorization` -> 200 OK
        let res = client
            .get(format!("{base_url}/protected"))
            .header("x-admin-password", "custom-admin-pw")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        // 4. `ADMIN_PASSWORD="REPLACE_ME"` cannot bypass `validate_api_key`
        let placeholder_url = spawn_protected_router("api-secret", "REPLACE_ME").await;
        let res = client
            .get(format!("{placeholder_url}/protected"))
            .header("x-admin-password", "REPLACE_ME")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

        let res = client
            .get(format!("{placeholder_url}/protected"))
            .header("Authorization", "Bearer REPLACE_ME")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
}
