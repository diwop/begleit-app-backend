use diwop_begleitapp::create_app;
use diwop_begleitapp::management::management::{
    management_client::ManagementClient, ListUsersRequest,
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
async fn test_management_json_endpoint() {
    let base_url = start_server().await;

    let client = reqwest::Client::new();

    let res = client
        .post(&format!("{}/management/list-users", base_url))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    // The user requested to strictly check for an OK status code
    assert_eq!(res.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn test_management_grpc_endpoint() {
    let base_url = start_server().await;

    // Build the underlying tonic client against our dynamic HTTP URL
    let mut client = ManagementClient::connect(base_url).await.unwrap();

    let request = tonic::Request::new(ListUsersRequest {});

    let res = client.list_users(request).await.unwrap().into_inner();

    // Ensure it correctly serves an empty array
    assert!(res.users.is_empty(), "Users list should be empty");
}
