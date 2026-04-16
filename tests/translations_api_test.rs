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

    // Spawn the server in the background
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Provide the caller with the ephemeral URL where the server was successfully bound
    url
}

#[tokio::test]
async fn test_translations_json_endpoint() {
    let base_url = start_server().await;

    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "original": "Test String"
    });

    let res = client
        .post(&format!("{}/translations/translate", base_url))
        .json(&body)
        .send()
        .await
        .unwrap();

    // The user requested to strictly check for an OK status code
    assert_eq!(res.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn test_translations_grpc_endpoint() {
    let base_url = start_server().await;

    // Build the underlying tonic client against our dynamic HTTP URL
    let mut client = TranslationsClient::connect(base_url).await.unwrap();

    let request = tonic::Request::new(TranslateRequest {
        original: "Test String".into(),
    });

    let res = client.translate(request).await.unwrap().into_inner();

    // The user requested to match for a non-empty value in "translated"
    assert!(
        !res.translated.is_empty(),
        "Translated string should not be empty"
    );
}
