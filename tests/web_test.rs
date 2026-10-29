use diwop_begleitapp::{create_app, web};
use reqwest::header;
use tokio::net::TcpListener;

/// Starts an ephemeral test server running the application router and returns its base URL.
async fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    url
}

/// Verifies that an unauthenticated GET to `/` renders the login card.
#[tokio::test]
async fn test_unauthenticated_index_serves_login_page() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client.get(format!("{}/", base_url)).send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let html = res.text().await.unwrap();
    assert!(
        html.contains("Admin-Anmeldung"),
        "HTML should contain login heading"
    );
    assert!(
        html.contains("action=\"/login\""),
        "HTML should contain form targeting /login"
    );
    assert!(
        html.contains("name=\"password\""),
        "HTML should contain password input"
    );
    assert!(
        html.contains("autocomplete=\"current-password\""),
        "HTML should contain autocomplete=current-password"
    );
}

/// Verifies that POST `/login` with an incorrect password returns 401 Unauthorized and an error message.
#[tokio::test]
async fn test_login_failure_invalid_password() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let form_params = [("password", "wrong-password")];
    let res = client
        .post(format!("{}/login", base_url))
        .form(&form_params)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Ungültiges Admin-Passwort."),
        "HTML should display invalid password error message"
    );
}

/// Verifies that POST `/login` with a valid password responds with 303 Redirect and sets the session cookie.
#[tokio::test]
async fn test_login_success_sets_cookie_and_redirects() {
    let base_url = start_server().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let form_params = [("password", "admin")];
    let res = client
        .post(format!("{}/login", base_url))
        .form(&form_params)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok()),
        Some("/")
    );

    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|h| h.to_str().ok())
        .expect("Set-Cookie header should be present");

    assert!(
        set_cookie.starts_with("admin_session="),
        "Cookie should start with admin_session="
    );
    assert!(
        set_cookie.contains("HttpOnly"),
        "Cookie must specify HttpOnly"
    );
    assert!(
        set_cookie.contains("SameSite=Lax"),
        "Cookie must specify SameSite=Lax"
    );
    assert!(set_cookie.contains("Path=/"), "Cookie must specify Path=/");
}

/// Verifies that GET `/` with a valid session cookie serves the authenticated dashboard view.
#[tokio::test]
async fn test_authenticated_index_serves_dashboard() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    let res = client
        .get(format!("{}/", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Willkommen im Administrationsbereich"),
        "HTML should display the dashboard welcome message"
    );
    assert!(
        html.contains("action=\"/logout\""),
        "HTML should contain logout form"
    );
}

/// Verifies that POST `/logout` clears the session cookie and redirects to `/`.
#[tokio::test]
async fn test_logout_clears_cookie() {
    let base_url = start_server().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = client
        .post(format!("{}/logout", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok()),
        Some("/")
    );

    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|h| h.to_str().ok())
        .expect("Set-Cookie header should be present");

    assert!(
        set_cookie.contains("Max-Age=0"),
        "Logout should clear cookie with Max-Age=0"
    );
}

/// Verifies that GET `/static/style.css` serves the embedded CSS with proper content-type and caching headers.
#[tokio::test]
async fn test_static_style_css() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/static/style.css", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok()),
        Some("text/css; charset=utf-8")
    );

    let css = res.text().await.unwrap();
    assert!(
        css.contains("color-scheme: light dark;"),
        "CSS should configure color-scheme"
    );
    assert!(
        css.contains(".login-card"),
        "CSS should style .login-card component"
    );
}
