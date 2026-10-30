use diwop_begleitapp::{create_app, ConversationMessage, ErrorResponse, Role};
use tokio::net::TcpListener;

/// Starts an ephemeral test server running the application router and returns its base URL.
async fn start_server() -> String {
    std::env::remove_var("GEMINI_API_KEY");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    url
}

/// Verifies that the `Authorization: Bearer <api-key>` header is enforced when `API_KEY` is configured.
#[tokio::test]
async fn test_auth_header_validation() {
    std::env::set_var("API_KEY", "test-auth-secret-123");

    let base_url = start_server().await;
    let rest_client = reqwest::Client::new();
    let payload = ConversationMessage {
        role: Role::Partner,
        message: "sie müssen den antrag bis ende nächste woche abgeben".to_string(),
    };

    let res = rest_client
        .post(format!("{}/correct-transcript", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "UNAUTHORIZED");

    let res = rest_client
        .post(format!("{}/correct-transcript", base_url))
        .header("Authorization", "Bearer wrong-key")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "UNAUTHORIZED");

    let res = rest_client
        .post(format!("{}/correct-transcript", base_url))
        .header("Authorization", "Bearer test-auth-secret-123")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let raw_api_key: serde_json::Value = res.json().await.unwrap();
    assert!(
        raw_api_key.get("debugInfo").is_none(),
        "Standard API_KEY must not expose debugInfo"
    );

    // Admin password Bearer token also authorizes the request and populates debugInfo
    let res_admin = rest_client
        .post(format!("{}/correct-transcript", base_url))
        .header("Authorization", "Bearer admin")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(res_admin.status(), reqwest::StatusCode::OK);
    let raw_admin: serde_json::Value = res_admin.json().await.unwrap();
    assert!(
        raw_admin.get("debugInfo").is_some(),
        "Valid admin token should authorize and populate debugInfo"
    );

    // API_KEY Bearer token + admin_session cookie (as sent by /dialog-translation) populates debugInfo
    let session_token = diwop_begleitapp::web::generate_session_token("admin");
    let res_ui = rest_client
        .post(format!("{}/correct-transcript", base_url))
        .header("Authorization", "Bearer test-auth-secret-123")
        .header("Cookie", format!("admin_session={session_token}"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(res_ui.status(), reqwest::StatusCode::OK);
    let raw_ui: serde_json::Value = res_ui.json().await.unwrap();
    assert!(
        raw_ui.get("debugInfo").is_some(),
        "API_KEY + admin_session cookie should populate debugInfo"
    );

    // Health check and readiness endpoints remain unauthenticated even when API_KEY is set
    let res = rest_client
        .get(format!("{}/healthz", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let res = rest_client
        .get(format!("{}/readyz", base_url))
        .send()
        .await
        .unwrap();
    // In test/debug environment with EchoPromptRunner, warmup finishes quickly and yields 200 OK
    assert_eq!(res.status(), reqwest::StatusCode::OK);
}

#[test]
#[cfg(not(debug_assertions))]
fn test_release_crash_without_key() {
    std::env::remove_var("API_KEY");
    let result = std::panic::catch_unwind(|| {
        let _router = diwop_begleitapp::create_app();
    });

    assert!(
        result.is_err(),
        "Expected create_app() to panic in release mode when API_KEY is omitted."
    );
}
