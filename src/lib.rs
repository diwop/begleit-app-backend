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

    // Extract the Swagger UI components
    let translations_swagger_json = include_str!("../api-docs/translations.swagger.json");
    let translations_swagger_val: serde_json::Value =
        serde_json::from_str(translations_swagger_json)
            .expect("Generated Translations API Swagger must be valid JSON");

    let management_swagger_json = include_str!("../api-docs/management.swagger.json");
    let management_swagger_val: serde_json::Value = serde_json::from_str(management_swagger_json)
        .expect("Generated Management API Swagger must be valid JSON");

    let swagger_ui = utoipa_swagger_ui::SwaggerUi::new("/open-api")
        .external_url_unchecked("/api-docs/translations.json", translations_swagger_val)
        .external_url_unchecked("/api-docs/management.json", management_swagger_val);

    // Combine all REST routers and the consolidated gRPC router
    translations_rest
        .merge(management_rest)
        .merge(grpc_router)
        .merge(swagger_ui)
        .layer(TraceLayer::new_for_http())
}
