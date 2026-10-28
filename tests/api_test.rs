use diwop_begleitapp::{
    create_app, ConversationMessage, CorrectTranscriptResponse, ErrorResponse, Role,
    TranslateRequest, TranslationResponse, TranslationType, Warning, WarningType,
};
use tokio::net::TcpListener;

/// Starts an ephemeral test server running the application router and returns its base URL.
async fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app();

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
    // Ensure `warning` is omitted (not `null`) when None
    assert!(raw.get("warning").is_none());

    let body: TranslationResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(
        body,
        TranslationResponse {
            r#type: TranslationType::Formatting,
            content: "sie müssen den antrag bis ende nächste woche abgeben".to_string(),
            warning: None,
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
        }
    );

    // 2. Staging environment honors directives
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
            content: msg.to_string(),
            warning: Some(Warning {
                headline: "Zu Ihrer Information".to_string(),
                body: "Passen Sie auf sich auf!".to_string(),
                r#type: WarningType::Warning,
                revealable: true,
            }),
        }
    );
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
