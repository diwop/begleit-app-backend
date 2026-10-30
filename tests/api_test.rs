use diwop_begleitapp::{
    create_app, create_app_with_runner_and_readiness, ConversationMessage,
    CorrectTranscriptResponse, EchoPromptRunner, ErrorResponse, PromptError, PromptOutput,
    PromptRunner, Role, TokenUsage, TranslateRequest, TranslationResponse, TranslationType,
    Warning, WarningType,
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
}

/// Configurable mock [`PromptRunner`] that records calls and returns a canned response.
struct MockPromptRunner {
    calls: Mutex<Vec<RecordedCall>>,
    response: Result<String, String>,
}

impl MockPromptRunner {
    /// Creates a new [`MockPromptRunner`] that returns `response` on every invocation.
    fn new(response: Result<String, String>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            response,
        })
    }

    /// Returns a snapshot of all recorded [`PromptRunner::run_prompt`] calls.
    fn recorded_calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl PromptRunner for MockPromptRunner {
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        self.calls.lock().unwrap().push(RecordedCall {
            system_prompt: system_prompt.to_string(),
            prompt: prompt.to_string(),
            thinking_level: thinking_level.to_string(),
        });
        let res = self
            .response
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
            .map_err(PromptError);
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

/// Verifies that `POST /correct-transcript` and `POST /translate` render prompts, parse model outputs, and map runner errors to 503.
#[tokio::test]
async fn test_prompt_runner_integration_and_error_handling() {
    let client = reqwest::Client::new();

    // 1. /correct-transcript with MockPromptRunner
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
    assert_eq!(calls[0].thinking_level, "low");
    assert!(calls[0]
        .prompt
        .contains("sie müssen den antrag bis ende nächste woche abgeben"));

    // 2. /translate with MockPromptRunner returning ÜBERSETZUNG
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
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].thinking_level, "medium");
    assert!(calls[0]
        .prompt
        .contains("User: wann muss ich den antrag abgeben"));
    assert!(calls[0]
        .prompt
        .contains("Hier sind Beispiele für Übersetzungen von Dialogen in Leichte Sprache:"));
    assert!(calls[0]
        .prompt
        .contains("sie müssen den antrag bis ende nächste woche abgeben"));

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

/// Verifies that `GET /healthz` returns `200 OK`.
#[tokio::test]
async fn test_healthz_endpoint() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/healthz", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
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
/// `Partner (Leichte Sprache): ...`, injects dynamic few-shot examples from the matching bucket,
/// and rejects `partner_simple` when it is the final message.
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
    assert_eq!(calls.len(), 1);
    assert!(calls[0].prompt.contains(
        "Partner: Guten Tag, Sie möchten Ihren Reisepass abholen?\n\nPartner (Leichte Sprache): Guten Tag!\nWollen Sie Ihren **Reisepass** abholen?\n\nUser: Ja, genau."
    ));
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

/// Verifies that `debugInfo` is omitted by default and populated on `/correct-transcript`
/// and `/translate` only when a valid admin password or `admin_session` cookie is sent.
#[tokio::test]
async fn test_debug_info_populated_only_with_valid_admin_token() {
    let client = reqwest::Client::new();
    let mock_runner = MockPromptRunner::new(Ok(
        "FORMATIERUNG\nBringen Sie **Ihren Ausweis** mit.".to_string()
    ));
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

    // 2. /correct-transcript with Bearer admin password -> debugInfo populated, examples omitted
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

    // 3. /translate with admin_session cookie -> debugInfo populated with few-shot examples
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
}
