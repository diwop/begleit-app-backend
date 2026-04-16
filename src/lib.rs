pub mod macros;
pub mod management;
pub mod translations;

use axum::Router;
use tower_http::trace::TraceLayer;

pub fn create_app() -> Router {
    // Get the separate REST and gRPC components
    let (translations_rest, translations_grpc) = translations::setup();
    let (management_rest, management_grpc) = management::setup();

    // Construct a SINGLE Tonic server housing all our gRPC endpoints
    #[allow(deprecated)]
    let grpc_router = tonic::transport::Server::builder()
        .add_service(translations_grpc)
        .add_service(management_grpc)
        .into_router();

    // Combine all REST routers and the consolidated gRPC router
    translations_rest
        .merge(management_rest)
        .merge(grpc_router)
        .layer(TraceLayer::new_for_http())
}
