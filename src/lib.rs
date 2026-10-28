pub mod api;
pub mod auth;

pub use api::{
    ConversationMessage, CorrectTranscriptResponse, ErrorDetail, ErrorResponse, Role,
    TranslateRequest, TranslationResponse, TranslationType, Warning, WarningType,
};

use axum::{http::StatusCode, routing::get, Router};
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::OpenApi;

/// Base OpenAPI documentation structure for the DiWoP Conversation API.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "DiWoP Conversation API",
        version = "0.1.0",
        description = "Contract draft for alignment.\nFor partner contributions, both endpoints are called in parallel and independently.\n/correct-transcript is used exclusively for the UI display. /translate receives the originally recognized spoken texts for the entire conversation history.\nAdditional headers and timeouts are configured in the client."
    ),
    paths(api::correct_transcript, api::translate),
    components(schemas(
        Role,
        ConversationMessage,
        CorrectTranscriptResponse,
        TranslateRequest,
        TranslationType,
        WarningType,
        Warning,
        TranslationResponse,
        ErrorDetail,
        ErrorResponse
    ))
)]
struct ApiDoc;

/// Creates and configures the main application router.
///
/// This function aggregates all separate HTTP components of the application into a single
/// `axum::Router`, exposes an unauthenticated `/healthz` endpoint and Swagger UI for OpenAPI
/// documentation, and applies authentication and request tracing middleware.
pub fn create_app() -> Router {
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

    let mut api_router = api::router();

    let is_secured = expected_api_key.is_some();

    // Conditionally map the authentication middleware intercepting any access
    if let Some(api_key) = expected_api_key {
        api_router = api_router.route_layer(axum::middleware::from_fn_with_state(
            api_key,
            auth::validate_api_key,
        ));
    }

    let swagger_ui = configure_swagger_ui(is_secured);
    let health_router = Router::new().route("/healthz", get(|| async { StatusCode::OK }));

    // Merge API definitions (secured above) alongside unauthenticated health check and Public Swagger
    api_router
        .merge(health_router)
        .merge(swagger_ui)
        .layer(TraceLayer::new_for_http())
}

/// Helper function to configure the Swagger UI and apply dynamic security definitions.
fn configure_swagger_ui(is_secured: bool) -> utoipa_swagger_ui::SwaggerUi {
    let mut openapi = ApiDoc::openapi();

    if is_secured {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "Configured API key; whether authentication is required depends on the deployment.",
                    ))
                    .build(),
            ),
        );
        openapi.security = Some(vec![SecurityRequirement::new(
            "bearerAuth",
            Vec::<String>::new(),
        )]);
    }

    utoipa_swagger_ui::SwaggerUi::new("/open-api").url("/api-docs/openapi.json", openapi)
}
