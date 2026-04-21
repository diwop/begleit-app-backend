use axum::Router;
use tonic::{Request, Response, Status};
use tracing::{debug, info};


// ----------------- gRPC Service -----------------

// Import the generated proto code
pub mod translations {
    tonic::include_proto!("translations");
}

use translations::translations_server::{Translations, TranslationsServer};
use translations::{TranslateRequest, TranslateResponse};

#[derive(Clone)]
pub enum TranslationsImpl {
    Mock(MockTranslations),
    LLM(LLMTranslations),
}

#[tonic::async_trait]
impl Translations for TranslationsImpl {
    async fn translate(
        &self,
        request: Request<TranslateRequest>,
    ) -> Result<Response<TranslateResponse>, Status> {
        match self {
            Self::Mock(m) => m.translate(request).await,
            Self::LLM(l) => l.translate(request).await,
        }
    }
}

// ----------------- Router Setup -----------------

pub fn setup() -> (Router, TranslationsServer<TranslationsImpl>) {
    let translations = match LLMTranslations::new() {
        Ok(llm) => {
            info!("Successfully initialized LLM translations backend.");
            TranslationsImpl::LLM(llm)
        }
        Err(err) => {
            #[cfg(debug_assertions)]
            {
                debug!("Could not initialize LLM translations backend ({}). Using mock implementation.", err);
                TranslationsImpl::Mock(MockTranslations::new())
            }
            #[cfg(not(debug_assertions))]
            {
                panic!("Failed to initialize LLM translations backend: {}", err);
            }
        }
    };

    let rest_router = crate::route_grpc_json_service!(
        Router::new(),
        "/translations",
        translations.clone(),
        TranslationsImpl,
        [translate]
    );

    let grpc_service = TranslationsServer::new(translations);

    (rest_router, grpc_service)
}

// ------------------ Mock Impl -------------------

#[derive(Clone)]
pub struct MockTranslations {}

impl MockTranslations {
    /// Creates a new MockTranslations instance.
    pub fn new() -> Self {
        Self {}
    }
}

// the mock implementation always returns the input as translation
#[tonic::async_trait]
impl Translations for MockTranslations {
    async fn translate(
        &self,
        request: Request<TranslateRequest>,
    ) -> Result<Response<TranslateResponse>, Status> {
        let request = request.into_inner();
        let translated = request.original;
        Ok(Response::new(TranslateResponse { translated }))
    }
}

// ------------------ LLM Impl -------------------

#[derive(Clone)]
pub struct LLMTranslations {}

impl LLMTranslations {
    /// Creates a new LLMTranslations instance, verifying that the
    /// required OpenAI-compatible server URL is provided in the environment.
    /// Returns an error if the environment variables are not set correctly.
    pub fn new() -> Result<Self, std::env::VarError> {
        let _url = std::env::var("OPENAI_BASE_URL")?;
        Ok(Self {})
    }
}

// TODO: call LLM API
#[tonic::async_trait]
impl Translations for LLMTranslations {
    async fn translate(
        &self,
        request: Request<TranslateRequest>,
    ) -> Result<Response<TranslateResponse>, Status> {
        debug!("Handling a translation request.");
        let request = request.into_inner();
        let translated = request.original;
        info!("Handled a translation request.");
        Ok(Response::new(TranslateResponse { translated }))
    }
}