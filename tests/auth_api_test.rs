use diwop_begleitapp::create_app;
use diwop_begleitapp::translations::translations::{
    translations_client::TranslationsClient, TranslateRequest,
};
use tokio::net::TcpListener;

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

#[tokio::test]
async fn test_auth_header_validation() {
    std::env::set_var("API_KEY", "test-auth-secret-123");

    let base_url = start_server().await;
    let rest_client = reqwest::Client::new();

    // --- 1. REST JSON Testing ---

    let res = rest_client
        .post(&format!("{}/translations/translate", base_url))
        .json(&serde_json::json!({"original": "Hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    let res = rest_client
        .post(&format!("{}/translations/translate", base_url))
        .header("x-api-key", "wrong-key")
        .json(&serde_json::json!({"original": "Hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    let res = rest_client
        .post(&format!("{}/translations/translate", base_url))
        .header("x-api-key", "test-auth-secret-123")
        .json(&serde_json::json!({"original": "Hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // --- 2. gRPC Testing ---

    let mut grpc_client = TranslationsClient::connect(base_url).await.unwrap();

    let request = tonic::Request::new(TranslateRequest {
        original: "Hello".into(),
    });
    let err = grpc_client
        .translate(request)
        .await
        .expect_err("Should fail without key");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    let mut request = tonic::Request::new(TranslateRequest {
        original: "Hello".into(),
    });
    request
        .metadata_mut()
        .insert("x-api-key", "test-auth-secret-123".parse().unwrap());

    let res = grpc_client
        .translate(request)
        .await
        .expect("Should succeed with key");
    assert!(!res.into_inner().translated.is_empty());
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
