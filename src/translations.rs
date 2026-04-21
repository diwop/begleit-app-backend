use axum::Router;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

// ----------------- gRPC Service -----------------

// Import the generated proto code
#[allow(clippy::module_inception)]
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

#[derive(Clone, Default)]
pub struct MockTranslations {}

impl MockTranslations {
    /// Creates a new MockTranslations instance.
    pub fn new() -> Self {
        Self::default()
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

use tekken::config::{ModelData, TokenizerVersion};
use tekken::Tekkenizer;

pub fn load_tekkenizer() -> Result<Tekkenizer, Box<dyn std::error::Error + Send + Sync>> {
    // Taken from https://huggingface.co/mistralai/Mistral-Small-4-119B-2603-NVFP4/blob/main/tekken.json
    let content = include_str!("../tekken-mistral-4-small.json");
    let model_data: ModelData = serde_json::from_str(content)?;

    // Map newer versions (like v15) seamlessly down to V13 compatible bounds inherently
    let version =
        TokenizerVersion::from_string(&model_data.config.version).unwrap_or(TokenizerVersion::V13);

    let special_tokens =
        model_data
            .special_tokens
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "Missing special tokens in JSON configuration payload".into()
            })?;

    Tekkenizer::new(
        model_data.vocab,
        &special_tokens,
        &model_data.config.pattern,
        model_data.config.default_vocab_size,
        model_data.config.default_num_special_tokens,
        version,
        model_data.audio,
    )
    .map_err(|e| {
        format!(
            "Failed to create Tekkenizer backend mapping bounds: {:?}",
            e
        )
        .into()
    })
}

#[derive(Clone)]
pub struct LLMTranslations {
    tokenizer: std::sync::Arc<Tekkenizer>,
}

impl LLMTranslations {
    /// Creates a new LLMTranslations instance, verifying that the
    /// required OpenAI-compatible server URL is provided in the environment.
    /// It statically embeds the Tekkenizer configuration directly into the compiled binary.
    /// Returns an error if the environment variables are not set correctly or parsing fails.
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let _url = std::env::var("OPENAI_BASE_URL")?;

        let tokenizer = load_tekkenizer()?;

        Ok(Self {
            tokenizer: std::sync::Arc::new(tokenizer),
        })
    }
}

// TODO: call  LLM API
#[tonic::async_trait]
impl Translations for LLMTranslations {
    #[tracing::instrument(skip_all, fields(input_tokens, output_tokens, token_ratio))]
    async fn translate(
        &self,
        request: Request<TranslateRequest>,
    ) -> Result<Response<TranslateResponse>, Status> {
        debug!("Handling a translation request.");
        let request = request.into_inner();

        let token_count = self
            .tokenizer
            .encode(&request.original, false, false)
            .map(|t: Vec<u32>| t.len())
            .unwrap_or(0);

        let output_tokens = token_count; // Mock output token counts mimicking exact inputs
        let token_ratio: f64 = 1.0;

        let span = tracing::Span::current();
        span.record("input_tokens", token_count);
        span.record("output_tokens", output_tokens);
        span.record("token_ratio", token_ratio);

        // Artificial async delay simulating Mistral backend token decoding execution (1ms per input token)
        tokio::time::sleep(tokio::time::Duration::from_millis(token_count as u64)).await;

        let translated = request.original.clone();

        info!("Handled a translation request payload with {} characters evaluating to {} Mistral tokens.", request.original.len(), token_count);

        Ok(Response::new(TranslateResponse { translated }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tekkenizer_loads_and_tokenizes() {
        let tokenizer = load_tekkenizer().expect("Tekkenizer must load and parse successfully");
        let text = "Hello world, testing the Mistral tokenizer.";

        let tokens = tokenizer
            .encode(text, false, false)
            .expect("Must continuously resolve bytes natively");
        assert!(
            !tokens.is_empty(),
            "Tokenizer failed to produce any tokens for standard input"
        );

        // Assert native bounds
        assert!(
            tokens.len() > 3,
            "Should evaluate to multiple sequence nodes"
        );
    }
}
