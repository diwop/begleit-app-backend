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
    pub next: Option<String>,
}

/// Askama template rendering the authenticated dashboard at `/`.
#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate;

/// Askama template rendering the dialog translation test interface at `/dialog-translation`.
#[derive(Template)]
#[template(path = "dialog_translation.html")]
pub struct DialogTranslationTemplate {
    pub api_key: String,
}

/// Payload received from the HTML login form submission (`POST /login`).
#[derive(serde::Deserialize)]
pub struct LoginForm {
    pub password: String,
    pub next: Option<String>,
}

/// Shared application state for the web router holding the configured admin password and optional API key.
#[derive(Clone)]
pub struct WebState {
    pub admin_password: Arc<String>,
    pub api_key: Option<Arc<String>>,
}

/// Creates the web interface router for the HTML administration pages.
pub fn router(admin_password: Arc<String>, api_key: Option<Arc<String>>) -> Router {
    let state = WebState {
        admin_password,
        api_key,
    };

    Router::new()
        .route("/", get(index))
        .route("/dialog-translation", get(dialog_translation))
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
pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
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

/// Validates a post-login redirect path, ensuring it is a safe relative path starting with `/`
/// and not an external or protocol-relative URL (e.g. `//evil.com` or `/\evil.com`).
pub fn validate_redirect_target(target: Option<&str>) -> &str {
    match target {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.starts_with("/\\")
                && !path.contains('\r')
                && !path.contains('\n') =>
        {
            path
        }
        _ => "/",
    }
}

/// Checks whether the incoming request headers carry a valid admin session cookie
/// (`admin_session=<token>`), a valid `Authorization: Bearer <admin_password_or_session_token>` header,
/// or a valid `X-Admin-Password: <admin_password_or_session_token>` header.
///
/// Rejects empty passwords and the unconfigured `"REPLACE_ME"` placeholder.
pub fn is_admin_authenticated(headers: &HeaderMap, admin_password: &str) -> bool {
    let trimmed_pw = admin_password.trim();
    if trimmed_pw.is_empty() || trimmed_pw == "REPLACE_ME" {
        return false;
    }

    if let Some(cookie_val) = extract_session_cookie(headers) {
        if is_valid_session(&cookie_val, trimmed_pw) {
            return true;
        }
    }

    if let Some(auth_header) = headers.get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                let trimmed_token = token.trim();
                if !trimmed_token.is_empty() {
                    let matches_pw: bool =
                        trimmed_token.as_bytes().ct_eq(trimmed_pw.as_bytes()).into();
                    if matches_pw || is_valid_session(trimmed_token, trimmed_pw) {
                        return true;
                    }
                }
            }
        }
    }

    if let Some(admin_header) = headers.get("x-admin-password") {
        if let Ok(admin_str) = admin_header.to_str() {
            let trimmed_token = admin_str.trim();
            if !trimmed_token.is_empty() {
                let matches_pw: bool = trimmed_token.as_bytes().ct_eq(trimmed_pw.as_bytes()).into();
                if matches_pw || is_valid_session(trimmed_token, trimmed_pw) {
                    return true;
                }
            }
        }
    }

    false
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
        next: None,
    };
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!("Failed to render login template: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}

/// Handler for `GET /dialog-translation`: Renders the dialog translation test interface if logged in,
/// or the login card with a return target redirect otherwise.
async fn dialog_translation(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(cookie_val) = extract_session_cookie(&headers) {
        if is_valid_session(&cookie_val, &state.admin_password) {
            let template = DialogTranslationTemplate {
                api_key: state
                    .api_key
                    .as_ref()
                    .map_or(String::new(), |k| k.to_string()),
            };
            return match template.render() {
                Ok(html) => Html(html).into_response(),
                Err(e) => {
                    tracing::error!("Failed to render dialog translation template: {e}");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
                }
            };
        }
    }

    let template = LoginTemplate {
        error_message: None,
        next: Some("/dialog-translation".to_string()),
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

        let target = validate_redirect_target(form.next.as_deref());
        (headers, Redirect::to(target)).into_response()
    } else {
        tracing::warn!("Failed admin login attempt: invalid or empty password");
        let template = LoginTemplate {
            error_message: Some("Ungültiges Admin-Passwort.".to_string()),
            next: form.next,
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

    /// Verifies safe relative redirect path validation preventing open redirects.
    #[test]
    fn test_validate_redirect_target() {
        assert_eq!(validate_redirect_target(None), "/");
        assert_eq!(validate_redirect_target(Some("")), "/");
        assert_eq!(validate_redirect_target(Some("https://evil.com")), "/");
        assert_eq!(validate_redirect_target(Some("//evil.com")), "/");
        assert_eq!(validate_redirect_target(Some("/\\evil.com")), "/");
        assert_eq!(
            validate_redirect_target(Some("/dialog-translation\r\nevil")),
            "/"
        );
        assert_eq!(
            validate_redirect_target(Some("/dialog-translation")),
            "/dialog-translation"
        );
        assert_eq!(
            validate_redirect_target(Some("/some/other/path")),
            "/some/other/path"
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

    /// Verifies `is_admin_authenticated` with session cookies, raw admin passwords, session Bearer tokens, and invalid inputs.
    #[test]
    fn test_is_admin_authenticated() {
        let admin_pw = "my-admin-secret";
        let session_token = generate_session_token(admin_pw);

        let empty_headers = HeaderMap::new();
        assert!(!is_admin_authenticated(&empty_headers, admin_pw));

        // Valid admin_session cookie
        let mut cookie_headers = HeaderMap::new();
        cookie_headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("admin_session={session_token}")).unwrap(),
        );
        assert!(is_admin_authenticated(&cookie_headers, admin_pw));

        // Valid Bearer raw admin password
        let mut bearer_pw_headers = HeaderMap::new();
        bearer_pw_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {admin_pw}")).unwrap(),
        );
        assert!(is_admin_authenticated(&bearer_pw_headers, admin_pw));

        // Valid Bearer session token
        let mut bearer_tok_headers = HeaderMap::new();
        bearer_tok_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {session_token}")).unwrap(),
        );
        assert!(is_admin_authenticated(&bearer_tok_headers, admin_pw));

        // Wrong Bearer token alone -> not admin
        let mut wrong_headers = HeaderMap::new();
        wrong_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer regular-api-key"),
        );
        assert!(!is_admin_authenticated(&wrong_headers, admin_pw));

        // Regular API key Bearer token + valid admin_session cookie (as sent by /dialog-translation) -> admin
        let mut ui_headers = wrong_headers.clone();
        ui_headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("admin_session={session_token}")).unwrap(),
        );
        assert!(is_admin_authenticated(&ui_headers, admin_pw));

        // Regular API key Bearer token + X-Admin-Password header -> admin
        let mut x_admin_headers = wrong_headers.clone();
        x_admin_headers.insert("x-admin-password", HeaderValue::from_str(admin_pw).unwrap());
        assert!(is_admin_authenticated(&x_admin_headers, admin_pw));

        // Placeholder REPLACE_ME or empty password must always be rejected
        assert!(!is_admin_authenticated(&bearer_pw_headers, "REPLACE_ME"));
        assert!(!is_admin_authenticated(&bearer_pw_headers, "   "));
    }
}
