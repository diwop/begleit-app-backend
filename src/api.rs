use crate::few_shots::{FewShotBucketIndex, DEFAULT_FEW_SHOT_K};
use crate::prompts::{
    build_correct_transcript_prompt, build_translate_prompt_with_few_shots, count_history_turns,
    determine_history_bucket, extract_staging_directives, parse_translate_output, PromptRunner,
    CORRECT_TRANSCRIPT_SYSTEM_PROMPT, TRANSLATE_SYSTEM_PROMPT,
};
use axum::{
    extract::{rejection::JsonRejection, FromRequest, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

/// Speaker role in a conversation: `user` is the app user, `partner` is the conversation partner,
/// and `partner_simple` is a previously translated simple-language partner contribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Partner,
    PartnerSimple,
}

/// Spoken contribution in a conversation with speaker role and plain-text message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "role": "partner",
    "message": "sie müssen den antrag bis ende nächste woche abgeben"
}))]
pub struct ConversationMessage {
    /// `user` is the app user, `partner` is the conversation partner, and `partner_simple` is a previously translated simple-language partner contribution.
    pub role: Role,
    /// Originally recognized spoken text as plain text.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "sie müssen den antrag bis ende nächste woche abgeben"
    )]
    pub message: String,
}

/// Optional diagnostic information returned when a request carries a valid admin password or session token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebugInfo {
    /// System prompt preamble sent to the language model.
    pub system_prompt: String,
    /// Rendered user prompt sent to the language model.
    pub prompt: String,
    /// Number of input prompt tokens reported by the language model.
    pub input_tokens: u64,
    /// Number of internal reasoning/thought tokens reported by the language model.
    pub reasoning_tokens: u64,
    /// Number of output response tokens reported by the language model.
    pub response_tokens: u64,
    /// Formatted dynamic few-shot example strings injected into the prompt; omitted when not applicable or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
}

/// Response returned by `/correct-transcript` containing the corrected plain-text message
/// and optional diagnostic metadata when authenticated as admin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "message": "Sie müssen den Antrag bis Ende nächster Woche abgeben."
}))]
pub struct CorrectTranscriptResponse {
    /// Complete corrected text as plain text, no Markdown.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "Sie müssen den Antrag bis Ende nächster Woche abgeben."
    )]
    pub message: String,
    /// Optional diagnostic information; omitted unless the request carries a valid admin token.
    #[serde(rename = "debugInfo", skip_serializing_if = "Option::is_none")]
    pub debug_info: Option<DebugInfo>,
}

/// Request payload for `/translate` containing the chronological conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "messages": [
        {
            "role": "user",
            "message": "wann muss ich den antrag abgeben"
        },
        {
            "role": "partner",
            "message": "sie müssen den antrag bis ende nächste woche abgeben"
        }
    ]
}))]
pub struct TranslateRequest {
    /// Chronological conversation history; the last role must be "partner".
    #[schema(min_items = 1)]
    pub messages: Vec<ConversationMessage>,
}

/// Type of preparation returned by `/translate`.
///
/// `formatting` structures/formats while largely preserving the wording.
/// `translation` simplifies the language or phrases it more understandably;
/// a target language is not passed in this version.
/// Use %type=formatting% or %type=translation% on staging environment to enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranslationType {
    Formatting,
    Translation,
}

/// Warning severity level for UI display and icon; does not control visibility.
/// Use %warning.type=danger%, %warning.type=warning%, or %warning.type=info% on staging environment to enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WarningType {
    Danger,
    Warning,
    Info,
}

/// Optional content warning attached to a `/translate` response.
///
/// Optional; if there is no warning, the field is omitted, not transmitted as null.
/// Without a warning, content is immediately visible. Content warnings are not
/// technical errors and are returned with HTTP 200.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Warning {
    /// Short headline as plain text.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "Möglicherweise verletzender Inhalt"
    )]
    pub headline: String,
    /// Explanation as Markdown, without embedded HTML.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "Diese Antwort enthält **abwertende Aussagen**."
    )]
    pub body: String,
    /// Warning level for display and icon; does not control visibility.
    /// Use %warning.type=danger%, %warning.type=warning%, or %warning.type=info% on staging environment to enforce.
    #[serde(rename = "type")]
    pub r#type: WarningType,
    /// true hides content initially. The warning with headline and body as well as
    /// the button "Antwort anzeigen" remain visible. The button reveals content
    /// locally without another API call. content is already completely
    /// included in the response. false displays the warning and content immediately.
    /// Use %warning.revealable% on staging environment to enforce.
    pub revealable: bool,
}

/// Response returned by `/translate` with the prepared Markdown content, optional warning,
/// and optional diagnostic metadata when authenticated as admin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranslationResponse {
    /// formatting structures/formats while largely preserving the wording.
    /// translation simplifies the language or phrases it more understandably;
    /// a target language is not passed in this version.
    /// Use %type=formatting% or %type=translation% on staging environment to enforce.
    #[serde(rename = "type")]
    pub r#type: TranslationType,
    /// Complete response text as Markdown, without embedded HTML.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "Geben Sie den Antrag **bis Ende nächster Woche** ab."
    )]
    pub content: String,
    /// Optional content warning; omitted when there is no warning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<Warning>,
    /// Optional diagnostic information; omitted unless the request carries a valid admin token.
    #[serde(rename = "debugInfo", skip_serializing_if = "Option::is_none")]
    pub debug_info: Option<DebugInfo>,
}

/// Inner error object containing a stable error code and human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    /// Stable machine-readable error code; to be evaluated by the client.
    pub code: String,
    /// Human-readable description; the exact wording is not stable.
    pub message: String,
}

/// Standard error envelope returned by all endpoints on 4xx/5xx responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    /// Error details containing code and message.
    pub error: ErrorDetail,
}

impl ErrorResponse {
    /// Constructs a new `ErrorResponse` from a machine-readable code and message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

/// Custom JSON extractor that converts syntax errors into `400 INVALID_JSON`
/// and schema/type errors into `422 VALIDATION_ERROR` using [`ErrorResponse`].
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(JsonRejection::JsonDataError(_)) => Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrorResponse::new(
                    "VALIDATION_ERROR",
                    "A required field is missing or invalid, or the text is empty.",
                )),
            )
                .into_response()),
            Err(_) => Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(
                    "INVALID_JSON",
                    "Request body is not valid JSON.",
                )),
            )
                .into_response()),
        }
    }
}

/// Shared application state for the conversation API endpoints.
#[derive(Clone)]
pub struct ApiState {
    /// Language model prompt runner used by `/correct-transcript` and `/translate`.
    pub runner: Arc<dyn PromptRunner>,
    /// Embedded 6-bucket semantic few-shot index used by `/translate`.
    pub few_shots: Arc<FewShotBucketIndex>,
    /// Configured admin password used to authorize optional `debugInfo` responses.
    pub admin_password: Arc<String>,
}

/// Creates and returns the router for the conversation API endpoints using the provided [`PromptRunner`]
/// and the process-wide default [`FewShotBucketIndex`].
pub fn router(runner: Arc<dyn PromptRunner>) -> Router {
    let few_shots = FewShotBucketIndex::default_shared().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Failed to load default few-shot bucket index; using empty index");
        Arc::new(FewShotBucketIndex::new(
            Default::default(),
            Arc::new(crate::few_shots::DeterministicHashEmbedder),
        ))
    });
    router_with_few_shots(runner, few_shots)
}

/// Creates and returns the router for the conversation API endpoints using the provided [`PromptRunner`]
/// and [`FewShotBucketIndex`], reading `ADMIN_PASSWORD` from the environment (or `"admin"` in debug builds).
pub fn router_with_few_shots(
    runner: Arc<dyn PromptRunner>,
    few_shots: Arc<FewShotBucketIndex>,
) -> Router {
    let admin_password = match std::env::var("ADMIN_PASSWORD") {
        Ok(pw) if !pw.trim().is_empty() => Arc::new(pw),
        _ => Arc::new("admin".to_string()),
    };
    router_with_state(runner, few_shots, admin_password)
}

/// Creates and returns the router for the conversation API endpoints using the provided [`PromptRunner`],
/// [`FewShotBucketIndex`], and `admin_password`.
pub fn router_with_state(
    runner: Arc<dyn PromptRunner>,
    few_shots: Arc<FewShotBucketIndex>,
    admin_password: Arc<String>,
) -> Router {
    Router::new()
        .route("/correct-transcript", post(correct_transcript))
        .route("/translate", post(translate))
        .with_state(ApiState {
            runner,
            few_shots,
            admin_password,
        })
}

/// Checks whether a string contains at least one non-whitespace character.
pub fn is_non_blank(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Correct spoken contribution for the UI.
///
/// For user and partner. Corrects recognition errors, spelling, and
/// punctuation; preserves meaning, language, and tone and does not add
/// information. If no correction is needed, the text is returned unchanged.
/// Processes only the individual contribution, without conversation history.
/// The corrected text replaces only the UI display, not the original text
/// for /translate. In case of an error, the original remains visible;
/// the correction can be retried independently of /translate.
#[utoipa::path(
    post,
    path = "/correct-transcript",
    operation_id = "correctTranscript",
    request_body(
        content = ConversationMessage,
        example = json!({
            "role": "partner",
            "message": "sie müssen den antrag bis ende nächste woche abgeben"
        })
    ),
    responses(
        (
            status = 200,
            description = "Corrected contribution as plain text, no Markdown.",
            body = CorrectTranscriptResponse,
            example = json!({
                "message": "Sie müssen den Antrag bis Ende nächster Woche abgeben."
            })
        ),
        (
            status = 400,
            description = "INVALID_JSON – Request body is not valid JSON.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "INVALID_JSON",
                    "message": "Request body is not valid JSON."
                }
            })
        ),
        (
            status = 401,
            description = "UNAUTHORIZED – Required authentication is missing or invalid.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "UNAUTHORIZED",
                    "message": "Required authentication is missing or invalid."
                }
            })
        ),
        (
            status = 403,
            description = "FORBIDDEN – Access is not permitted.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "FORBIDDEN",
                    "message": "Access is not permitted."
                }
            })
        ),
        (
            status = 422,
            description = "VALIDATION_ERROR – Required field is missing, type or role is invalid, or text is empty.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "VALIDATION_ERROR",
                    "message": "A required field is missing or invalid, or the text is empty."
                }
            })
        ),
        (
            status = 429,
            description = "RATE_LIMITED – Too many requests.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "RATE_LIMITED",
                    "message": "Too many requests."
                }
            })
        ),
        (
            status = 500,
            description = "INTERNAL_ERROR – Processing failed.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "INTERNAL_ERROR",
                    "message": "Processing failed."
                }
            })
        ),
        (
            status = 503,
            description = "SERVICE_UNAVAILABLE – Service is temporarily unavailable.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "SERVICE_UNAVAILABLE",
                    "message": "Service is temporarily unavailable."
                }
            })
        )
    )
)]
pub async fn correct_transcript(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AppJson(request): AppJson<ConversationMessage>,
) -> Result<Json<CorrectTranscriptResponse>, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!(
        endpoint = "/correct-transcript",
        role = ?request.role,
        "Handling correct-transcript request"
    );

    if !is_non_blank(&request.message) {
        tracing::warn!(
            endpoint = "/correct-transcript",
            error = "VALIDATION_ERROR",
            "Validation rejected request: text is empty"
        );
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse::new(
                "VALIDATION_ERROR",
                "A required field is missing or invalid, or the text is empty.",
            )),
        ));
    }

    let prompt = build_correct_transcript_prompt(&request.message);
    let prompt_output = state
        .runner
        .run_prompt(CORRECT_TRANSCRIPT_SYSTEM_PROMPT, &prompt, "minimal")
        .await
        .map_err(|e| {
            tracing::error!(
                endpoint = "/correct-transcript",
                error = %e,
                "LLM API request failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "SERVICE_UNAVAILABLE",
                    "Service is temporarily unavailable.",
                )),
            )
        })?;

    crate::metrics::record_token_usage(
        "/correct-transcript",
        prompt_output.usage.input_tokens,
        prompt_output.usage.reasoning_tokens,
        prompt_output.usage.output_tokens,
    );

    tracing::info!(
        endpoint = "/correct-transcript",
        status = 200,
        input_tokens = prompt_output.usage.input_tokens,
        reasoning_tokens = prompt_output.usage.reasoning_tokens,
        output_tokens = prompt_output.usage.output_tokens,
        "LLM request completed successfully"
    );

    let trimmed = prompt_output.text.trim();
    let message = if trimmed.is_empty() {
        request.message
    } else {
        trimmed.to_string()
    };

    let debug_info =
        crate::web::is_admin_authenticated(&headers, &state.admin_password).then(|| DebugInfo {
            system_prompt: CORRECT_TRANSCRIPT_SYSTEM_PROMPT.to_string(),
            prompt,
            input_tokens: prompt_output.usage.input_tokens,
            reasoning_tokens: prompt_output.usage.reasoning_tokens,
            response_tokens: prompt_output.usage.output_tokens,
            examples: None,
        });

    Ok(Json(CorrectTranscriptResponse {
        message,
        debug_info,
    }))
}

/// Prepare the last statement from "partner" in German simple language.
///
/// Receives the conversation history chronologically with the originally recognized
/// spoken texts, not the UI corrections. Generated preparations are not sent along
/// as conversation contributions. Earlier contributions provide context; the response
/// refers to the last statement from "partner".
/// The last message MUST have the role "partner" (server-side validation;
/// otherwise 422 LAST_MESSAGE_MUST_BE_PARTNER).
/// Consecutive contributions with the same role are allowed.
/// Starts in parallel with /correct-transcript and does not wait for its result.
/// The order of responses is arbitrary; errors do not block the respective other call.
/// Retries continue to use the original texts.
#[utoipa::path(
    post,
    path = "/translate",
    operation_id = "translate",
    request_body(
        content = TranslateRequest,
        example = json!({
            "messages": [
                {
                    "role": "user",
                    "message": "wann muss ich den antrag abgeben"
                },
                {
                    "role": "partner",
                    "message": "sie müssen den antrag bis ende nächste woche abgeben"
                }
            ]
        })
    ),
    responses(
        (
            status = 200,
            description = "Prepared response with optional content warning.",
            body = TranslationResponse,
            examples(
                ("withoutWarning" = (
                    summary = "Response without warning",
                    value = json!({
                        "type": "translation",
                        "content": "Geben Sie den Antrag **bis Ende nächster Woche** ab."
                    })
                )),
                ("withWarning" = (
                    summary = "Example with initially hidden response text",
                    value = json!({
                        "type": "translation",
                        "content": "Die aufbereitete Antwort steht hier.",
                        "warning": {
                            "headline": "Möglicherweise verletzender Inhalt",
                            "body": "Diese Antwort enthält **abwertende Aussagen**.",
                            "type": "warning",
                            "revealable": true
                        }
                    })
                ))
            )
        ),
        (
            status = 400,
            description = "INVALID_JSON – Request body is not valid JSON.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "INVALID_JSON",
                    "message": "Request body is not valid JSON."
                }
            })
        ),
        (
            status = 401,
            description = "UNAUTHORIZED – Required authentication is missing or invalid.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "UNAUTHORIZED",
                    "message": "Required authentication is missing or invalid."
                }
            })
        ),
        (
            status = 403,
            description = "FORBIDDEN – Access is not permitted.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "FORBIDDEN",
                    "message": "Access is not permitted."
                }
            })
        ),
        (
            status = 422,
            description = "VALIDATION_ERROR – Invalid fields or empty message list; LAST_MESSAGE_MUST_BE_PARTNER – Last statement does not come from partner.",
            body = ErrorResponse,
            examples(
                ("invalidFields" = (
                    value = json!({
                        "error": {
                            "code": "VALIDATION_ERROR",
                            "message": "The message list must not be empty."
                        }
                    })
                )),
                ("lastRole" = (
                    value = json!({
                        "error": {
                            "code": "LAST_MESSAGE_MUST_BE_PARTNER",
                            "message": "The last message must come from the conversation partner."
                        }
                    })
                ))
            )
        ),
        (
            status = 429,
            description = "RATE_LIMITED – Too many requests.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "RATE_LIMITED",
                    "message": "Too many requests."
                }
            })
        ),
        (
            status = 500,
            description = "INTERNAL_ERROR – Processing failed.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "INTERNAL_ERROR",
                    "message": "Processing failed."
                }
            })
        ),
        (
            status = 503,
            description = "SERVICE_UNAVAILABLE – Service is temporarily unavailable.",
            body = ErrorResponse,
            example = json!({
                "error": {
                    "code": "SERVICE_UNAVAILABLE",
                    "message": "Service is temporarily unavailable."
                }
            })
        )
    )
)]
pub async fn translate(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AppJson(request): AppJson<TranslateRequest>,
) -> Result<Json<TranslationResponse>, (StatusCode, Json<ErrorResponse>)> {
    let Some((last_message, history)) = request.messages.split_last() else {
        tracing::warn!(
            endpoint = "/translate",
            error = "VALIDATION_ERROR",
            "Validation rejected request: message list is empty"
        );
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse::new(
                "VALIDATION_ERROR",
                "The message list must not be empty.",
            )),
        ));
    };

    if request.messages.iter().any(|m| !is_non_blank(&m.message)) {
        tracing::warn!(
            endpoint = "/translate",
            error = "VALIDATION_ERROR",
            "Validation rejected request: contains blank message"
        );
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse::new(
                "VALIDATION_ERROR",
                "A required field is missing or invalid, or the text is empty.",
            )),
        ));
    }

    if last_message.role != Role::Partner {
        tracing::warn!(
            endpoint = "/translate",
            error = "LAST_MESSAGE_MUST_BE_PARTNER",
            "Validation rejected request: last message role is not partner"
        );
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse::new(
                "LAST_MESSAGE_MUST_BE_PARTNER",
                "The last message must come from the conversation partner.",
            )),
        ));
    }

    let history_len = count_history_turns(history);
    let bucket = determine_history_bucket(history);
    crate::metrics::record_translate_history_length(history_len);

    tracing::debug!(
        endpoint = "/translate",
        history_length = history_len,
        bucket = bucket,
        "Handling translate request"
    );

    let is_staging = std::env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "production".to_string())
        .eq_ignore_ascii_case("staging");

    let directives = extract_staging_directives(&last_message.message, is_staging);
    let examples = state
        .few_shots
        .get_closest_examples(&directives.cleaned_message, bucket, DEFAULT_FEW_SHOT_K)
        .unwrap_or_else(|e| {
            tracing::warn!(
                endpoint = "/translate",
                error = %e,
                "Failed to retrieve dynamic few-shot examples; falling back to zero-shot prompt"
            );
            Vec::new()
        });
    let prompt =
        build_translate_prompt_with_few_shots(history, &directives.cleaned_message, &examples);

    let prompt_output = state
        .runner
        .run_prompt(TRANSLATE_SYSTEM_PROMPT, &prompt, "low")
        .await
        .map_err(|e| {
            tracing::error!(
                endpoint = "/translate",
                error = %e,
                "LLM API request failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "SERVICE_UNAVAILABLE",
                    "Service is temporarily unavailable.",
                )),
            )
        })?;

    crate::metrics::record_token_usage(
        "/translate",
        prompt_output.usage.input_tokens,
        prompt_output.usage.reasoning_tokens,
        prompt_output.usage.output_tokens,
    );

    tracing::info!(
        endpoint = "/translate",
        status = 200,
        history_length = history_len,
        input_tokens = prompt_output.usage.input_tokens,
        reasoning_tokens = prompt_output.usage.reasoning_tokens,
        output_tokens = prompt_output.usage.output_tokens,
        "LLM request completed successfully"
    );

    let (model_type, model_content) = parse_translate_output(&prompt_output.text);
    let content = if model_content.is_empty() {
        directives.cleaned_message
    } else {
        model_content
    };

    let debug_info =
        crate::web::is_admin_authenticated(&headers, &state.admin_password).then(|| {
            let example_strings: Vec<String> = examples.iter().map(|e| e.sample.clone()).collect();
            DebugInfo {
                system_prompt: TRANSLATE_SYSTEM_PROMPT.to_string(),
                prompt,
                input_tokens: prompt_output.usage.input_tokens,
                reasoning_tokens: prompt_output.usage.reasoning_tokens,
                response_tokens: prompt_output.usage.output_tokens,
                examples: (!example_strings.is_empty()).then_some(example_strings),
            }
        });

    Ok(Json(TranslationResponse {
        r#type: directives.r#type.unwrap_or(model_type),
        content,
        warning: directives.warning,
        debug_info,
    }))
}

/// Builds a [`TranslationResponse`] from the partner's message text and a model output.
///
/// When `is_staging` is `true` (`ENVIRONMENT=staging`), honors inline test directives
/// (`%type=formatting%`, `%type=translation%`, `%warning.type=...%`, and `%warning.revealable%`)
/// to override `type` and attach `warning`, while stripping the directive markers from `content`.
/// In `production` (default, `is_staging == false`), directives are ignored.
pub fn build_translation_response(message: &str, is_staging: bool) -> TranslationResponse {
    let directives = extract_staging_directives(message, is_staging);
    TranslationResponse {
        r#type: directives.r#type.unwrap_or(TranslationType::Formatting),
        content: directives.cleaned_message,
        warning: directives.warning,
        debug_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `is_non_blank` rejects empty and whitespace-only strings and accepts non-blank strings.
    #[test]
    fn test_is_non_blank() {
        assert!(!is_non_blank(""));
        assert!(!is_non_blank("   \t\n "));
        assert!(is_non_blank("hello"));
        assert!(is_non_blank("  a  "));
    }

    /// Verifies that `build_translation_response` enforces `%type=...%` and `%warning...%` directives only in staging.
    #[test]
    fn test_build_translation_response_directives() {
        let default_res = build_translation_response("Normaler Text", true);
        assert_eq!(default_res.r#type, TranslationType::Formatting);
        assert!(default_res.warning.is_none());

        let trans_res = build_translation_response("Text %type=translation%", true);
        assert_eq!(trans_res.r#type, TranslationType::Translation);
        assert!(trans_res.warning.is_none());

        let fmt_res = build_translation_response("Text %type=formatting%", true);
        assert_eq!(fmt_res.r#type, TranslationType::Formatting);

        let revealable_only = build_translation_response("Achtung %warning.revealable%", true);
        assert_eq!(
            revealable_only.warning,
            Some(Warning {
                headline: "Zu Ihrer Information".to_string(),
                body: "Passen Sie auf sich auf!".to_string(),
                r#type: WarningType::Info,
                revealable: true,
            })
        );

        let warning_type_only = build_translation_response("Gefahr %warning.type=danger%", true);
        assert_eq!(
            warning_type_only.warning,
            Some(Warning {
                headline: "Zu Ihrer Information".to_string(),
                body: "Passen Sie auf sich auf!".to_string(),
                r#type: WarningType::Danger,
                revealable: false,
            })
        );

        let combined = build_translation_response(
            "Kombi %type=translation% %warning.type=warning% %warning.revealable%",
            true,
        );
        assert_eq!(combined.r#type, TranslationType::Translation);
        assert_eq!(
            combined.warning,
            Some(Warning {
                headline: "Zu Ihrer Information".to_string(),
                body: "Passen Sie auf sich auf!".to_string(),
                r#type: WarningType::Warning,
                revealable: true,
            })
        );

        // In production (is_staging = false), directives are ignored
        let prod_res = build_translation_response(
            "Kombi %type=translation% %warning.type=warning% %warning.revealable%",
            false,
        );
        assert_eq!(prod_res.r#type, TranslationType::Formatting);
        assert!(prod_res.warning.is_none());
    }
}
