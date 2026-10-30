pub mod api;
pub mod auth;
pub mod few_shots;
pub mod metrics;
pub mod prompts;
pub mod web;

pub use api::{
    ConversationMessage, CorrectTranscriptResponse, DebugInfo, ErrorDetail, ErrorResponse, Role,
    TranslateRequest, TranslationResponse, TranslationType, Warning, WarningType,
};
pub use few_shots::{FewShotBucketIndex, FewShotEntry};
pub use prompts::{
    EchoPromptRunner, GeminiPromptRunner, PromptError, PromptOutput, PromptRunner, TokenUsage,
};

use axum::{http::StatusCode, routing::get, Router};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
        DebugInfo,
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

/// Creates and configures the main application router using the environment-configured [`PromptRunner`].
///
/// In release builds, `GEMINI_API_KEY` is required and configures [`GeminiPromptRunner`].
/// In debug/test builds, falls back to [`EchoPromptRunner`] when `GEMINI_API_KEY` is unset.
///
/// Spawns a background warmup task to verify the LLM API before marking the service as ready on `/readyz`.
pub fn create_app() -> Router {
    let runner: Arc<dyn PromptRunner> = match std::env::var("GEMINI_API_KEY") {
        Ok(key) if !key.trim().is_empty() => Arc::new(GeminiPromptRunner::new(key)),
        _ => {
            #[cfg(not(debug_assertions))]
            panic!("GEMINI_API_KEY environment variable is crucially required in release profile!");

            #[cfg(debug_assertions)]
            Arc::new(EchoPromptRunner)
        }
    };

    create_app_with_runner(runner)
}

/// Creates and configures the main application router with an explicit [`PromptRunner`].
///
/// Spawns a single asynchronous background warmup check against the embedded few-shot vector index
/// and `runner` to verify both semantic lookup and LLM API connectivity.
/// While warmup is in progress or if all attempts fail, `/readyz` returns `503 Service Unavailable`.
/// Once warmup succeeds, the readiness state is cached as `true`, and subsequent `/readyz` queries
/// return `200 OK` in-memory without recurring LLM API calls.
pub fn create_app_with_runner(runner: Arc<dyn PromptRunner>) -> Router {
    let readiness = Arc::new(AtomicBool::new(false));
    let few_shots_res = FewShotBucketIndex::default_shared();

    // Spawn an asynchronous startup warmup probe if running within an active Tokio runtime
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let runner_clone = Arc::clone(&runner);
        let readiness_clone = Arc::clone(&readiness);
        handle.spawn(async move {
            let few_shots = match few_shots_res {
                Ok(idx) => idx,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to load few-shot bucket index during startup warmup. Service remains unready.");
                    return;
                }
            };

            if let Err(e) = few_shots.get_closest_examples("ping", 0, 1) {
                tracing::error!(error = %e, "Few-shot embedding warmup check failed. Service remains unready.");
                return;
            }

            for attempt in 1..=3 {
                tracing::info!(attempt, "Running LLM API startup warmup check for readiness probe...");
                match runner_clone
                    .run_prompt("Du bist ein Test-Assistent.", "ping", "minimal")
                    .await
                {
                    Ok(_) => {
                        tracing::info!("LLM API and few-shot index startup warmup succeeded. Service is ready.");
                        readiness_clone.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(attempt, error = %e, "LLM API startup warmup attempt failed");
                        if attempt < 3 {
                            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        }
                    }
                }
            }
            tracing::error!("LLM API startup warmup failed after 3 attempts. Service remains unready.");
        });
    } else if few_shots_res.is_ok() {
        // Fallback for non-Tokio contexts (e.g. synchronous router instantiation in tests)
        readiness.store(true, Ordering::SeqCst);
    }

    create_app_with_runner_and_readiness(runner, readiness)
}

/// Creates and configures the main application router with an explicit [`PromptRunner`] and [`AtomicBool`] readiness flag.
///
/// This function aggregates all separate HTTP components into a single `axum::Router`:
/// - Exposes an unauthenticated `/healthz` endpoint returning `200 OK` (process liveness probe).
/// - Exposes an unauthenticated `/readyz` endpoint returning `200 OK` if `readiness` is true, or `503 Service Unavailable` if false.
/// - Exposes Swagger UI documentation on `/open-api` and `/api-docs/openapi.json`.
/// - Protects operational endpoints with Bearer authentication if `API_KEY` is configured.
/// - Exposes the administration web interface on `/`, `/login`, `/logout`, and `/static/style.css` protected via `ADMIN_PASSWORD`.
pub fn create_app_with_runner_and_readiness(
    runner: Arc<dyn PromptRunner>,
    readiness: Arc<AtomicBool>,
) -> Router {
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

    // Attempt to extract the Admin Password enforcing production rules
    let admin_password = match std::env::var("ADMIN_PASSWORD") {
        Ok(pw) if !pw.trim().is_empty() => Arc::new(pw),
        _ => {
            #[cfg(not(debug_assertions))]
            panic!("ADMIN_PASSWORD environment variable is crucially required in release profile!");

            #[cfg(debug_assertions)]
            Arc::new("admin".to_string())
        }
    };

    let few_shots = FewShotBucketIndex::default_shared().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Failed to load default few-shot bucket index; using empty index");
        Arc::new(FewShotBucketIndex::new(
            Default::default(),
            Arc::new(crate::few_shots::DeterministicHashEmbedder),
        ))
    });
    let mut api_router = api::router_with_state(runner, few_shots, Arc::clone(&admin_password));

    let is_secured = expected_api_key.is_some();

    // Conditionally map the authentication middleware intercepting any access
    if let Some(ref api_key) = expected_api_key {
        let auth_state = auth::AuthState {
            api_key: Arc::clone(api_key),
            admin_password: Arc::clone(&admin_password),
        };
        api_router = api_router.route_layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth::validate_api_key,
        ));
    }

    // Attach HTTP metrics tracking to api_router to record request count and duration
    api_router = api_router.route_layer(axum::middleware::from_fn(
        crate::metrics::track_http_metrics,
    ));

    let swagger_ui = configure_swagger_ui(is_secured);
    let web_router = web::router(admin_password, expected_api_key.clone());

    let readiness_state = Arc::clone(&readiness);
    let health_router = Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let ready = readiness_state.load(Ordering::SeqCst);
                async move {
                    if ready {
                        StatusCode::OK
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    }
                }
            }),
        );

    // Merge API definitions (secured above) alongside unauthenticated health/readiness checks, Public Swagger, and Web Admin
    api_router
        .merge(health_router)
        .merge(swagger_ui)
        .merge(web_router)
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
