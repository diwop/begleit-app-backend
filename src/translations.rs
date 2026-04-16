use axum::Router;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

fn translate(original: &str) -> String {
    original.to_string()
}

// ----------------- gRPC Service -----------------

// Import the generated proto code
pub mod translations {
    tonic::include_proto!("translations");
}

use translations::translations_server::{Translations, TranslationsServer};
use translations::{TranslateRequest, TranslateResponse};

#[derive(Default, Clone)]
pub struct TranslationsImpl {}

#[tonic::async_trait]
impl Translations for TranslationsImpl {
    async fn translate(
        &self,
        request: Request<TranslateRequest>,
    ) -> Result<Response<TranslateResponse>, Status> {
        debug!("Handling a translation request.");
        let request = request.into_inner();
        let translated = translate(&request.original);
        info!("Handled a translation request.");
        Ok(Response::new(TranslateResponse { translated }))
    }
}

// ----------------- Router Setup -----------------

pub fn setup() -> (Router, TranslationsServer<TranslationsImpl>) {
    let translations = TranslationsImpl::default();

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

