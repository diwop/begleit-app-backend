use std::net::SocketAddr;
use tracing::info;

#[tokio::main]
async fn main() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env();

    if cfg!(debug_assertions) {
        // Pretty, colorful output for local development
        tracing_subscriber::fmt()
            .with_env_filter(
                env_filter.unwrap_or_else(|_| "diwop_begleitapp=debug,tower_http=debug".into()),
            )
            .init();
    } else {
        // Configure STACKIT/Kubernetes friendly structured JSON logging for production
        tracing_subscriber::fmt()
            .with_env_filter(
                env_filter.unwrap_or_else(|_| "diwop_begleitapp=info,tower_http=info".into()),
            )
            .json()
            .flatten_event(true)
            .init();
    }

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let app = diwop_begleitapp::create_app();

    info!(port = port, "Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
