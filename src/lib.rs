pub mod auth;
pub mod macros;
pub mod management;
pub mod translations;

use axum::Router;
use tower_http::trace::TraceLayer;

/// Creates and configures the main application router.
///
/// This function aggregates all separate components of the application into a single
/// `axum::Router`. It sets up both REST and gRPC endpoints for various modules (like
/// translations and management), configures Swagger UI to expose OpenAPI documentation,
/// and applies necessary middleware such as request tracing.
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

    // Attempt to extract the API Key enforcing production rules
    let expected_api_key = match std::env::var("API_KEY") {
        Ok(key) => Some(std::sync::Arc::new(key)),
        Err(_) => {
            #[cfg(not(debug_assertions))]
            panic!("API_KEY environment variable is crucially required in release profile!");

            #[cfg(debug_assertions)]
            None
        }
    };

    // Combine all REST routers and the consolidated gRPC router
    let mut api_router = translations_rest.merge(management_rest).merge(grpc_router);

    // Conditionally map the authentication middleware intercepting any access
    if let Some(api_key) = expected_api_key {
        api_router = api_router
            .route_layer(axum::middleware::from_fn_with_state(api_key, auth::validate_api_key));
    }

    // Prepare and configure Swagger UI for API documentation.
    // We load the statically generated Swagger JSON files for the translations
    // and management APIs. These are then served via an interactive Swagger UI
    // mounted at the `/open-api` route.
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

    // Merge API definitions (secured above) alongside Public Swagger
    api_router
        .merge(swagger_ui)
        .layer(TraceLayer::new_for_http())
}
