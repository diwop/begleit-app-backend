use askama::Template;
use axum::{
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;
use subtle::ConstantTimeEq;

/// Static CSS stylesheet embedded at compile time.
const STYLE_CSS: &str = include_str!("../static/style.css");

/// ONNX Runtime Web JavaScript bridge embedded at compile time.
const ORT_BRIDGE_JS: &str = include_str!("../static/ort_bridge.js");

/// 16 kHz downsampling AudioWorkletProcessor script embedded at compile time.
const VAD_WORKLET_JS: &str = include_str!("../static/vad_worklet.js");

/// Streaming SpeechRecognition-to-Diarization transcript alignment module embedded at compile time.
const TRANSCRIPT_ALIGNMENT_JS: &str = include_str!("../static/transcript_alignment.js");

/// Precomputed SHA-256 ETag for the compile-time embedded CSS stylesheet.
static STYLE_CSS_ETAG: LazyLock<String> = LazyLock::new(|| compute_etag(STYLE_CSS.as_bytes()));

/// Precomputed SHA-256 ETag for the compile-time embedded ONNX Runtime Web JavaScript bridge.
static ORT_BRIDGE_JS_ETAG: LazyLock<String> =
    LazyLock::new(|| compute_etag(ORT_BRIDGE_JS.as_bytes()));

/// Precomputed SHA-256 ETag for the compile-time embedded AudioWorkletProcessor script.
static VAD_WORKLET_JS_ETAG: LazyLock<String> =
    LazyLock::new(|| compute_etag(VAD_WORKLET_JS.as_bytes()));

/// Precomputed SHA-256 ETag for the compile-time embedded transcript alignment module.
static TRANSCRIPT_ALIGNMENT_JS_ETAG: LazyLock<String> =
    LazyLock::new(|| compute_etag(TRANSCRIPT_ALIGNMENT_JS.as_bytes()));

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

/// Askama template rendering the WASM VAD + Speaker Diarization test interface at `/vad-diarization`.
#[derive(Template)]
#[template(path = "vad_diarization.html")]
pub struct VadDiarizationTemplate;

/// Categorizes allowlisted static asset directories served from disk for WASM VAD & Diarization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticAssetCategory {
    /// Compiled `wasm-bindgen` output under `static/wasm/`.
    Wasm,
    /// Self-hosted `onnxruntime-web` runtime modules and WASM binaries under `static/ort/`.
    Ort,
    /// Self-hosted ONNX speech models under `models/vad-diarization/`.
    Model,
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

/// Creates the web interface router for the HTML administration pages and static assets.
pub fn router(admin_password: Arc<String>, api_key: Option<Arc<String>>) -> Router {
    let state = WebState {
        admin_password,
        api_key,
    };

    Router::new()
        .route("/", get(index))
        .route("/dialog-translation", get(dialog_translation))
        .route("/vad-diarization", get(vad_diarization))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/static/style.css", get(serve_style))
        .route("/static/ort_bridge.js", get(serve_ort_bridge))
        .route("/static/vad_worklet.js", get(serve_vad_worklet))
        .route(
            "/static/transcript_alignment.js",
            get(serve_transcript_alignment),
        )
        .route("/static/wasm/{filename}", get(serve_wasm_asset))
        .route("/static/ort/{filename}", get(serve_ort_asset))
        .route("/static/models/{filename}", get(serve_model_asset))
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

/// Handler for `GET /vad-diarization`: Renders the WASM VAD + Speaker Diarization test interface
/// with Cross-Origin Isolation headers (`COOP` + `COEP`) when logged in, or the login card otherwise.
async fn vad_diarization(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(cookie_val) = extract_session_cookie(&headers) {
        if is_valid_session(&cookie_val, &state.admin_password) {
            let template = VadDiarizationTemplate;
            return match template.render() {
                Ok(html) => {
                    let mut resp_headers = HeaderMap::new();
                    resp_headers.insert(
                        "cross-origin-opener-policy",
                        header::HeaderValue::from_static("same-origin"),
                    );
                    resp_headers.insert(
                        "cross-origin-embedder-policy",
                        header::HeaderValue::from_static("require-corp"),
                    );
                    (resp_headers, Html(html)).into_response()
                }
                Err(e) => {
                    tracing::error!("Failed to render vad_diarization template: {e}");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
                }
            };
        }
    }

    let template = LoginTemplate {
        error_message: None,
        next: Some("/vad-diarization".to_string()),
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

/// Resolves an allowlisted static filename within `category` to its safe local disk [`PathBuf`]
/// and static MIME `Content-Type` header string, rejecting any unknown file or path traversal attempt.
pub fn resolve_allowlisted_static_asset(
    category: StaticAssetCategory,
    filename: &str,
) -> Option<(PathBuf, &'static str)> {
    if filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
    {
        return None;
    }

    match category {
        StaticAssetCategory::Wasm => match filename {
            "vad_diarization_wasm.js" => Some((
                PathBuf::from("static/wasm").join(filename),
                "text/javascript; charset=utf-8",
            )),
            "vad_diarization_wasm_bg.wasm" => Some((
                PathBuf::from("static/wasm").join(filename),
                "application/wasm",
            )),
            _ => None,
        },
        StaticAssetCategory::Ort => match filename {
            "ort.webgpu.min.mjs"
            | "ort.wasm.min.mjs"
            | "ort-wasm-simd-threaded.mjs"
            | "ort-wasm-simd-threaded.jsep.mjs"
            | "ort-wasm-simd-threaded.jspi.mjs"
            | "ort-wasm-simd-threaded.asyncify.mjs" => Some((
                PathBuf::from("static/ort").join(filename),
                "text/javascript; charset=utf-8",
            )),
            "ort-wasm-simd-threaded.wasm"
            | "ort-wasm-simd-threaded.jsep.wasm"
            | "ort-wasm-simd-threaded.jspi.wasm"
            | "ort-wasm-simd-threaded.asyncify.wasm" => Some((
                PathBuf::from("static/ort").join(filename),
                "application/wasm",
            )),
            _ => None,
        },
        StaticAssetCategory::Model => match filename {
            "silero_vad_v5.onnx"
            | "pyannote_segmentation_3_0.onnx"
            | "wespeaker_voxblink2_samresnet100_LM.int8.onnx"
            | "wespeaker_en_voxceleb_resnet152_LM.onnx"
            | "wespeaker_en_voxceleb_CAM++_LM.onnx" => Some((
                PathBuf::from("models/vad-diarization").join(filename),
                "application/octet-stream",
            )),
            _ => None,
        },
    }
}

/// Cached metadata and computed SHA-256 ETag for an on-disk static file.
#[derive(Debug, Clone)]
struct DiskAssetCacheEntry {
    modified: SystemTime,
    file_size: u64,
    etag: String,
}

/// In-memory cache mapping disk file paths to their last known modification time, size, and ETag.
static DISK_ASSET_CACHE: LazyLock<Mutex<HashMap<PathBuf, DiskAssetCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Computes a quoted SHA-256 ETag string (e.g. `"\"d41d8cd9...\""`) for the provided byte slice.
pub fn compute_etag(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut etag = String::with_capacity(66);
    etag.push('"');
    for b in digest {
        use std::fmt::Write;
        let _ = write!(etag, "{b:02x}");
    }
    etag.push('"');
    etag
}

/// Checks whether the incoming `If-None-Match` request header matches the resource's `etag`.
///
/// Supports exact quoted matching, wildcard `*`, and comma-delimited lists per RFC 7232 § 3.2.
pub fn matches_if_none_match(req_headers: &HeaderMap, etag: &str) -> bool {
    let Some(header_val) = req_headers.get(header::IF_NONE_MATCH) else {
        return false;
    };
    let Ok(header_str) = header_val.to_str() else {
        return false;
    };
    let trimmed = header_str.trim();
    if trimmed == "*" {
        return true;
    }
    for item in trimmed.split(',') {
        let tag = item.trim();
        let clean_tag = tag.strip_prefix("W/").unwrap_or(tag);
        let clean_etag = etag.strip_prefix("W/").unwrap_or(etag);
        if clean_tag == clean_etag {
            return true;
        }
    }
    false
}

/// Builds response headers for same-origin static assets compatible with
/// `Cross-Origin-Embedder-Policy: require-corp` and includes `ETag` and `Cache-Control`.
fn build_static_headers(
    content_type: &'static str,
    cache_control: &'static str,
    etag: &str,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache_control),
    );
    if let Ok(etag_val) = header::HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, etag_val);
    }
    headers.insert(
        "cross-origin-resource-policy",
        header::HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "cross-origin-embedder-policy",
        header::HeaderValue::from_static("require-corp"),
    );
    headers.insert(
        "cross-origin-opener-policy",
        header::HeaderValue::from_static("same-origin"),
    );
    headers
}

/// Handler for `GET /static/style.css`: Serves the embedded CSS stylesheet with ETag validation.
async fn serve_style(headers: HeaderMap) -> Response {
    if matches_if_none_match(&headers, &STYLE_CSS_ETAG) {
        (
            StatusCode::NOT_MODIFIED,
            build_static_headers("text/css; charset=utf-8", "no-cache", &STYLE_CSS_ETAG),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            build_static_headers("text/css; charset=utf-8", "no-cache", &STYLE_CSS_ETAG),
            STYLE_CSS,
        )
            .into_response()
    }
}

/// Handler for `GET /static/ort_bridge.js`: Serves the embedded ONNX Runtime Web JS bridge with ETag validation.
async fn serve_ort_bridge(headers: HeaderMap) -> Response {
    if matches_if_none_match(&headers, &ORT_BRIDGE_JS_ETAG) {
        (
            StatusCode::NOT_MODIFIED,
            build_static_headers(
                "text/javascript; charset=utf-8",
                "no-cache",
                &ORT_BRIDGE_JS_ETAG,
            ),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            build_static_headers(
                "text/javascript; charset=utf-8",
                "no-cache",
                &ORT_BRIDGE_JS_ETAG,
            ),
            ORT_BRIDGE_JS,
        )
            .into_response()
    }
}

/// Handler for `GET /static/vad_worklet.js`: Serves the embedded 16 kHz AudioWorkletProcessor script with ETag validation.
async fn serve_vad_worklet(headers: HeaderMap) -> Response {
    if matches_if_none_match(&headers, &VAD_WORKLET_JS_ETAG) {
        (
            StatusCode::NOT_MODIFIED,
            build_static_headers(
                "text/javascript; charset=utf-8",
                "no-cache",
                &VAD_WORKLET_JS_ETAG,
            ),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            build_static_headers(
                "text/javascript; charset=utf-8",
                "no-cache",
                &VAD_WORKLET_JS_ETAG,
            ),
            VAD_WORKLET_JS,
        )
            .into_response()
    }
}

/// Handler for `GET /static/transcript_alignment.js`: Serves the embedded streaming SpeechRecognition-to-Diarization alignment module with ETag validation.
async fn serve_transcript_alignment(headers: HeaderMap) -> Response {
    if matches_if_none_match(&headers, &TRANSCRIPT_ALIGNMENT_JS_ETAG) {
        (
            StatusCode::NOT_MODIFIED,
            build_static_headers(
                "text/javascript; charset=utf-8",
                "no-cache",
                &TRANSCRIPT_ALIGNMENT_JS_ETAG,
            ),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            build_static_headers(
                "text/javascript; charset=utf-8",
                "no-cache",
                &TRANSCRIPT_ALIGNMENT_JS_ETAG,
            ),
            TRANSCRIPT_ALIGNMENT_JS,
        )
            .into_response()
    }
}

/// Serves an allowlisted disk file for `category` and `filename`, validating `If-None-Match`
/// against the cached SHA-256 ETag to return `304 Not Modified` without re-reading
/// large assets from disk.
async fn serve_allowlisted_disk_asset(
    category: StaticAssetCategory,
    filename: &str,
    req_headers: &HeaderMap,
    cache_control: &'static str,
) -> Response {
    let Some((path, content_type)) = resolve_allowlisted_static_asset(category, filename) else {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    };

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(_) => return (StatusCode::NOT_FOUND, "Asset Not Found").into_response(),
    };

    let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let file_size = metadata.len();

    let cached_etag = {
        let cache = DISK_ASSET_CACHE.lock().unwrap();
        cache.get(&path).and_then(|entry| {
            if entry.modified == mtime && entry.file_size == file_size {
                Some(entry.etag.clone())
            } else {
                None
            }
        })
    };

    let (etag, maybe_bytes) = match cached_etag {
        Some(tag) => (tag, None),
        None => {
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(_) => return (StatusCode::NOT_FOUND, "Asset Not Found").into_response(),
            };
            let computed = compute_etag(&bytes);
            let mut cache = DISK_ASSET_CACHE.lock().unwrap();
            cache.insert(
                path.clone(),
                DiskAssetCacheEntry {
                    modified: mtime,
                    file_size,
                    etag: computed.clone(),
                },
            );
            (computed, Some(bytes))
        }
    };

    if matches_if_none_match(req_headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            build_static_headers(content_type, cache_control, &etag),
        )
            .into_response();
    }

    let bytes = match maybe_bytes {
        Some(b) => b,
        None => match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => return (StatusCode::NOT_FOUND, "Asset Not Found").into_response(),
        },
    };

    (
        StatusCode::OK,
        build_static_headers(content_type, cache_control, &etag),
        bytes,
    )
        .into_response()
}

/// Handler for `GET /static/wasm/{filename}`: Serves compiled `vad-diarization-wasm` bundle files with ETag validation.
async fn serve_wasm_asset(headers: HeaderMap, AxumPath(filename): AxumPath<String>) -> Response {
    serve_allowlisted_disk_asset(StaticAssetCategory::Wasm, &filename, &headers, "no-cache").await
}

/// Handler for `GET /static/ort/{filename}`: Serves self-hosted `onnxruntime-web` modules and WASM binaries with ETag validation.
async fn serve_ort_asset(headers: HeaderMap, AxumPath(filename): AxumPath<String>) -> Response {
    serve_allowlisted_disk_asset(StaticAssetCategory::Ort, &filename, &headers, "no-cache").await
}

/// Handler for `GET /static/models/{filename}`: Serves allowlisted Silero VAD and WeSpeaker ONNX models with ETag validation.
async fn serve_model_asset(headers: HeaderMap, AxumPath(filename): AxumPath<String>) -> Response {
    serve_allowlisted_disk_asset(StaticAssetCategory::Model, &filename, &headers, "no-cache").await
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

    /// Verifies strict filename allowlisting and MIME resolution for WASM, ORT, and ONNX static assets.
    #[test]
    fn test_resolve_allowlisted_static_asset() {
        assert_eq!(
            resolve_allowlisted_static_asset(
                StaticAssetCategory::Wasm,
                "vad_diarization_wasm_bg.wasm"
            ),
            Some((
                PathBuf::from("static/wasm/vad_diarization_wasm_bg.wasm"),
                "application/wasm"
            ))
        );
        assert_eq!(
            resolve_allowlisted_static_asset(StaticAssetCategory::Ort, "ort.webgpu.min.mjs"),
            Some((
                PathBuf::from("static/ort/ort.webgpu.min.mjs"),
                "text/javascript; charset=utf-8"
            ))
        );
        assert_eq!(
            resolve_allowlisted_static_asset(StaticAssetCategory::Model, "silero_vad_v5.onnx"),
            Some((
                PathBuf::from("models/vad-diarization/silero_vad_v5.onnx"),
                "application/octet-stream"
            ))
        );
        assert_eq!(
            resolve_allowlisted_static_asset(
                StaticAssetCategory::Model,
                "pyannote_segmentation_3_0.onnx"
            ),
            Some((
                PathBuf::from("models/vad-diarization/pyannote_segmentation_3_0.onnx"),
                "application/octet-stream"
            ))
        );
        assert_eq!(
            resolve_allowlisted_static_asset(
                StaticAssetCategory::Model,
                "wespeaker_voxblink2_samresnet100_LM.int8.onnx"
            ),
            Some((
                PathBuf::from(
                    "models/vad-diarization/wespeaker_voxblink2_samresnet100_LM.int8.onnx"
                ),
                "application/octet-stream"
            ))
        );
        assert_eq!(
            resolve_allowlisted_static_asset(
                StaticAssetCategory::Model,
                "wespeaker_en_voxceleb_resnet152_LM.onnx"
            ),
            Some((
                PathBuf::from("models/vad-diarization/wespeaker_en_voxceleb_resnet152_LM.onnx"),
                "application/octet-stream"
            ))
        );

        // Reject path traversal and non-allowlisted filenames
        assert!(
            resolve_allowlisted_static_asset(StaticAssetCategory::Model, "../Cargo.toml").is_none()
        );
        assert!(
            resolve_allowlisted_static_asset(StaticAssetCategory::Wasm, "unknown.wasm").is_none()
        );
        assert!(resolve_allowlisted_static_asset(StaticAssetCategory::Ort, "").is_none());
    }
}
