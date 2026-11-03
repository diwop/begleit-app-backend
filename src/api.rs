use crate::few_shots::{FewShotBucketIndex, DEFAULT_FEW_SHOT_K};
use crate::prompts::{
    apply_classification, build_classify_dialog_prompt_for_partner,
    build_correct_transcript_contextual_prompt, build_correct_transcript_simple_prompt,
    build_translate_prompt_with_few_shots_for_partner, count_history_turns,
    determine_history_bucket, enforce_correct_transcript_guardrails, extract_staging_directives,
    parse_classify_dialog_output, parse_correct_transcript_output,
    parse_correct_transcript_simple_output, parse_translate_output, ClassificationCategory,
    ClassificationLevel, DialogTopic, PromptRunner, CLASSIFY_DIALOG_RESPONSE_SCHEMA,
    CLASSIFY_DIALOG_SYSTEM_PROMPT, CORRECT_TRANSCRIPT_RESPONSE_SCHEMA,
    CORRECT_TRANSCRIPT_SIMPLE_RESPONSE_SCHEMA, CORRECT_TRANSCRIPT_SIMPLE_SYSTEM_PROMPT,
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

/// Spoken contribution in a conversation with speaker role, plain-text message, and optional 1-based partner number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "role": "partner",
    "message": "sie müssen den antrag bis ende nächste woche abgeben"
}))]
pub struct ConversationMessage {
    /// `user` is the app user, `partner` is the conversation partner, and `partner_simple` is a previously translated simple-language partner contribution.
    pub role: Role,
    /// Corrected transcript if available (otherwise plain recognized text) for `user` and `partner`, or prepared Markdown content for `partner_simple`.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "sie müssen den antrag bis ende nächste woche abgeben"
    )]
    pub message: String,
    /// Optional 1-based partner identifier when `role` is `"partner"` (or `"partner_simple"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 1)]
    pub partner_number: Option<u32>,
}

/// Inferred real-world role of a conversation partner with model confidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "roleType": "Arzt",
    "confidence": 0.95
}))]
pub struct PartnerRole {
    /// Concise German role title suitable as a UI card header (e.g. `"Arzt"`, `"Ärztin"`, `"ÄrztIn"`, `"SachbearbeiterIn"`).
    #[schema(min_length = 1, pattern = r"\S", example = "Arzt")]
    pub role_type: String,
    /// Model confidence in `[0.0, 1.0]`.
    #[schema(minimum = 0.0, maximum = 1.0, example = 0.95)]
    pub confidence: f64,
}

/// Diagnostic details for the post-translation dialog classification step (`/translate` only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationDebugInfo {
    /// Extracted high-level dialog topic.
    pub topic: DialogTopic,
    /// Classification severity level (`pass`, `info`, `warning`, or `danger`).
    pub classification_type: ClassificationLevel,
    /// Specific classification category paired with `classification_type`.
    pub category: ClassificationCategory,
    /// Optional inferred partner role and confidence from the classification step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partner_role: Option<PartnerRole>,
    /// Expanded system prompt preamble sent to the classification model.
    pub system_prompt: String,
    /// Rendered user prompt sent to the classification model.
    pub prompt: String,
    /// Number of tokens in the classification system prompt preamble.
    #[serde(default)]
    pub system_prompt_tokens: u64,
    /// Number of tokens in the classification user prompt.
    #[serde(default)]
    pub prompt_tokens: u64,
    /// Total number of input prompt tokens reported by the classification model.
    pub input_tokens: u64,
    /// Number of internal reasoning/thought tokens reported by the classification model.
    pub reasoning_tokens: u64,
    /// Number of output response tokens reported by the classification model.
    pub response_tokens: u64,
    /// Optional internal reasoning/thought text reported by the classification model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Raw JSON output returned by the classification model.
    pub raw_response: String,
}

/// Optional diagnostic information returned when a request carries a valid admin password or session token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebugInfo {
    /// System prompt preamble sent to the primary language model step.
    pub system_prompt: String,
    /// Rendered user prompt sent to the primary language model step.
    pub prompt: String,
    /// Number of tokens in the system prompt preamble.
    #[serde(default)]
    pub system_prompt_tokens: u64,
    /// Number of tokens in the user prompt.
    #[serde(default)]
    pub prompt_tokens: u64,
    /// Total number of input prompt tokens reported by the primary language model step.
    pub input_tokens: u64,
    /// Number of internal reasoning/thought tokens reported by the primary language model step.
    pub reasoning_tokens: u64,
    /// Number of output response tokens reported by the primary language model step.
    pub response_tokens: u64,
    /// Optional internal reasoning/thought text reported by the primary language model step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Formatted dynamic few-shot example strings injected into the prompt; omitted when not applicable or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    /// Diagnostic details for the post-translation dialog classification step; omitted on `/correct-transcript`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<ClassificationDebugInfo>,
}

/// Single chronological entry in [`CorrectTranscriptRequest::history`].
///
/// The final entry in `history` is the spoken turn to correct (`user` or `partner`);
/// preceding entries (`user`, `partner`, or `partner_simple`) provide read-only conversation context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CorrectTranscriptHistoryItem {
    /// Provisional or previously resolved speaker role (`user`, `partner`, or `partner_simple` in prior context; final entry must be `user` or `partner`).
    pub role: Role,
    /// Raw spoken transcript for the final turn to correct, or corrected/prepared text for earlier turns.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "sie müssen den antrag bis ende nächste woche abgeben"
    )]
    pub message: String,
    /// Optional 1-based partner identifier when `role` is `"partner"` (or `"partner_simple"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 1)]
    pub partner_number: Option<u32>,
    /// Optional acoustic speaker cluster identifier from WASM diarization (e.g. `"cluster_0"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    /// Optional acoustic probability in `[0.0, 1.0]` that the speaker is `user`.
    /// When omitted, defaults deterministically from `role` (`1.0` for `user`, `0.0` for `partner`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_likelihood: Option<f32>,
    /// Optional silence gap in milliseconds between the end of the previous turn and the start of this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_ms: Option<u64>,
}

/// Backwards-compatible request payload for `POST /correct-transcript`.
///
/// If `history` is set (`Some`), `history` is used and its final element is the turn to correct
/// while preceding elements provide read-only conversation context.
/// If `history` is not set (`None`), the legacy `role` and `message` fields are used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "role": "partner",
    "message": "sie müssen den antrag bis ende nächste woche abgeben"
}))]
pub struct CorrectTranscriptRequest {
    /// Legacy speaker role (`user` or `partner`); used when `history` is not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Optional 1-based partner identifier when `role` is `"partner"` in legacy mode; ignored when `history` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 1)]
    pub partner_number: Option<u32>,
    /// Legacy raw spoken transcript to correct; used when `history` is not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional chronological conversation turns ending with the turn to correct.
    /// Takes precedence over `role` and `message` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<CorrectTranscriptHistoryItem>>,
}

impl CorrectTranscriptRequest {
    /// Validates and normalizes the request into `(prior_history, target_turn, has_contextual_history)`.
    ///
    /// - When `history` is `Some(items)`, validates that `items` is non-empty, every item's `message`
    ///   is non-blank, the final item's `role` is `Role::User` or `Role::Partner`, any present
    ///   `partner_number` is `>= 1` and not paired with `Role::User`, and any present
    ///   `user_likelihood` is finite in `[0.0, 1.0]`.
    /// - When `history` is `None`, validates that `role` (`Role::User` or `Role::Partner`), a
    ///   non-blank `message`, and a valid optional `partner_number` are provided.
    pub fn resolve_turns(
        &self,
    ) -> Result<
        (
            Vec<CorrectTranscriptHistoryItem>,
            CorrectTranscriptHistoryItem,
            bool,
        ),
        &'static str,
    > {
        if let Some(ref items) = self.history {
            let Some((last, prior)) = items.split_last() else {
                return Err("The history list must not be empty.");
            };
            if items.iter().any(|item| !is_non_blank(&item.message)) {
                return Err("A required field is missing or invalid, or the text is empty.");
            }
            if !matches!(last.role, Role::User | Role::Partner) {
                return Err("The target turn to correct must have role 'user' or 'partner'.");
            }
            for item in items {
                if let Some(pn) = item.partner_number {
                    if pn == 0 {
                        return Err("partnerNumber must be at least 1.");
                    }
                    if item.role == Role::User {
                        return Err("partnerNumber is only allowed when role is 'partner'.");
                    }
                }
                if let Some(p) = item.user_likelihood {
                    if !p.is_finite() || !(0.0..=1.0).contains(&p) {
                        return Err("userLikelihood must be a finite number between 0.0 and 1.0.");
                    }
                }
            }
            return Ok((prior.to_vec(), last.clone(), true));
        }

        let Some(role) = self.role else {
            return Err("A required field is missing or invalid, or the text is empty.");
        };
        if !matches!(role, Role::User | Role::Partner) {
            return Err("The target turn to correct must have role 'user' or 'partner'.");
        }
        if let Some(pn) = self.partner_number {
            if pn == 0 {
                return Err("partnerNumber must be at least 1.");
            }
            if role == Role::User {
                return Err("partnerNumber is only allowed when role is 'partner'.");
            }
        }
        let Some(ref message) = self.message else {
            return Err("A required field is missing or invalid, or the text is empty.");
        };
        if !is_non_blank(message) {
            return Err("A required field is missing or invalid, or the text is empty.");
        }

        Ok((
            Vec::new(),
            CorrectTranscriptHistoryItem {
                role,
                message: message.clone(),
                partner_number: self.partner_number,
                cluster_id: None,
                user_likelihood: None,
                pause_ms: None,
            },
            false,
        ))
    }
}

/// Response returned by `/correct-transcript` containing the corrected plain-text message,
/// optional speaker/turn-merge resolution metadata when `history` was provided,
/// and optional diagnostic metadata when authenticated as admin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "message": "Sie müssen den Antrag bis Ende nächster Woche abgeben."
}))]
pub struct CorrectTranscriptResponse {
    /// Complete corrected text as plain text, no Markdown (or combined text of the preceding and current turn when `mergeWithPrevious` is `true`).
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "Sie müssen den Antrag bis Ende nächster Woche abgeben."
    )]
    pub message: String,
    /// Verified speaker role (`user` or `partner`); populated when `history` is provided, omitted in legacy mode.
    #[serde(
        default,
        rename = "resolvedRole",
        skip_serializing_if = "Option::is_none"
    )]
    pub resolved_role: Option<Role>,
    /// Whether this turn is a direct continuation that should be merged into the immediately preceding spoken turn; populated when `history` is provided, omitted in legacy mode.
    #[serde(
        default,
        rename = "mergeWithPrevious",
        skip_serializing_if = "Option::is_none"
    )]
    pub merge_with_previous: Option<bool>,
    /// Optional diagnostic information; omitted unless the request carries a valid admin token.
    #[serde(default, rename = "debugInfo", skip_serializing_if = "Option::is_none")]
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

impl TranslationType {
    /// Returns the canonical lowercase string label (`"formatting"` or `"translation"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Formatting => "formatting",
            Self::Translation => "translation",
        }
    }
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
    #[schema(min_length = 1, pattern = r"\S", example = "Hinweis zu Rechts-Themen")]
    pub headline: String,
    /// Explanation as Markdown, without embedded HTML.
    #[schema(
        min_length = 1,
        pattern = r"\S",
        example = "Diese Übersetzung erklärt rechtliche Begriffe.  \nDie Erklärung kommt von einer Künstlichen Intelligenz.  \nDie Übersetzung ist **keine** Rechts-Beratung.  \nBitte wenden Sie sich bei rechtlichen Fragen an eine Rechts-Beratung."
    )]
    pub body: String,
    /// Warning level for display and icon; does not control visibility.
    /// Use %warning.type=danger%, %warning.type=warning%, or %warning.type=info% on staging environment to enforce.
    #[serde(rename = "type")]
    pub r#type: WarningType,
    /// true hides content initially. The warning with headline and body as well as
    /// the button "Antwort anzeigen" remain visible. The button reveals content
    /// locally without another API call. content is already completely
    /// included in the response. false displays the warning and content immediately
    /// (or only the warning when content is empty for blocked `safety` / `youth-protection` statements).
    /// Use %warning.revealable% on staging environment to enforce.
    pub revealable: bool,
}

/// Response returned by `/translate` with the prepared Markdown content, optional warning,
/// optional inferred partner role, and optional diagnostic metadata when authenticated as admin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TranslationResponse {
    /// formatting structures/formats while largely preserving the wording.
    /// translation simplifies the language or phrases it more understandably;
    /// a target language is not passed in this version.
    /// Use %type=formatting% or %type=translation% on staging environment to enforce.
    #[serde(rename = "type")]
    pub r#type: TranslationType,
    /// Complete response text as Markdown, without embedded HTML.
    /// Empty (`""`) when the statement is blocked due to `danger` (`safety` or `youth-protection`).
    #[schema(example = "Geben Sie den Antrag **bis Ende nächster Woche** ab.")]
    pub content: String,
    /// Optional content warning; omitted when there is no warning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<Warning>,
    /// Optional inferred real-world role of the speaking conversation partner; omitted when unknown.
    #[serde(
        default,
        rename = "partnerRole",
        skip_serializing_if = "Option::is_none"
    )]
    pub partner_role: Option<PartnerRole>,
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

/// Correct spoken contribution for the UI and subsequent conversation history.
///
/// Supports both the legacy single-message payload (`{ role, message }`) and the contextual
/// `history` payload (`{ history: [...] }` where the last item is the turn to correct and
/// preceding items provide read-only conversation context alongside optional acoustic diarization
/// metadata `clusterId`, `userLikelihood`, and `pauseMs`).
/// Uses `"minimal"` reasoning effort and structured JSON output for low-latency ASR disambiguation,
/// borderline speaker verification (`resolvedRole`), and split-turn unification (`mergeWithPrevious`).
#[utoipa::path(
    post,
    path = "/correct-transcript",
    operation_id = "correctTranscript",
    request_body(
        content = CorrectTranscriptRequest,
        example = json!({
            "role": "partner",
            "message": "sie müssen den antrag bis ende nächste woche abgeben"
        })
    ),
    responses(
        (
            status = 200,
            description = "Corrected contribution as plain text, with optional speaker and turn-merge resolution when history is provided.",
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
    AppJson(request): AppJson<CorrectTranscriptRequest>,
) -> Result<Json<CorrectTranscriptResponse>, (StatusCode, Json<ErrorResponse>)> {
    let (prior_history, target_turn, is_contextual_history) =
        request.resolve_turns().map_err(|reason| {
            tracing::warn!(
                endpoint = "/correct-transcript",
                error = "VALIDATION_ERROR",
                reason = reason,
                "Validation rejected correct-transcript request"
            );
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrorResponse::new("VALIDATION_ERROR", reason)),
            )
        })?;

    tracing::debug!(
        endpoint = "/correct-transcript",
        role = ?target_turn.role,
        history_len = prior_history.len(),
        is_contextual_history,
        "Handling correct-transcript request"
    );

    let (message, resolved_role, merge_with_previous, prompt, system_prompt_str, prompt_output) =
        if is_contextual_history {
            let prompt = build_correct_transcript_contextual_prompt(&prior_history, &target_turn);
            let prompt_output = state
                .runner
                .run_prompt_with_schema(
                    CORRECT_TRANSCRIPT_SYSTEM_PROMPT,
                    &prompt,
                    "minimal",
                    &CORRECT_TRANSCRIPT_RESPONSE_SCHEMA,
                )
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
                "correct-transcript",
                prompt_output.usage.input_tokens,
                prompt_output.usage.reasoning_tokens,
                prompt_output.usage.output_tokens,
            );

            let parsed_output =
                parse_correct_transcript_output(&prompt_output.text).map_err(|e| {
                    tracing::error!(
                        endpoint = "/correct-transcript",
                        error = %e,
                        raw_output = %prompt_output.text,
                        "Failed to parse structured correct-transcript output"
                    );
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(ErrorResponse::new(
                            "SERVICE_UNAVAILABLE",
                            "Service is temporarily unavailable.",
                        )),
                    )
                })?;

            let (msg, r_role, m_prev) =
                enforce_correct_transcript_guardrails(parsed_output, &prior_history, &target_turn);

            (
                msg,
                Some(r_role),
                Some(m_prev),
                prompt,
                CORRECT_TRANSCRIPT_SYSTEM_PROMPT,
                prompt_output,
            )
        } else {
            let prompt =
                build_correct_transcript_simple_prompt(target_turn.role, &target_turn.message);
            let prompt_output = state
                .runner
                .run_prompt_with_schema(
                    CORRECT_TRANSCRIPT_SIMPLE_SYSTEM_PROMPT,
                    &prompt,
                    "minimal",
                    &CORRECT_TRANSCRIPT_SIMPLE_RESPONSE_SCHEMA,
                )
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
                "correct-transcript",
                prompt_output.usage.input_tokens,
                prompt_output.usage.reasoning_tokens,
                prompt_output.usage.output_tokens,
            );

            let parsed_msg =
                parse_correct_transcript_simple_output(&prompt_output.text).map_err(|e| {
                    tracing::error!(
                        endpoint = "/correct-transcript",
                        error = %e,
                        raw_output = %prompt_output.text,
                        "Failed to parse simple correct-transcript output"
                    );
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(ErrorResponse::new(
                            "SERVICE_UNAVAILABLE",
                            "Service is temporarily unavailable.",
                        )),
                    )
                })?;

            (
                parsed_msg,
                None,
                None,
                prompt,
                CORRECT_TRANSCRIPT_SIMPLE_SYSTEM_PROMPT,
                prompt_output,
            )
        };

    tracing::info!(
        endpoint = "/correct-transcript",
        status = 200,
        resolved_role = ?resolved_role,
        merge_with_previous = ?merge_with_previous,
        input_tokens = prompt_output.usage.input_tokens,
        reasoning_tokens = prompt_output.usage.reasoning_tokens,
        output_tokens = prompt_output.usage.output_tokens,
        "LLM request completed successfully"
    );

    let debug_info =
        crate::web::is_admin_authenticated(&headers, &state.admin_password).then(|| {
            let system_prompt_tokens = crate::prompts::count_gemma_tokens(system_prompt_str);
            let prompt_tokens = if prompt_output.usage.input_tokens > system_prompt_tokens {
                prompt_output.usage.input_tokens - system_prompt_tokens
            } else {
                crate::prompts::count_gemma_tokens(&prompt)
            };
            DebugInfo {
                system_prompt: system_prompt_str.to_string(),
                prompt,
                system_prompt_tokens,
                prompt_tokens,
                input_tokens: prompt_output.usage.input_tokens,
                reasoning_tokens: prompt_output.usage.reasoning_tokens,
                response_tokens: prompt_output.usage.output_tokens,
                reasoning: prompt_output.reasoning,
                examples: None,
                classification: None,
            }
        });

    Ok(Json(CorrectTranscriptResponse {
        message,
        resolved_role,
        merge_with_previous,
        debug_info,
    }))
}

/// Prepare and classify the last statement from "partner" in German simple language.
///
/// Receives the conversation history chronologically, using corrected transcripts
/// for earlier "user" and "partner" contributions whenever already available
/// (falling back to plain recognized texts when still pending or failed, such as
/// for the latest "partner" contribution) and including completed preparations as
/// "partner_simple" contributions. Earlier contributions provide context; the
/// response refers to the last statement from "partner".
/// After preparing the translation (`"medium"` reasoning), runs the dialog
/// classification prompt (`"low"` reasoning + JSON Schema) to extract the dialog
/// topic for monitoring, attach EU AI Act / safety disclaimers, and block unsafe
/// statements (`safety`, `youth-protection`) with an empty `content` (`""`).
/// Fails closed (`503 SERVICE_UNAVAILABLE`) if either LLM step fails.
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
                    summary = "Example with EU AI Act disclaimer and initially hidden response text",
                    value = json!({
                        "type": "translation",
                        "content": "Die aufbereitete Antwort steht hier.",
                        "warning": {
                            "headline": "Hinweis zu Rechts-Themen",
                            "body": "Diese Übersetzung erklärt rechtliche Begriffe.  \nDie Erklärung kommt von einer Künstlichen Intelligenz.  \nDie Übersetzung ist **keine** Rechts-Beratung.  \nBitte wenden Sie sich bei rechtlichen Fragen an eine Rechts-Beratung.",
                            "type": "warning",
                            "revealable": true
                        }
                    })
                )),
                ("blockedDanger" = (
                    summary = "Example blocked for safety or youth-protection with empty content",
                    value = json!({
                        "type": "translation",
                        "content": "",
                        "warning": {
                            "headline": "Aussage aus Sicherheits-Gründen blockiert",
                            "body": "Diese Aussage beschreibt gefährliche oder verbotene Handlungen.  \nDeshalb wird diese Aussage **nicht** übersetzt.",
                            "type": "danger",
                            "revealable": false
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

    for msg in &request.messages {
        if let Some(pn) = msg.partner_number {
            if pn == 0 {
                tracing::warn!(
                    endpoint = "/translate",
                    error = "VALIDATION_ERROR",
                    "Validation rejected request: partnerNumber is 0"
                );
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorResponse::new(
                        "VALIDATION_ERROR",
                        "partnerNumber must be at least 1.",
                    )),
                ));
            }
            if msg.role == Role::User {
                tracing::warn!(
                    endpoint = "/translate",
                    error = "VALIDATION_ERROR",
                    "Validation rejected request: partnerNumber provided on user role"
                );
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorResponse::new(
                        "VALIDATION_ERROR",
                        "partnerNumber is only allowed when role is 'partner'.",
                    )),
                ));
            }
        }
    }

    let history_len = count_history_turns(history);
    let bucket = determine_history_bucket(history);
    crate::metrics::record_translate_history_length(history_len);

    tracing::debug!(
        endpoint = "/translate",
        history_length = history_len,
        bucket = bucket,
        partner_number = ?last_message.partner_number,
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
    let prompt = build_translate_prompt_with_few_shots_for_partner(
        history,
        &directives.cleaned_message,
        last_message.partner_number,
        &examples,
    );

    let prompt_output = state
        .runner
        .run_prompt(TRANSLATE_SYSTEM_PROMPT, &prompt, "medium")
        .await
        .map_err(|e| {
            tracing::error!(
                endpoint = "/translate",
                stage = "translate",
                error = %e,
                "LLM API translation request failed"
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
        "translate",
        prompt_output.usage.input_tokens,
        prompt_output.usage.reasoning_tokens,
        prompt_output.usage.output_tokens,
    );

    let (model_type, model_content) = parse_translate_output(&prompt_output.text);
    let translated_content = if model_content.is_empty() {
        crate::prompts::ensure_markdown_line_breaks(&directives.cleaned_message)
    } else {
        model_content
    };

    let classify_prompt = build_classify_dialog_prompt_for_partner(
        history,
        &directives.cleaned_message,
        &translated_content,
        last_message.partner_number,
    );
    let classify_output = state
        .runner
        .run_prompt_with_schema(
            &CLASSIFY_DIALOG_SYSTEM_PROMPT,
            &classify_prompt,
            "low",
            &CLASSIFY_DIALOG_RESPONSE_SCHEMA,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                endpoint = "/translate",
                stage = "classify",
                error = %e,
                "LLM API dialog classification request failed"
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
        "classify",
        classify_output.usage.input_tokens,
        classify_output.usage.reasoning_tokens,
        classify_output.usage.output_tokens,
    );

    let classification = parse_classify_dialog_output(&classify_output.text).map_err(|e| {
        tracing::error!(
            endpoint = "/translate",
            stage = "classify",
            error = %e,
            raw_output = %classify_output.text,
            "Failed to parse or validate dialog classification output"
        );
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse::new(
                "SERVICE_UNAVAILABLE",
                "Service is temporarily unavailable.",
            )),
        )
    })?;

    let (classified_type, final_content, classified_warning) =
        apply_classification(&classification, model_type, translated_content);
    let final_type = directives.r#type.unwrap_or(classified_type);

    crate::metrics::record_translate_classification(
        classification.topic.as_str(),
        final_type.as_str(),
        classification.category.as_str(),
    );

    let partner_role = classification.normalized_partner_role();

    tracing::info!(
        endpoint = "/translate",
        status = 200,
        history_length = history_len,
        topic = classification.topic.as_str(),
        translation_type = final_type.as_str(),
        classification_type = classification.r#type.as_str(),
        classification_category = classification.category.as_str(),
        partner_role = ?partner_role,
        input_tokens = prompt_output.usage.input_tokens,
        reasoning_tokens = prompt_output.usage.reasoning_tokens,
        output_tokens = prompt_output.usage.output_tokens,
        classify_input_tokens = classify_output.usage.input_tokens,
        classify_reasoning_tokens = classify_output.usage.reasoning_tokens,
        classify_output_tokens = classify_output.usage.output_tokens,
        "Translate and dialog classification completed successfully"
    );

    let debug_info =
        crate::web::is_admin_authenticated(&headers, &state.admin_password).then(|| {
            let example_strings: Vec<String> = examples.iter().map(|e| e.sample.clone()).collect();
            let system_prompt_tokens = crate::prompts::count_gemma_tokens(TRANSLATE_SYSTEM_PROMPT);
            let prompt_tokens = if prompt_output.usage.input_tokens > system_prompt_tokens {
                prompt_output.usage.input_tokens - system_prompt_tokens
            } else {
                crate::prompts::count_gemma_tokens(&prompt)
            };

            let classify_system_prompt = CLASSIFY_DIALOG_SYSTEM_PROMPT.clone();
            let classify_system_prompt_tokens =
                crate::prompts::count_gemma_tokens(&classify_system_prompt);
            let classify_prompt_tokens =
                if classify_output.usage.input_tokens > classify_system_prompt_tokens {
                    classify_output.usage.input_tokens - classify_system_prompt_tokens
                } else {
                    crate::prompts::count_gemma_tokens(&classify_prompt)
                };

            DebugInfo {
                system_prompt: TRANSLATE_SYSTEM_PROMPT.to_string(),
                prompt,
                system_prompt_tokens,
                prompt_tokens,
                input_tokens: prompt_output.usage.input_tokens,
                reasoning_tokens: prompt_output.usage.reasoning_tokens,
                response_tokens: prompt_output.usage.output_tokens,
                reasoning: prompt_output.reasoning,
                examples: (!example_strings.is_empty()).then_some(example_strings),
                classification: Some(ClassificationDebugInfo {
                    topic: classification.topic,
                    classification_type: classification.r#type,
                    category: classification.category,
                    partner_role: partner_role.clone(),
                    system_prompt: classify_system_prompt,
                    prompt: classify_prompt,
                    system_prompt_tokens: classify_system_prompt_tokens,
                    prompt_tokens: classify_prompt_tokens,
                    input_tokens: classify_output.usage.input_tokens,
                    reasoning_tokens: classify_output.usage.reasoning_tokens,
                    response_tokens: classify_output.usage.output_tokens,
                    reasoning: classify_output.reasoning,
                    raw_response: classify_output.text,
                }),
            }
        });

    Ok(Json(TranslationResponse {
        r#type: final_type,
        content: final_content,
        warning: directives.warning.or(classified_warning),
        partner_role,
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
        partner_role: None,
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

    /// Verifies `CorrectTranscriptRequest::resolve_turns` for legacy payloads, contextual `history`
    /// precedence, `partnerNumber` propagation, and validation failures (`history: []`, blank messages,
    /// `partner_simple` target, invalid `partnerNumber`, and out-of-range `userLikelihood`).
    #[test]
    fn test_correct_transcript_request_resolve_turns() {
        // 1. Legacy payload (history = None)
        let legacy = CorrectTranscriptRequest {
            role: Some(Role::Partner),
            partner_number: Some(2),
            message: Some("hallo welt".to_string()),
            history: None,
        };
        let (prior, target, is_ctx) = legacy.resolve_turns().unwrap();
        assert!(prior.is_empty());
        assert_eq!(target.role, Role::Partner);
        assert_eq!(target.partner_number, Some(2));
        assert_eq!(target.message, "hallo welt");
        assert_eq!(target.user_likelihood, None);
        assert!(!is_ctx);

        // 2. Contextual history takes precedence over legacy role/message
        let contextual = CorrectTranscriptRequest {
            role: Some(Role::Partner),
            partner_number: Some(9),
            message: Some("ignored".to_string()),
            history: Some(vec![
                CorrectTranscriptHistoryItem {
                    role: Role::Partner,
                    message: "Guten Tag.".to_string(),
                    partner_number: Some(1),
                    cluster_id: Some("cluster_1".to_string()),
                    user_likelihood: Some(0.05),
                    pause_ms: None,
                },
                CorrectTranscriptHistoryItem {
                    role: Role::PartnerSimple,
                    message: "Guten Tag.".to_string(),
                    partner_number: None,
                    cluster_id: None,
                    user_likelihood: None,
                    pause_ms: None,
                },
                CorrectTranscriptHistoryItem {
                    role: Role::User,
                    message: "ich habe einen termin".to_string(),
                    partner_number: None,
                    cluster_id: Some("cluster_0".to_string()),
                    user_likelihood: Some(0.29),
                    pause_ms: Some(540),
                },
            ]),
        };
        let (prior_ctx, target_ctx, is_ctx_flag) = contextual.resolve_turns().unwrap();
        assert_eq!(prior_ctx.len(), 2);
        assert_eq!(prior_ctx[0].partner_number, Some(1));
        assert_eq!(target_ctx.role, Role::User);
        assert_eq!(target_ctx.partner_number, None);
        assert_eq!(target_ctx.message, "ich habe einen termin");
        assert_eq!(target_ctx.user_likelihood, Some(0.29));
        assert_eq!(target_ctx.pause_ms, Some(540));
        assert!(is_ctx_flag);

        // 3. Rejections: empty history, blank message, partner_simple as target, invalid partnerNumber, invalid userLikelihood
        assert!(CorrectTranscriptRequest {
            role: None,
            partner_number: None,
            message: None,
            history: Some(vec![]),
        }
        .resolve_turns()
        .is_err());

        assert!(CorrectTranscriptRequest {
            role: Some(Role::PartnerSimple),
            partner_number: None,
            message: Some("hallo".to_string()),
            history: None,
        }
        .resolve_turns()
        .is_err());

        assert!(CorrectTranscriptRequest {
            role: Some(Role::Partner),
            partner_number: Some(0),
            message: Some("hallo".to_string()),
            history: None,
        }
        .resolve_turns()
        .is_err());

        assert!(CorrectTranscriptRequest {
            role: Some(Role::User),
            partner_number: Some(1),
            message: Some("hallo".to_string()),
            history: None,
        }
        .resolve_turns()
        .is_err());

        assert!(CorrectTranscriptRequest {
            role: None,
            partner_number: None,
            message: None,
            history: Some(vec![CorrectTranscriptHistoryItem {
                role: Role::PartnerSimple,
                message: "hallo".to_string(),
                partner_number: None,
                cluster_id: None,
                user_likelihood: None,
                pause_ms: None,
            }]),
        }
        .resolve_turns()
        .is_err());

        assert!(CorrectTranscriptRequest {
            role: None,
            partner_number: None,
            message: None,
            history: Some(vec![CorrectTranscriptHistoryItem {
                role: Role::Partner,
                message: "hallo".to_string(),
                partner_number: None,
                cluster_id: None,
                user_likelihood: Some(1.2),
                pause_ms: None,
            }]),
        }
        .resolve_turns()
        .is_err());
    }
}
