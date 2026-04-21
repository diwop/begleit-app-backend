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

    let is_secured = expected_api_key.is_some();

    // Conditionally map the authentication middleware intercepting any access
    if let Some(api_key) = expected_api_key {
        api_router = api_router.route_layer(axum::middleware::from_fn_with_state(
            api_key,
            auth::validate_api_key,
        ));
    }

    let swagger_ui = configure_swagger_ui(is_secured);

    // Merge API definitions (secured above) alongside Public Swagger
    api_router
        .merge(swagger_ui)
        .layer(TraceLayer::new_for_http())
}

/// Helper function to configure the Swagger UI and apply dynamic security definitions.
fn configure_swagger_ui(is_secured: bool) -> utoipa_swagger_ui::SwaggerUi {
    let mut translations_swagger_val: serde_json::Value =
        serde_json::from_str(include_str!("../api-docs/translations.swagger.json"))
            .expect("Generated Translations API Swagger must be valid JSON");

    let mut management_swagger_val: serde_json::Value =
        serde_json::from_str(include_str!("../api-docs/management.swagger.json"))
            .expect("Generated Management API Swagger must be valid JSON");

    let pkg_version = env!("CARGO_PKG_VERSION");

    let security_definitions = if is_secured {
        Some(serde_json::json!({
            "ApiKeyAuth": {
                "type": "apiKey",
                "in": "header",
                "name": "x-api-key"
            }
        }))
    } else {
        None
    };

    let security_global = if is_secured {
        Some(serde_json::json!([{ "ApiKeyAuth": [] }]))
    } else {
        None
    };

    for swagger_val in [&mut translations_swagger_val, &mut management_swagger_val] {
        if let Some(obj) = swagger_val.as_object_mut() {
            // Unconditionally set the API metadata version mirroring Cargo.toml
            if let Some(info) = obj.get_mut("info").and_then(|i| i.as_object_mut()) {
                info.insert("version".to_string(), serde_json::json!(pkg_version));
            }

            if is_secured {
                if let Some(ref sec_def) = security_definitions {
                    obj.insert("securityDefinitions".to_string(), sec_def.clone());
                }
                if let Some(ref sec_global) = security_global {
                    obj.insert("security".to_string(), sec_global.clone());
                }
            }
        }
    }

    utoipa_swagger_ui::SwaggerUi::new("/open-api")
        .external_url_unchecked("/api-docs/translations.json", translations_swagger_val)
        .external_url_unchecked("/api-docs/management.json", management_swagger_val)
}
