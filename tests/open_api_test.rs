use diwop_begleitapp::create_app;
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
async fn test_openapi_endpoints() {
    let base_url = start_server().await;

    let client = reqwest::Client::new();

    // Verify Swagger UI HTML is served
    let res = client
        .get(&format!("{}/open-api/", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // Verify Translations Swagger JSON is served
    let res = client
        .get(&format!("{}/api-docs/translations.json", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // Verify Management Swagger JSON is served
    let res = client
        .get(&format!("{}/api-docs/management.json", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
}
