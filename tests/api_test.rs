use diwop_begleitapp::{
    create_app, create_app_with_runner_and_readiness, ClassificationCategory, ClassificationLevel,
    ConversationMessage, CorrectTranscriptResponse, DialogTopic, EchoPromptRunner, ErrorResponse,
    PromptError, PromptOutput, PromptRunner, Role, TokenUsage, TranslateRequest,
    TranslationResponse, TranslationType, Warning, WarningType,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

/// Captured invocation arguments passed to [`MockPromptRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedCall {
    system_prompt: String,
    prompt: String,
    thinking_level: String,
    has_json_schema: bool,
}

/// Configurable mock [`PromptRunner`] that records calls and returns canned responses for primary and classification prompts.
struct MockPromptRunner {
    calls: Mutex<Vec<RecordedCall>>,
    response: Result<String, String>,
    classify_response: Result<String, String>,
}

impl MockPromptRunner {
    /// Creates a new [`MockPromptRunner`] that returns `response` for primary prompts and a default
    /// `pass`/`ok`/`other` classification response (or propagates `Err` if `response` is `Err`).
    fn new(response: Result<String, String>) -> Arc<Self> {
        let classify_response = match &response {
            Ok(_) => Ok(r#"{"type":"pass","category":"ok","topic":"other"}"#.to_string()),
            Err(e) => Err(e.clone()),
        };
        Self::with_classification(response, classify_response)
    }

    /// Creates a new [`MockPromptRunner`] with explicit canned responses for primary prompts and dialog classification.
    fn with_classification(
        response: Result<String, String>,
        classify_response: Result<String, String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            response,
            classify_response,
        })
    }

    /// Returns a snapshot of all recorded [`PromptRunner`] calls.
    fn recorded_calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Records a call and produces the configured [`PromptOutput`] or [`PromptError`].
    fn handle_call(
        &self,
        system_prompt: &str,
        prompt: &str,
        thinking_level: &str,
        has_json_schema: bool,
    ) -> Result<PromptOutput, PromptError> {
        self.calls.lock().unwrap().push(RecordedCall {
            system_prompt: system_prompt.to_string(),
            prompt: prompt.to_string(),
            thinking_level: thinking_level.to_string(),
            has_json_schema,
        });

        let selected =
            if system_prompt == diwop_begleitapp::prompts::CLASSIFY_DIALOG_SYSTEM_PROMPT.as_str() {
                &self.classify_response
            } else {
                &self.response
            };

        selected
            .clone()
            .map(|text| PromptOutput {
                text,
                reasoning: Some("Mock reasoning".to_string()),
                usage: TokenUsage {
                    input_tokens: 10,
                    reasoning_tokens: 5,
                    output_tokens: 15,
                },
            })
            .map_err(PromptError)
    }
}

impl PromptRunner for MockPromptRunner {
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        let res = self.handle_call(system_prompt, prompt, thinking_level, false);
        Box::pin(async move { res })
    }

    fn run_prompt_with_schema<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
        _json_schema: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        let res = self.handle_call(system_prompt, prompt, thinking_level, true);
        Box::pin(async move { res })
    }
}

/// Starts an ephemeral test server running the application router with [`EchoPromptRunner`] and returns its base URL.
async fn start_server() -> String {
    std::env::remove_var("GEMINI_API_KEY");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    url
}

/// Starts an ephemeral test server with a custom [`PromptRunner`] and returns its base URL.
async fn start_server_with_runner(runner: Arc<dyn PromptRunner>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app_with_runner_and_readiness(runner, Arc::new(AtomicBool::new(true)));

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    url
}

/// Starts an ephemeral test server with a custom [`AtomicBool`] readiness flag and returns its base URL.
async fn start_server_with_readiness(readiness: Arc<AtomicBool>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app_with_runner_and_readiness(Arc::new(EchoPromptRunner), readiness);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    url
}

/// Verifies that `POST /correct-transcript` returns the incoming message unchanged for valid `user` and `partner` inputs.
#[tokio::test]
async fn test_correct_transcript_echoes_message() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    for role in [Role::User, Role::Partner] {
        let input = "sie müssen den antrag bis ende nächste woche abgeben";
        let res = client
            .post(format!("{}/correct-transcript", base_url))
            .json(&ConversationMessage {
                role,
                message: input.to_string(),
            })
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let body: CorrectTranscriptResponse = res.json().await.unwrap();
        assert_eq!(body.message, input);
    }
}

/// Verifies that `POST /correct-transcript` rejects blank messages, invalid roles, and invalid JSON.
#[tokio::test]
async fn test_correct_transcript_validation_and_invalid_json() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    // Blank message -> 422 VALIDATION_ERROR
    let res = client
        .post(format!("{}/correct-transcript", base_url))
        .json(&serde_json::json!({
            "role": "user",
            "message": "   "
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "VALIDATION_ERROR");

    // Invalid role -> 422 VALIDATION_ERROR
    let res = client
        .post(format!("{}/correct-transcript", base_url))
        .json(&serde_json::json!({
            "role": "assistant",
            "message": "Hallo"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "VALIDATION_ERROR");

    // Malformed JSON -> 400 INVALID_JSON
    let res = client
        .post(format!("{}/correct-transcript", base_url))
        .header("content-type", "application/json")
        .body("{not-valid-json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "INVALID_JSON");
}

/// Verifies that `POST /translate` returns the last message's content as-is when its role is `partner`.
#[tokio::test]
async fn test_translate_echoes_last_partner_message() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .post(format!("{}/translate", base_url))
        .json(&TranslateRequest {
            messages: vec![
                ConversationMessage {
                    role: Role::User,
                    message: "wann muss ich den antrag abgeben".to_string(),
                },
                ConversationMessage {
                    role: Role::Partner,
                    message: "sie müssen den antrag bis ende nächste woche abgeben".to_string(),
                },
            ],
        })
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let raw: serde_json::Value = res.json().await.unwrap();
    // Ensure `warning` and `debugInfo` are omitted (not `null`) when None
    assert!(raw.get("warning").is_none());
    assert!(raw.get("debugInfo").is_none());

    let body: TranslationResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(
        body,
        TranslationResponse {
            r#type: TranslationType::Formatting,
            content: "sie müssen den antrag bis ende nächste woche abgeben".to_string(),
            warning: None,
            debug_info: None,
        }
    );
}

/// Verifies that `POST /translate` returns 422 with appropriate error codes for empty messages, blank text, or non-partner last message.
#[tokio::test]
async fn test_translate_validation_errors() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    // Empty message list -> 422 VALIDATION_ERROR
    let res = client
        .post(format!("{}/translate", base_url))
        .json(&TranslateRequest { messages: vec![] })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "VALIDATION_ERROR");

    // Blank message in list -> 422 VALIDATION_ERROR
    let res = client
        .post(format!("{}/translate", base_url))
        .json(&TranslateRequest {
            messages: vec![ConversationMessage {
                role: Role::Partner,
                message: "  ".to_string(),
            }],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "VALIDATION_ERROR");

    // Last message is user -> 422 LAST_MESSAGE_MUST_BE_PARTNER
    let res = client
        .post(format!("{}/translate", base_url))
        .json(&TranslateRequest {
            messages: vec![
                ConversationMessage {
                    role: Role::Partner,
                    message: "Hallo".to_string(),
                },
                ConversationMessage {
                    role: Role::User,
                    message: "wann muss ich den antrag abgeben".to_string(),
                },
            ],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "LAST_MESSAGE_MUST_BE_PARTNER");
}

/// Verifies that `POST /translate` ignores `%...%` markers by default (`production`) and honors them when `ENVIRONMENT=staging`.
#[tokio::test]
async fn test_translate_directives_for_type_and_warning() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let msg = "Bitte beachten %type=translation% %warning.type=warning% %warning.revealable%";

    // 1. Default environment (production) ignores directives
    std::env::remove_var("ENVIRONMENT");
    let res = client
        .post(format!("{}/translate", base_url))
        .json(&TranslateRequest {
            messages: vec![ConversationMessage {
                role: Role::Partner,
                message: msg.to_string(),
            }],
        })
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: TranslationResponse = res.json().await.unwrap();
    assert_eq!(
        body,
        TranslationResponse {
            r#type: TranslationType::Formatting,
            content: msg.to_string(),
            warning: None,
            debug_info: None,
        }
    );

    // 2. Staging environment honors directives and strips them from the prompt/content
    std::env::set_var("ENVIRONMENT", "staging");
    let res = client
        .post(format!("{}/translate", base_url))
        .json(&TranslateRequest {
            messages: vec![ConversationMessage {
                role: Role::Partner,
                message: msg.to_string(),
            }],
        })
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: TranslationResponse = res.json().await.unwrap();
    assert_eq!(
        body,
        TranslationResponse {
            r#type: TranslationType::Translation,
            content: "Bitte beachten".to_string(),
            warning: Some(Warning {
                headline: "Zu Ihrer Information".to_string(),
                body: "Passen Sie auf sich auf!".to_string(),
                r#type: WarningType::Warning,
                revealable: true,
            }),
            debug_info: None,
        }
    );
}

/// Verifies that `POST /correct-transcript` (`"minimal"` reasoning) and `POST /translate`
/// (`"medium"` translation followed by `"low"` structured JSON classification) render prompts,
/// parse model outputs, and map runner errors to 503.
#[tokio::test]
async fn test_prompt_runner_integration_and_error_handling() {
    let client = reqwest::Client::new();

    // 1. /correct-transcript with MockPromptRunner ("minimal" thinking)
    let mock_correct = MockPromptRunner::new(Ok(
        "Sie müssen den Antrag bis Ende nächster Woche abgeben.".to_string(),
    ));
    let url_correct = start_server_with_runner(mock_correct.clone()).await;

    let res = client
        .post(format!("{}/correct-transcript", url_correct))
        .json(&ConversationMessage {
            role: Role::Partner,
            message: "sie müssen den antrag bis ende nächste woche abgeben".to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: CorrectTranscriptResponse = res.json().await.unwrap();
    assert_eq!(
        body.message,
        "Sie müssen den Antrag bis Ende nächster Woche abgeben."
    );

    let calls = mock_correct.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].thinking_level, "minimal");
    assert!(!calls[0].has_json_schema);
    assert!(calls[0]
        .prompt
        .contains("sie müssen den antrag bis ende nächste woche abgeben"));

    // 2. /translate with MockPromptRunner returning ÜBERSETZUNG + pass/ok classification
    let mock_translate = MockPromptRunner::new(Ok(
        "ÜBERSETZUNG\nGeben Sie den Antrag **bis Ende nächster Woche** ab.".to_string(),
    ));
    let url_translate = start_server_with_runner(mock_translate.clone()).await;

    std::env::remove_var("ENVIRONMENT");
    let res = client
        .post(format!("{}/translate", url_translate))
        .json(&TranslateRequest {
            messages: vec![
                ConversationMessage {
                    role: Role::User,
                    message: "wann muss ich den antrag abgeben".to_string(),
                },
                ConversationMessage {
                    role: Role::Partner,
                    message: "sie müssen den antrag bis ende nächste woche abgeben".to_string(),
                },
            ],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: TranslationResponse = res.json().await.unwrap();
    assert_eq!(
        body,
        TranslationResponse {
            r#type: TranslationType::Translation,
            content: "Geben Sie den Antrag **bis Ende nächster Woche** ab.".to_string(),
            warning: None,
            debug_info: None,
        }
    );

    let calls = mock_translate.recorded_calls();
    assert_eq!(calls.len(), 2);
    // Step 1: translate ("medium", free-text header + markdown)
    assert_eq!(calls[0].thinking_level, "medium");
    assert!(!calls[0].has_json_schema);
    assert!(calls[0]
        .prompt
        .contains("User: wann muss ich den antrag abgeben"));
    assert!(calls[0]
        .prompt
        .contains("Hier sind Beispiele für Übersetzungen von Dialogen in Leichte Sprache:"));
    assert!(calls[0]
        .prompt
        .contains("sie müssen den antrag bis ende nächste woche abgeben"));

    // Step 2: classify-dialog ("low", structured JSON schema)
    assert_eq!(calls[1].thinking_level, "low");
    assert!(calls[1].has_json_schema);
    assert!(calls[1]
        .prompt
        .contains("User: wann muss ich den antrag abgeben"));
    assert!(calls[1]
        .prompt
        .contains("```input\nsie müssen den antrag bis ende nächste woche abgeben\n```"));
    assert!(calls[1]
        .prompt
        .contains("```translation\nGeben Sie den Antrag **bis Ende nächster Woche** ab.\n```"));

    // 3. Model error -> 503 SERVICE_UNAVAILABLE
    let mock_err = MockPromptRunner::new(Err("Gemini timeout".to_string()));
    let url_err = start_server_with_runner(mock_err).await;

    let res = client
        .post(format!("{}/correct-transcript", url_err))
        .json(&ConversationMessage {
            role: Role::User,
            message: "Hallo".to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "SERVICE_UNAVAILABLE");

    let res = client
        .post(format!("{}/translate", url_err))
        .json(&TranslateRequest {
            messages: vec![ConversationMessage {
                role: Role::Partner,
                message: "Hallo".to_string(),
            }],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let err: ErrorResponse = res.json().await.unwrap();
    assert_eq!(err.error.code, "SERVICE_UNAVAILABLE");
}

/// Verifies `POST /translate` warning boxes, `revealable` gating, empty-answer blocking for
/// `safety` and `youth-protection`, and fail-closed `503 SERVICE_UNAVAILABLE` when classification fails.
#[tokio::test]
async fn test_translate_classification_categories_and_fail_closed() {
    std::env::remove_var("ENVIRONMENT");
    let client = reqwest::Client::new();
    let sample_req = TranslateRequest {
        messages: vec![ConversationMessage {
            role: Role::Partner,
            message: "Beispielaussage des Gesprächspartners.".to_string(),
        }],
    };

    // 1. info / uncertain -> revealable: false, content preserved
    let mock_info = MockPromptRunner::with_classification(
        Ok("ÜBERSETZUNG\nVielleicht meint die Person das so.".to_string()),
        Ok(r#"{"type":"info","category":"uncertain","topic":"shopping"}"#.to_string()),
    );
    let url_info = start_server_with_runner(mock_info).await;
    let body_info: TranslationResponse = client
        .post(format!("{}/translate", url_info))
        .json(&sample_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body_info.content, "Vielleicht meint die Person das so.");
    let w_info = body_info.warning.expect("Expected info warning");
    assert_eq!(w_info.r#type, WarningType::Info);
    assert!(!w_info.revealable);
    assert_eq!(w_info.headline, "Unsichere Übersetzung");

    // 2. warning / juridical -> revealable: true, content preserved
    let mock_warn = MockPromptRunner::with_classification(
        Ok("ÜBERSETZUNG\nDas bedeutet eine gesetzliche Frist.".to_string()),
        Ok(r#"{"type":"warning","category":"juridical","topic":"juridical"}"#.to_string()),
    );
    let url_warn = start_server_with_runner(mock_warn).await;
    let body_warn: TranslationResponse = client
        .post(format!("{}/translate", url_warn))
        .json(&sample_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body_warn.content, "Das bedeutet eine gesetzliche Frist.");
    let w_warn = body_warn.warning.expect("Expected juridical warning");
    assert_eq!(w_warn.r#type, WarningType::Warning);
    assert!(w_warn.revealable);
    assert_eq!(w_warn.headline, "Hinweis zu Rechts-Themen");

    // 3. danger / hate-speech -> revealable: true, content preserved
    let mock_hate = MockPromptRunner::with_classification(
        Ok("ÜBERSETZUNG\nBeleidigende Aussage.".to_string()),
        Ok(r#"{"type":"danger","category":"hate-speech","topic":"social"}"#.to_string()),
    );
    let url_hate = start_server_with_runner(mock_hate).await;
    let body_hate: TranslationResponse = client
        .post(format!("{}/translate", url_hate))
        .json(&sample_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body_hate.content, "Beleidigende Aussage.");
    let w_hate = body_hate.warning.expect("Expected hate-speech danger box");
    assert_eq!(w_hate.r#type, WarningType::Danger);
    assert!(w_hate.revealable);
    assert_eq!(w_hate.headline, "Verletzende Sprache erkannt");

    // 4. danger / safety -> blocked empty content "", forced TranslationType::Translation, revealable: false
    let mock_safety = MockPromptRunner::with_classification(
        Ok("FORMATIERUNG\nGefährliche Anleitung.".to_string()),
        Ok(r#"{"type":"danger","category":"safety","topic":"emergency"}"#.to_string()),
    );
    let url_safety = start_server_with_runner(mock_safety).await;
    let body_safety: TranslationResponse = client
        .post(format!("{}/translate", url_safety))
        .json(&sample_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body_safety.r#type, TranslationType::Translation);
    assert_eq!(body_safety.content, "");
    let w_safety = body_safety.warning.expect("Expected safety danger box");
    assert_eq!(w_safety.r#type, WarningType::Danger);
    assert!(!w_safety.revealable);
    assert_eq!(
        w_safety.headline,
        "Aussage aus Sicherheits-Gründen blockiert"
    );

    // 5. danger / youth-protection -> blocked empty content "", revealable: false
    let mock_youth = MockPromptRunner::with_classification(
        Ok("ÜBERSETZUNG\nUngeeigneter Inhalt.".to_string()),
        Ok(r#"{"type":"danger","category":"youth-protection","topic":"social"}"#.to_string()),
    );
    let url_youth = start_server_with_runner(mock_youth).await;
    let body_youth: TranslationResponse = client
        .post(format!("{}/translate", url_youth))
        .json(&sample_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body_youth.content, "");
    let w_youth = body_youth
        .warning
        .expect("Expected youth-protection danger box");
    assert_eq!(w_youth.r#type, WarningType::Danger);
    assert!(!w_youth.revealable);
    assert_eq!(w_youth.headline, "Aussage zum Jugend-Schutz blockiert");

    // 6. Classification LLM failure -> fail closed with 503 SERVICE_UNAVAILABLE
    let mock_cls_err = MockPromptRunner::with_classification(
        Ok("ÜBERSETZUNG\nNormaler Text.".to_string()),
        Err("Classification model timeout".to_string()),
    );
    let url_cls_err = start_server_with_runner(mock_cls_err).await;
    let res_cls_err = client
        .post(format!("{}/translate", url_cls_err))
        .json(&sample_req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_cls_err.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );

    // 7. Classification invalid JSON / mismatched hierarchy -> fail closed with 503 SERVICE_UNAVAILABLE
    let mock_bad_json = MockPromptRunner::with_classification(
        Ok("ÜBERSETZUNG\nNormaler Text.".to_string()),
        Ok(r#"{"type":"pass","category":"safety","topic":"other"}"#.to_string()),
    );
    let url_bad_json = start_server_with_runner(mock_bad_json).await;
    let res_bad_json = client
        .post(format!("{}/translate", url_bad_json))
        .json(&sample_req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_bad_json.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
}

/// Verifies that `GET /healthz` and `GET /api-docs/openapi.json` return `200 OK` with registered classification schemas.
#[tokio::test]
async fn test_healthz_and_openapi_endpoints() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/healthz", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let openapi_res = client
        .get(format!("{}/api-docs/openapi.json", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(openapi_res.status(), reqwest::StatusCode::OK);
    let openapi_json = openapi_res.text().await.unwrap();
    assert!(openapi_json.contains("ClassificationDebugInfo"));
    assert!(openapi_json.contains("DialogTopic"));
}

/// Verifies that `GET /readyz` returns `200 OK` when the readiness flag is `true`.
#[tokio::test]
async fn test_readyz_endpoint_when_ready() {
    let readiness = Arc::new(AtomicBool::new(true));
    let base_url = start_server_with_readiness(readiness).await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/readyz", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
}

/// Verifies that `GET /readyz` returns `503 Service Unavailable` when the readiness flag is `false`.
#[tokio::test]
async fn test_readyz_endpoint_when_unready() {
    let readiness = Arc::new(AtomicBool::new(false));
    let base_url = start_server_with_readiness(readiness).await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/readyz", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
}

/// Verifies that `GET /readyz` transitions from initial state to `200 OK` once background warmup completes.
#[tokio::test]
async fn test_readyz_startup_warmup_transition() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    // Poll /readyz until background warmup completes (should be almost immediate with EchoPromptRunner)
    let mut became_ready = false;
    for _ in 0..20 {
        let res = client
            .get(format!("{}/readyz", base_url))
            .send()
            .await
            .unwrap();
        if res.status() == reqwest::StatusCode::OK {
            became_ready = true;
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    assert!(
        became_ready,
        "Server failed to become ready via background warmup probe"
    );
}

/// Verifies that `POST /translate` supports `partner_simple` in history, formats it as
/// `Partner (Leichte Sprache): ...` in both translation and classification prompts,
/// injects dynamic few-shot examples from the matching bucket, and rejects `partner_simple` as the final message.
#[tokio::test]
async fn test_translate_partner_simple_role_and_dynamic_few_shots() {
    let client = reqwest::Client::new();
    let mock_translate = MockPromptRunner::new(Ok(
        "FORMATIERUNG\nBringen Sie **Ihren Ausweis** mit.".to_string(),
    ));
    let base_url = start_server_with_runner(mock_translate.clone()).await;

    let res = client
        .post(format!("{}/translate", base_url))
        .json(&serde_json::json!({
            "messages": [
                {
                    "role": "partner",
                    "message": "Guten Tag, Sie möchten Ihren Reisepass abholen?"
                },
                {
                    "role": "partner_simple",
                    "message": "Guten Tag!\nWollen Sie Ihren **Reisepass** abholen?"
                },
                {
                    "role": "user",
                    "message": "Ja, genau."
                },
                {
                    "role": "partner",
                    "message": "Bitte bringen Sie Ihren Ausweis mit."
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: TranslationResponse = res.json().await.unwrap();
    assert_eq!(body.r#type, TranslationType::Formatting);
    assert_eq!(body.content, "Bringen Sie **Ihren Ausweis** mit.");

    let calls = mock_translate.recorded_calls();
    assert_eq!(calls.len(), 2);
    let expected_history = "Partner: Guten Tag, Sie möchten Ihren Reisepass abholen?\n\nPartner (Leichte Sprache): Guten Tag!\nWollen Sie Ihren **Reisepass** abholen?\n\nUser: Ja, genau.";
    assert!(calls[0].prompt.contains(expected_history));
    assert!(calls[1].prompt.contains(expected_history));
    assert!(calls[0]
        .prompt
        .contains("Hier sind Beispiele für Übersetzungen von Dialogen in Leichte Sprache:"));

    // Last message is partner_simple -> 422 LAST_MESSAGE_MUST_BE_PARTNER
    let res_err = client
        .post(format!("{}/translate", base_url))
        .json(&serde_json::json!({
            "messages": [
                {
                    "role": "partner",
                    "message": "Hallo"
                },
                {
                    "role": "partner_simple",
                    "message": "Hallo!"
                }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res_err.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let err: ErrorResponse = res_err.json().await.unwrap();
    assert_eq!(err.error.code, "LAST_MESSAGE_MUST_BE_PARTNER");
}

/// Verifies that `debugInfo` (including `debugInfo.classification` on `/translate`) is omitted by default
/// and populated only when a valid admin password or `admin_session` cookie is sent.
#[tokio::test]
async fn test_debug_info_populated_only_with_valid_admin_token() {
    std::env::remove_var("ENVIRONMENT");
    let client = reqwest::Client::new();
    let mock_runner = MockPromptRunner::with_classification(
        Ok("FORMATIERUNG\nBringen Sie **Ihren Ausweis** mit.".to_string()),
        Ok(r#"{"type":"warning","category":"authorities","topic":"authorities"}"#.to_string()),
    );
    let base_url = start_server_with_runner(mock_runner).await;

    // Default debug ADMIN_PASSWORD is "admin"
    let admin_session = diwop_begleitapp::web::generate_session_token("admin");

    // 1. /correct-transcript without admin token -> debugInfo is None
    let res_no_admin = client
        .post(format!("{}/correct-transcript", base_url))
        .json(&ConversationMessage {
            role: Role::Partner,
            message: "bitte bringen sie ihren ausweis mit".to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res_no_admin.status(), reqwest::StatusCode::OK);
    let raw_no_admin: serde_json::Value = res_no_admin.json().await.unwrap();
    assert!(raw_no_admin.get("debugInfo").is_none());

    // 2. /correct-transcript with Bearer admin password -> debugInfo populated, examples & classification omitted
    let res_admin_ct = client
        .post(format!("{}/correct-transcript", base_url))
        .header("Authorization", "Bearer admin")
        .json(&ConversationMessage {
            role: Role::Partner,
            message: "bitte bringen sie ihren ausweis mit".to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res_admin_ct.status(), reqwest::StatusCode::OK);
    let raw_ct: serde_json::Value = res_admin_ct.json().await.unwrap();
    assert!(raw_ct["debugInfo"]["examples"].is_null());
    assert!(raw_ct["debugInfo"]["classification"].is_null());
    let body_ct: CorrectTranscriptResponse = serde_json::from_value(raw_ct).unwrap();
    let dbg_ct = body_ct
        .debug_info
        .expect("Expected debugInfo on admin /correct-transcript");
    assert_eq!(
        dbg_ct.system_prompt,
        diwop_begleitapp::prompts::CORRECT_TRANSCRIPT_SYSTEM_PROMPT
    );
    assert!(dbg_ct
        .prompt
        .contains("bitte bringen sie ihren ausweis mit"));
    assert!(dbg_ct.system_prompt_tokens > 0);
    assert!(dbg_ct.prompt_tokens > 0);
    assert_eq!(dbg_ct.input_tokens, 10);
    assert_eq!(dbg_ct.reasoning_tokens, 5);
    assert_eq!(dbg_ct.response_tokens, 15);
    assert_eq!(dbg_ct.reasoning.as_deref(), Some("Mock reasoning"));
    assert!(dbg_ct.examples.is_none());
    assert!(dbg_ct.classification.is_none());

    // 3. /translate with admin_session cookie -> debugInfo populated with few-shot examples and classification details
    let res_admin_tr = client
        .post(format!("{}/translate", base_url))
        .header("Cookie", format!("admin_session={admin_session}"))
        .json(&TranslateRequest {
            messages: vec![ConversationMessage {
                role: Role::Partner,
                message: "Bitte bringen Sie Ihren Ausweis mit.".to_string(),
            }],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res_admin_tr.status(), reqwest::StatusCode::OK);
    let body_tr: TranslationResponse = res_admin_tr.json().await.unwrap();
    assert_eq!(
        body_tr.warning.as_ref().map(|w| w.headline.as_str()),
        Some("Hinweis zu Amts-Themen")
    );
    let dbg_tr = body_tr
        .debug_info
        .expect("Expected debugInfo on admin /translate");
    assert_eq!(
        dbg_tr.system_prompt,
        diwop_begleitapp::prompts::TRANSLATE_SYSTEM_PROMPT
    );
    assert!(dbg_tr
        .prompt
        .contains("Bitte bringen Sie Ihren Ausweis mit."));
    assert!(dbg_tr.system_prompt_tokens > 0);
    assert!(dbg_tr.prompt_tokens > 0);
    assert_eq!(dbg_tr.input_tokens, 10);
    assert_eq!(dbg_tr.reasoning_tokens, 5);
    assert_eq!(dbg_tr.response_tokens, 15);
    assert_eq!(dbg_tr.reasoning.as_deref(), Some("Mock reasoning"));
    let examples = dbg_tr
        .examples
        .expect("Expected dynamic few-shot examples in translate debugInfo");
    assert_eq!(examples.len(), 3);
    assert!(examples[0].contains("```history"));

    let cls = dbg_tr
        .classification
        .expect("Expected classification debug details on /translate");
    assert_eq!(cls.topic, DialogTopic::Authorities);
    assert_eq!(cls.classification_type, ClassificationLevel::Warning);
    assert_eq!(cls.category, ClassificationCategory::Authorities);
    assert!(cls.system_prompt_tokens > 0);
    assert!(cls.prompt_tokens > 0);
    assert_eq!(cls.input_tokens, 10);
    assert_eq!(cls.reasoning_tokens, 5);
    assert_eq!(cls.response_tokens, 15);
    assert_eq!(cls.reasoning.as_deref(), Some("Mock reasoning"));
    assert!(cls.raw_response.contains("\"authorities\""));
}
