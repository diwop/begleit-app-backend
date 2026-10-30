use askama::Template;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Static CSS stylesheet embedded at compile time.
const STYLE_CSS: &str = include_str!("../static/style.css");

/// Name of the session cookie used to authenticate the admin web interface.
pub const SESSION_COOKIE_NAME: &str = "admin_session";

/// Askama template rendering the login card at `/`.
#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub error_message: Option<String>,
}

/// Askama template rendering the authenticated dashboard at `/`.
#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate;

/// Payload received from the HTML login form submission (`POST /login`).
#[derive(serde::Deserialize)]
pub struct LoginForm {
    pub password: String,
}

/// Shared application state for the web router holding the configured admin password.
#[derive(Clone)]
pub struct WebState {
    pub admin_password: Arc<String>,
}

/// Creates the web interface router for the HTML administration pages.
pub fn router(admin_password: Arc<String>) -> Router {
    let state = WebState { admin_password };

    Router::new()
        .route("/", get(index))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/static/style.css", get(serve_style))
        .with_state(state)
}

/// Generates a deterministic SHA-256 session token from the admin password.
///
/// Any client possessing this token proves knowledge of the active server password
/// without requiring a persistent database or stateful server-side sessions.
pub fn generate_session_token(admin_password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"diwop-admin-session-v1:");
    hasher.update(admin_password.as_bytes());
    // `sha2` 0.11 digests no longer implement `LowerHex`, so encode each byte explicitly.
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

/// Validates whether a provided session token matches the active admin password in constant time.
pub fn is_valid_session(token: &str, admin_password: &str) -> bool {
    let expected = generate_session_token(admin_password);
    expected.as_bytes().ct_eq(token.as_bytes()).into()
}

/// Extracts the value of the `admin_session` cookie from the incoming request headers.
fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in cookie_header.split(';') {
        let mut parts = pair.trim().splitn(2, '=');
        if let (Some(name), Some(val)) = (parts.next(), parts.next()) {
            if name == SESSION_COOKIE_NAME {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Handler for `GET /`: Renders the authenticated dashboard if logged in, or the login form otherwise.
async fn index(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(cookie_val) = extract_session_cookie(&headers) {
        if is_valid_session(&cookie_val, &state.admin_password) {
            let template = DashboardTemplate;
            return match template.render() {
                Ok(html) => Html(html).into_response(),
                Err(e) => {
                    tracing::error!("Failed to render dashboard template: {e}");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
                }
            };
        }
    }

    let template = LoginTemplate {
        error_message: None,
    };
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!("Failed to render login template: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}

/// Handler for `POST /login`: Validates the admin password and sets an `HttpOnly` session cookie on success.
async fn login(State(state): State<WebState>, Form(form): Form<LoginForm>) -> Response {
    let trimmed_pw = form.password.trim();

    // Reject empty passwords and unconfigured default placeholder
    let is_valid = if trimmed_pw.is_empty() || state.admin_password.as_str() == "REPLACE_ME" {
        false
    } else {
        trimmed_pw
            .as_bytes()
            .ct_eq(state.admin_password.as_bytes())
            .into()
    };

    if is_valid {
        let token = generate_session_token(&state.admin_password);
        let cookie = format!(
            "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
            60 * 60 * 24 * 7 // 7 days
        );

        let mut headers = HeaderMap::new();
        if let Ok(cookie_val) = header::HeaderValue::from_str(&cookie) {
            headers.insert(header::SET_COOKIE, cookie_val);
        }

        (headers, Redirect::to("/")).into_response()
    } else {
        tracing::warn!("Failed admin login attempt: invalid or empty password");
        let template = LoginTemplate {
            error_message: Some("Ungültiges Admin-Passwort.".to_string()),
        };
        match template.render() {
            Ok(html) => (StatusCode::UNAUTHORIZED, Html(html)).into_response(),
            Err(e) => {
                tracing::error!("Failed to render login template: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
        }
    }
}

/// Handler for `POST /logout`: Clears the session cookie and redirects to `/`.
async fn logout() -> Response {
    let cookie = format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    let mut headers = HeaderMap::new();
    if let Ok(cookie_val) = header::HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, cookie_val);
    }
    (headers, Redirect::to("/")).into_response()
}

/// Handler for `GET /static/style.css`: Serves the embedded CSS stylesheet.
async fn serve_style() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/css; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=3600"),
    );
    (headers, STYLE_CSS).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    /// Verifies deterministic generation and constant-time validation of session tokens.
    #[test]
    fn test_generate_and_validate_session_token() {
        let password = "super-secret-admin-password";
        let token = generate_session_token(password);

        assert!(!token.is_empty(), "Token should not be empty");
        assert!(
            is_valid_session(&token, password),
            "Valid token should authenticate successfully"
        );
        assert!(
            !is_valid_session(&token, "wrong-password"),
            "Token generated with different password should fail validation"
        );
        assert!(
            !is_valid_session("invalid-token-string", password),
            "Arbitrary invalid token string should fail validation"
        );
    }

    /// Verifies extraction of the `admin_session` cookie from HeaderMap across various formats.
    #[test]
    fn test_extract_session_cookie() {
        let mut headers = HeaderMap::new();
        assert_eq!(
            extract_session_cookie(&headers),
            None,
            "Empty headers should yield None"
        );

        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=123; admin_session=my-secret-token; foo=bar"),
        );
        assert_eq!(
            extract_session_cookie(&headers),
            Some("my-secret-token".to_string()),
            "Cookie value should be cleanly extracted amongst multiple cookies"
        );

        let mut single_cookie_headers = HeaderMap::new();
        single_cookie_headers.insert(
            header::COOKIE,
            HeaderValue::from_static("admin_session=solo-token"),
        );
        assert_eq!(
            extract_session_cookie(&single_cookie_headers),
            Some("solo-token".to_string()),
            "Single cookie header should be cleanly extracted"
        );
    }
}
