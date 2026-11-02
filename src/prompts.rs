use crate::api::{
    ConversationMessage, CorrectTranscriptHistoryItem, PartnerRole, Role, TranslationType, Warning,
    WarningType,
};
pub use crate::few_shots::count_gemma_tokens;
use crate::few_shots::FewShotEntry;
use rig::client::CompletionClient;
use rig::completion::CompletionRequestBuilder;
use rig::providers::gemini;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use utoipa::ToSchema;

/// System prompt for `/correct-transcript`.
pub const CORRECT_TRANSCRIPT_SYSTEM_PROMPT: &str =
    include_str!("../prompts/correct-transcript-system.md");

/// User prompt template for `/correct-transcript`.
pub const CORRECT_TRANSCRIPT_PROMPT_TEMPLATE: &str =
    include_str!("../prompts/correct-transcript.md");

/// JSON Schema string defining the structured output of `/correct-transcript` (`prompts/correct-transcript-schema.json`).
pub const CORRECT_TRANSCRIPT_SCHEMA_JSON: &str =
    include_str!("../prompts/correct-transcript-schema.json");

/// Parsed JSON Schema value passed to `generationConfig.responseJsonSchema` during `/correct-transcript`.
pub static CORRECT_TRANSCRIPT_RESPONSE_SCHEMA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(CORRECT_TRANSCRIPT_SCHEMA_JSON)
        .expect("prompts/correct-transcript-schema.json must be valid JSON")
});

/// System prompt for `/translate` (`prompts/translate-system.md`).
pub const TRANSLATE_SYSTEM_PROMPT: &str = include_str!("../prompts/translate-system.md");

/// Zero-shot user prompt template for `/translate` (`prompts/translate.md`).
pub const TRANSLATE_PROMPT_TEMPLATE: &str = include_str!("../prompts/translate.md");

/// Dynamic few-shot user prompt template for `/translate` (`prompts/translate-dynamic-few-shots.md`).
pub const TRANSLATE_DYNAMIC_FEW_SHOTS_PROMPT_TEMPLATE: &str =
    include_str!("../prompts/translate-dynamic-few-shots.md");

/// Raw system prompt template for dialog classification (`prompts/classify-dialog-system.md`).
const CLASSIFY_DIALOG_SYSTEM_PROMPT_TEMPLATE: &str =
    include_str!("../prompts/classify-dialog-system.md");

/// JSON Schema string defining the structured output of dialog classification (`prompts/classify-dialog-schema.json`).
pub const CLASSIFY_DIALOG_SCHEMA_JSON: &str =
    include_str!("../prompts/classify-dialog-schema.json");

/// Few-shot examples injected into the dialog classification system prompt (`prompts/classify-dialog-few-shots.md`).
pub const CLASSIFY_DIALOG_FEW_SHOTS: &str = include_str!("../prompts/classify-dialog-few-shots.md");

/// User prompt template for dialog classification (`prompts/classify-dialog.md`).
pub const CLASSIFY_DIALOG_PROMPT_TEMPLATE: &str = include_str!("../prompts/classify-dialog.md");

/// Rendered system prompt for dialog classification with schema and few-shot placeholders expanded.
pub static CLASSIFY_DIALOG_SYSTEM_PROMPT: LazyLock<String> = LazyLock::new(|| {
    CLASSIFY_DIALOG_SYSTEM_PROMPT_TEMPLATE
        .replace(
            "%classify-dialog-schema.json%",
            CLASSIFY_DIALOG_SCHEMA_JSON.trim(),
        )
        .replace(
            "%classify-dialog-few-shots.md%",
            CLASSIFY_DIALOG_FEW_SHOTS.trim(),
        )
});

/// Parsed JSON Schema value passed to `generationConfig.responseJsonSchema` during dialog classification.
pub static CLASSIFY_DIALOG_RESPONSE_SCHEMA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(CLASSIFY_DIALOG_SCHEMA_JSON)
        .expect("prompts/classify-dialog-schema.json must be valid JSON")
});

/// Static warning Markdown for `info` / `uncertain` (`prompts/warnings/uncertain.md`).
pub const WARNING_UNCERTAIN_MD: &str = include_str!("../prompts/warnings/uncertain.md");

/// Static warning Markdown for `warning` / `medical` (`prompts/warnings/medical.md`).
pub const WARNING_MEDICAL_MD: &str = include_str!("../prompts/warnings/medical.md");

/// Static warning Markdown for `warning` / `financial` (`prompts/warnings/financial.md`).
pub const WARNING_FINANCIAL_MD: &str = include_str!("../prompts/warnings/financial.md");

/// Static warning Markdown for `warning` / `juridical` (`prompts/warnings/juridical.md`).
pub const WARNING_JURIDICAL_MD: &str = include_str!("../prompts/warnings/juridical.md");

/// Static warning Markdown for `warning` / `authorities` (`prompts/warnings/authorities.md`).
pub const WARNING_AUTHORITIES_MD: &str = include_str!("../prompts/warnings/authorities.md");

/// Static warning Markdown for `danger` / `hate-speech` (`prompts/warnings/hate-speech.md`).
pub const WARNING_HATE_SPEECH_MD: &str = include_str!("../prompts/warnings/hate-speech.md");

/// Static warning Markdown for `danger` / `injection` (`prompts/warnings/injection.md`).
pub const WARNING_INJECTION_MD: &str = include_str!("../prompts/warnings/injection.md");

/// Static warning Markdown for `danger` / `safety` (`prompts/warnings/safety.md`).
pub const WARNING_SAFETY_MD: &str = include_str!("../prompts/warnings/safety.md");

/// Static warning Markdown for `danger` / `youth-protection` (`prompts/warnings/youth-protection.md`).
pub const WARNING_YOUTH_PROTECTION_MD: &str =
    include_str!("../prompts/warnings/youth-protection.md");

/// All recognized staging directive tokens that can be embedded in a message when `ENVIRONMENT=staging`.
const STAGING_DIRECTIVE_TOKENS: &[&str] = &[
    "%type=formatting%",
    "%type=translation%",
    "%warning.type=danger%",
    "%warning.type=warning%",
    "%warning.type=info%",
    "%warning.revealable=true%",
    "%warning.revealable=false%",
    "%warning.revealable%",
];

/// High-level topic of a dialog extracted by the classification step for monitoring and analytics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DialogTopic {
    Medical,
    Shopping,
    Social,
    Education,
    Emergency,
    Authorities,
    Financial,
    Mobility,
    Juridical,
    Other,
}

impl DialogTopic {
    /// Returns the canonical lowercase string label used in JSON, logs, and OpenTelemetry attributes.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Medical => "medical",
            Self::Shopping => "shopping",
            Self::Social => "social",
            Self::Education => "education",
            Self::Emergency => "emergency",
            Self::Authorities => "authorities",
            Self::Financial => "financial",
            Self::Mobility => "mobility",
            Self::Juridical => "juridical",
            Self::Other => "other",
        }
    }
}

/// Severity level returned by the dialog classification prompt (`pass`, `info`, `warning`, or `danger`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClassificationLevel {
    Pass,
    Info,
    Warning,
    Danger,
}

impl ClassificationLevel {
    /// Returns the canonical lowercase string label used in JSON, logs, and OpenTelemetry attributes.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Danger => "danger",
        }
    }
}

/// Specific reason category returned by the dialog classification prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClassificationCategory {
    Ok,
    Uncertain,
    Medical,
    Financial,
    Juridical,
    Authorities,
    Safety,
    HateSpeech,
    YouthProtection,
    Injection,
}

impl ClassificationCategory {
    /// Returns the canonical kebab-case string label used in JSON, logs, and OpenTelemetry attributes.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Uncertain => "uncertain",
            Self::Medical => "medical",
            Self::Financial => "financial",
            Self::Juridical => "juridical",
            Self::Authorities => "authorities",
            Self::Safety => "safety",
            Self::HateSpeech => "hate-speech",
            Self::YouthProtection => "youth-protection",
            Self::Injection => "injection",
        }
    }
}

/// Raw partner role inference returned by the dialog classification model (`roleType` + `confidence`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPartnerRole {
    /// Optional inferred German role label (e.g. `"ÄrztIn"`, `"Arzt"`, `"SachbearbeiterIn"`) or `None`.
    pub role_type: Option<String>,
    /// Confidence score in `[0.0, 1.0]`.
    pub confidence: f64,
}

/// Structured output produced by the dialog classification prompt (`type`, `category`, `topic`, and optional `partnerRole`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DialogClassification {
    /// Severity tier (`pass`, `info`, `warning`, or `danger`).
    #[serde(rename = "type")]
    pub r#type: ClassificationLevel,
    /// Specific disclaimer or rejection category paired with `type`.
    pub category: ClassificationCategory,
    /// Overall topic of the conversation (`"other"` fallback).
    pub topic: DialogTopic,
    /// Optional contextual role inference for the speaking partner of `# Aussage`.
    #[serde(default, rename = "partnerRole")]
    pub partner_role: Option<ModelPartnerRole>,
}

impl DialogClassification {
    /// Checks whether `(type, category)` forms a valid pair according to `prompts/classify-dialog-schema.json`.
    pub fn is_valid_hierarchy(&self) -> bool {
        matches!(
            (self.r#type, self.category),
            (ClassificationLevel::Pass, ClassificationCategory::Ok)
                | (ClassificationLevel::Info, ClassificationCategory::Uncertain)
                | (
                    ClassificationLevel::Warning,
                    ClassificationCategory::Medical
                        | ClassificationCategory::Financial
                        | ClassificationCategory::Juridical
                        | ClassificationCategory::Authorities
                )
                | (
                    ClassificationLevel::Danger,
                    ClassificationCategory::Safety
                        | ClassificationCategory::HateSpeech
                        | ClassificationCategory::YouthProtection
                        | ClassificationCategory::Injection
                )
        )
    }

    /// Normalizes and validates `partner_role` into an API [`PartnerRole`], returning `None` when
    /// `role_type` is absent, empty, longer than 40 characters, a generic placeholder, or when
    /// `confidence` is `<= 0.0` or non-finite.
    pub fn normalized_partner_role(&self) -> Option<PartnerRole> {
        let raw = self.partner_role.as_ref()?;
        let role_type = raw.role_type.as_deref()?.trim();
        if role_type.is_empty() || role_type.chars().count() > 40 {
            return None;
        }
        let lower = role_type.to_lowercase();
        if matches!(
            lower.as_str(),
            "gesprächspartnerin"
                | "gesprächspartner"
                | "partner"
                | "person"
                | "unbekannt"
                | "other"
                | "null"
        ) {
            return None;
        }
        if !raw.confidence.is_finite() {
            return None;
        }
        let confidence = raw.confidence.clamp(0.0, 1.0);
        if confidence <= 0.0 {
            return None;
        }
        Some(PartnerRole {
            role_type: role_type.to_string(),
            confidence,
        })
    }
}

/// Upstream LLM token counts for input, reasoning, and output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub reasoning_tokens: u64,
    pub output_tokens: u64,
}

/// Output of a prompt execution including response text, optional reasoning thoughts, and token usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOutput {
    pub text: String,
    pub reasoning: Option<String>,
    pub usage: TokenUsage,
}

/// Error returned when a prompt execution against the language model fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptError(pub String);

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PromptError {}

/// Trait abstracting prompt execution so production uses Gemini via `rig` and tests can inject mocks.
pub trait PromptRunner: Send + Sync {
    /// Executes a prompt with the given system preamble and thinking level (e.g. `"minimal"`, `"low"`, or `"medium"`).
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>>;

    /// Executes a prompt requesting structured JSON output conforming to `json_schema` when supported by the provider.
    fn run_prompt_with_schema<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
        _json_schema: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        self.run_prompt(system_prompt, prompt, thinking_level)
    }
}

/// Production [`PromptRunner`] that sends requests to Google's Gemini API (`gemini-3.5-flash-lite`) using `rig`.
pub struct GeminiPromptRunner {
    api_key: String,
}

impl GeminiPromptRunner {
    /// Creates a new [`GeminiPromptRunner`] authenticated with the provided Gemini API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    /// Sends a prompt request to Gemini with optional structured JSON schema enforcement in `generationConfig`.
    fn execute_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
        json_schema: Option<&'a serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        Box::pin(async move {
            tracing::debug!(
                thinking_level,
                structured_json = json_schema.is_some(),
                system_prompt = %system_prompt,
                prompt = %prompt,
                "Sending prompt to LLM API"
            );

            let client = gemini::Client::builder()
                .api_key(&self.api_key)
                .build()
                .map_err(|e| PromptError(e.to_string()))?;
            let model = client.completion_model("gemini-3.5-flash-lite");

            let mut builder = CompletionRequestBuilder::new(model, prompt.to_string())
                .preamble(system_prompt.to_string());

            let mut generation_config = serde_json::Map::new();
            generation_config.insert("temperature".to_string(), serde_json::json!(0.0));

            let mut thinking_config = serde_json::Map::new();
            thinking_config.insert(
                "thinkingLevel".to_string(),
                serde_json::json!(thinking_level),
            );
            thinking_config.insert("includeThoughts".to_string(), serde_json::json!(true));

            generation_config.insert(
                "thinkingConfig".to_string(),
                serde_json::Value::Object(thinking_config),
            );

            if let Some(schema) = json_schema {
                generation_config.insert(
                    "responseMimeType".to_string(),
                    serde_json::json!("application/json"),
                );
                generation_config.insert("responseJsonSchema".to_string(), schema.clone());
            }

            let additional_params = serde_json::json!({
                "generationConfig": generation_config
            });
            builder = builder.additional_params(additional_params);

            let res = builder
                .send()
                .await
                .map_err(|e| PromptError(e.to_string()))?;

            let mut response = String::new();
            let mut reasoning_text = String::new();
            for part in res.choice {
                match part {
                    rig::completion::AssistantContent::Reasoning(reasoning) => {
                        let thought_string = reasoning.display_text();
                        let thought_text = thought_string.trim();
                        tracing::debug!(reasoning = %thought_text, "Received LLM reasoning thought");
                        if !reasoning_text.is_empty() {
                            reasoning_text.push('\n');
                        }
                        reasoning_text.push_str(thought_text);
                    }
                    rig::completion::AssistantContent::ToolCall(tool) => {
                        tracing::warn!(
                            tool = %tool.function.name,
                            args = %tool.function.arguments,
                            "Unexpected ToolCall received from LLM model"
                        );
                    }
                    rig::completion::AssistantContent::Text(text) => {
                        if !response.is_empty() {
                            response.push('\n');
                        }
                        response.push_str(&text.text);
                    }
                    _ => {}
                }
            }

            tracing::debug!(response = %response, "Received LLM response text");

            let input_tokens = res.usage.input_tokens;
            let output_tokens = res.usage.output_tokens;
            let reasoning_tokens = res.usage.reasoning_tokens;

            Ok(PromptOutput {
                text: response,
                reasoning: if reasoning_text.is_empty() {
                    None
                } else {
                    Some(reasoning_text)
                },
                usage: TokenUsage {
                    input_tokens,
                    reasoning_tokens,
                    output_tokens,
                },
            })
        })
    }
}

impl PromptRunner for GeminiPromptRunner {
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        self.execute_prompt(system_prompt, prompt, thinking_level, None)
    }

    fn run_prompt_with_schema<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
        json_schema: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        self.execute_prompt(system_prompt, prompt, thinking_level, Some(json_schema))
    }
}

/// Verified speaker role returned by the `/correct-transcript` model (`user` or `partner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResolvedSpeakerRole {
    User,
    Partner,
}

impl From<ResolvedSpeakerRole> for Role {
    fn from(value: ResolvedSpeakerRole) -> Self {
        match value {
            ResolvedSpeakerRole::User => Role::User,
            ResolvedSpeakerRole::Partner => Role::Partner,
        }
    }
}

/// Structured JSON output produced by the `/correct-transcript` model (`message`, `resolvedRole`, `mergeWithPrevious`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrectTranscriptModelOutput {
    /// Corrected transcript of the current turn (or unified text when `merge_with_previous` is `true`).
    pub message: String,
    /// Verified speaker role (`user` or `partner`).
    pub resolved_role: ResolvedSpeakerRole,
    /// Whether this turn is a direct continuation of the immediately preceding spoken turn by the same speaker.
    pub merge_with_previous: bool,
}

/// Fallback [`PromptRunner`] used in local debug/test builds when `GEMINI_API_KEY` is not set.
/// Extracts the final ```input block from the prompt and echoes it back deterministically
/// (emitting structured JSON for `CORRECT_TRANSCRIPT_SYSTEM_PROMPT` and `CLASSIFY_DIALOG_SYSTEM_PROMPT`),
/// tokenized using the Gemma 4 tokenizer.
#[derive(Debug, Default, Clone)]
pub struct EchoPromptRunner;

impl PromptRunner for EchoPromptRunner {
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        _thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        Box::pin(async move {
            let input = extract_input_block(prompt).unwrap_or(prompt);
            let mocked_reasoning = format!("I am thinking about '{input}'");
            let output_text = if system_prompt == TRANSLATE_SYSTEM_PROMPT {
                format!("FORMATIERUNG\n{input}")
            } else if system_prompt == CLASSIFY_DIALOG_SYSTEM_PROMPT.as_str() {
                r#"{"type":"pass","category":"ok","topic":"other","partnerRole":{"roleType":null,"confidence":0.0}}"#.to_string()
            } else if system_prompt == CORRECT_TRANSCRIPT_SYSTEM_PROMPT {
                let resolved_role = extract_provisional_role(prompt);
                serde_json::to_string(&CorrectTranscriptModelOutput {
                    message: input.to_string(),
                    resolved_role,
                    merge_with_previous: false,
                })
                .expect("Failed to serialize EchoPromptRunner CorrectTranscriptModelOutput")
            } else {
                input.to_string()
            };

            let combined_input = format!("{system_prompt}\n{prompt}");
            let input_tokens = count_gemma_tokens(&combined_input);
            let reasoning_tokens = count_gemma_tokens(&mocked_reasoning);
            let output_tokens = count_gemma_tokens(&output_text);

            tracing::debug!(
                input_tokens,
                reasoning_tokens,
                output_tokens,
                reasoning = %mocked_reasoning,
                output = %output_text,
                "EchoPromptRunner executed mock prompt"
            );

            Ok(PromptOutput {
                text: output_text,
                reasoning: Some(mocked_reasoning),
                usage: TokenUsage {
                    input_tokens,
                    reasoning_tokens,
                    output_tokens,
                },
            })
        })
    }
}

/// Extracts the provisional speaker role (`user` or `partner`) from a rendered `/correct-transcript` prompt.
fn extract_provisional_role(prompt: &str) -> ResolvedSpeakerRole {
    if prompt.contains("- Vorläufige Rolle: user") {
        ResolvedSpeakerRole::User
    } else {
        ResolvedSpeakerRole::Partner
    }
}

/// Extracts the text inside the last ` ```input ... ``` ` fence of a rendered prompt template.
fn extract_input_block(prompt: &str) -> Option<&str> {
    let marker = "```input\n";
    let start = prompt.rfind(marker)? + marker.len();
    let rest = &prompt[start..];
    let end = rest.find("\n```")?;
    Some(&rest[..end])
}

/// Replaces triple backticks (` ``` `) with triple single quotes (`'''`) to prevent code fence breakout.
pub fn sanitize(content: &str) -> String {
    content.replace("```", "'''")
}

/// Returns the effective acoustic user likelihood in `[0.0, 1.0]` for a [`CorrectTranscriptHistoryItem`],
/// mapping omitted `user_likelihood` deterministically from `role` (`1.0` for `User`, `0.0` for `Partner` / `PartnerSimple`).
pub fn effective_user_likelihood(item: &CorrectTranscriptHistoryItem) -> f32 {
    item.user_likelihood
        .unwrap_or(match item.role {
            Role::User => 1.0,
            Role::Partner | Role::PartnerSimple => 0.0,
        })
        .clamp(0.0, 1.0)
}

/// Slices `history` so at most the last 5 `user`/`partner` turns (and any attached `partner_simple`
/// entries following them) are retained for `/correct-transcript` context.
pub fn cap_correct_transcript_history_turns(
    history: &[CorrectTranscriptHistoryItem],
) -> &[CorrectTranscriptHistoryItem] {
    let mut turn_count = 0usize;
    let mut start_idx = 0usize;

    for (idx, item) in history.iter().enumerate().rev() {
        if matches!(item.role, Role::User | Role::Partner) {
            turn_count += 1;
            if turn_count == 5 {
                start_idx = idx;
                break;
            }
        }
    }

    &history[start_idx..]
}

/// Formats prior [`CorrectTranscriptHistoryItem`] entries for `# Gesprächsverlauf` in `/correct-transcript`,
/// capping to the last 5 `user`/`partner` turns and including optional partner numbers and acoustic metadata tags.
/// Returns `"keine Historie"` when `prior_items` is empty.
pub fn format_correct_transcript_history(prior_items: &[CorrectTranscriptHistoryItem]) -> String {
    let capped = cap_correct_transcript_history_turns(prior_items);
    if capped.is_empty() {
        return "keine Historie".to_string();
    }

    let mut last_partner_num: Option<u32> = None;
    let mut blocks = Vec::with_capacity(capped.len());

    for entry in capped {
        let msg = sanitize(&entry.message);
        match entry.role {
            Role::PartnerSimple => {
                let n = entry.partner_number.or(last_partner_num);
                match n {
                    Some(num) => blocks.push(format!("Partner {num} (Leichte Sprache): {msg}")),
                    None => blocks.push(format!("Partner (Leichte Sprache): {msg}")),
                }
            }
            Role::User | Role::Partner => {
                let role_label = if entry.role == Role::User {
                    last_partner_num = None;
                    "User".to_string()
                } else {
                    last_partner_num = entry.partner_number;
                    match entry.partner_number {
                        Some(num) => format!("Partner {num}"),
                        None => "Partner".to_string(),
                    }
                };
                let likelihood = effective_user_likelihood(entry);
                let mut meta_parts = Vec::with_capacity(3);
                if let Some(ref cid) = entry.cluster_id {
                    let trimmed_cid = sanitize(cid).trim().to_string();
                    if !trimmed_cid.is_empty() {
                        meta_parts.push(format!("clusterId={trimmed_cid}"));
                    }
                }
                meta_parts.push(format!("userLikelihood={likelihood:.2}"));
                if let Some(pause) = entry.pause_ms {
                    meta_parts.push(format!("pauseMs={pause}"));
                }
                blocks.push(format!("{role_label} [{}]: {msg}", meta_parts.join(", ")));
            }
        }
    }

    blocks.join("\n\n")
}

/// Renders the contextual user prompt for `/correct-transcript` (`prompts/correct-transcript.md`)
/// from `prior_items` and the `target` turn to correct.
pub fn build_correct_transcript_contextual_prompt(
    prior_items: &[CorrectTranscriptHistoryItem],
    target: &CorrectTranscriptHistoryItem,
) -> String {
    let history_str = format_correct_transcript_history(prior_items);
    let provisional_role = match (target.role, target.partner_number) {
        (Role::User, _) => "user".to_string(),
        (Role::Partner | Role::PartnerSimple, Some(n)) => format!("partner (Partner {n})"),
        (Role::Partner | Role::PartnerSimple, None) => "partner".to_string(),
    };
    let cluster_id_str = target
        .cluster_id
        .as_deref()
        .map(sanitize)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "unbekannt".to_string());
    let likelihood_str = format!("{:.2}", effective_user_likelihood(target));
    let pause_str = target
        .pause_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_else(|| "unbekannt".to_string());

    CORRECT_TRANSCRIPT_PROMPT_TEMPLATE
        .replace("%HISTORY%", &history_str)
        .replace("%PROVISIONAL_ROLE%", &provisional_role)
        .replace("%CLUSTER_ID%", cluster_id_str.trim())
        .replace("%USER_LIKELIHOOD%", &likelihood_str)
        .replace("%PAUSE_MS%", &pause_str)
        .replace("%INPUT%", &sanitize(&target.message))
}

/// Renders the user prompt for `/correct-transcript` for a single legacy `input` message (`Role::Partner` default).
pub fn build_correct_transcript_prompt(input: &str) -> String {
    let target = CorrectTranscriptHistoryItem {
        role: Role::Partner,
        message: input.to_string(),
        partner_number: None,
        cluster_id: None,
        user_likelihood: None,
        pause_ms: None,
    };
    build_correct_transcript_contextual_prompt(&[], &target)
}

/// Parses and validates the structured JSON output of `/correct-transcript` into a [`CorrectTranscriptModelOutput`].
///
/// Strips optional surrounding Markdown code fences (e.g. ` ```json ... ``` `) and deserializes the JSON payload.
pub fn parse_correct_transcript_output(
    raw: &str,
) -> Result<CorrectTranscriptModelOutput, PromptError> {
    let mut trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        if let Some(inner) = rest.strip_suffix("```") {
            trimmed = inner.trim();
        }
    }

    serde_json::from_str(trimmed).map_err(|e| {
        PromptError(format!(
            "Failed to parse correct-transcript JSON output: {e}"
        ))
    })
}

/// Applies deterministic acoustic and structural guardrails to a [`CorrectTranscriptModelOutput`]:
///
/// 1. Forces `merge_with_previous = false` when:
///    - there is no preceding spoken (`user` or `partner`) turn in `prior_items`,
///    - the target turn carries no diarization metadata (`cluster_id`, `user_likelihood`, and `pause_ms` are all `None`), or
///    - the preceding spoken turn and target turn have contradictory acoustic locks (`>= 0.80` vs `<= 0.10`).
/// 2. Enforces acoustic confidence locks on `resolved_role`:
///    - When `!merge_with_previous`: effective `user_likelihood >= 0.80` locks `Role::User`; `<= 0.10` locks `Role::Partner`.
///    - When `merge_with_previous`: if either the preceding spoken turn or target turn is acoustically locked (`>= 0.80` or `<= 0.10`), locks `resolved_role` to that role.
/// 3. Guarantees a non-empty `message` string (falling back to `target.message` if empty or if a contradictory-lock merge veto had to discard a combined message).
pub fn enforce_correct_transcript_guardrails(
    raw_output: CorrectTranscriptModelOutput,
    prior_items: &[CorrectTranscriptHistoryItem],
    target: &CorrectTranscriptHistoryItem,
) -> (String, Role, bool) {
    let prior_spoken = prior_items
        .iter()
        .rev()
        .find(|item| matches!(item.role, Role::User | Role::Partner));

    let has_diarization_meta = target.cluster_id.is_some()
        || target.user_likelihood.is_some()
        || target.pause_ms.is_some();

    let target_p = effective_user_likelihood(target);
    let mut merge =
        raw_output.merge_with_previous && has_diarization_meta && prior_spoken.is_some();
    let mut vetoed_contradictory_merge = false;

    if let (true, Some(prev)) = (merge, prior_spoken) {
        let prev_p = effective_user_likelihood(prev);
        let contradictory =
            (prev_p >= 0.80 && target_p <= 0.10) || (prev_p <= 0.10 && target_p >= 0.80);
        if contradictory {
            merge = false;
            vetoed_contradictory_merge = true;
        }
    }

    let mut resolved_role: Role = raw_output.resolved_role.into();
    if merge {
        if let Some(prev) = prior_spoken {
            let prev_p = effective_user_likelihood(prev);
            if prev_p >= 0.80 || target_p >= 0.80 {
                resolved_role = Role::User;
            } else if prev_p <= 0.10 || target_p <= 0.10 {
                resolved_role = Role::Partner;
            }
        }
    } else if target_p >= 0.80 {
        resolved_role = Role::User;
    } else if target_p <= 0.10 {
        resolved_role = Role::Partner;
    }

    let trimmed_msg = raw_output.message.trim();
    let message = if trimmed_msg.is_empty() || vetoed_contradictory_merge {
        target.message.trim().to_string()
    } else {
        trimmed_msg.to_string()
    };

    (message, resolved_role, merge)
}

/// Counts the number of `user` and `partner` turns in `history` (excluding `partner_simple` translations).
pub fn count_history_turns(history: &[ConversationMessage]) -> usize {
    history
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Partner))
        .count()
}

/// Determines the history-length bucket (`0..=5`) for a conversation history slice.
pub fn determine_history_bucket(history: &[ConversationMessage]) -> usize {
    count_history_turns(history).min(5)
}

/// Slices `history` so at most the last 5 `user`/`partner` turns (and any attached `partner_simple`
/// messages following them) are retained.
pub fn cap_history_turns(history: &[ConversationMessage]) -> &[ConversationMessage] {
    let mut turn_count = 0usize;
    let mut start_idx = 0usize;

    for (idx, msg) in history.iter().enumerate().rev() {
        if matches!(msg.role, Role::User | Role::Partner) {
            turn_count += 1;
            if turn_count == 5 {
                start_idx = idx;
                break;
            }
        }
    }

    &history[start_idx..]
}

/// Formats the prior conversation messages into `User: ...`, `Partner [n]: ...`, and
/// `Partner [n] (Leichte Sprache): ...` blocks (capped to the last 5 `user`/`partner` turns),
/// or `"keine Historie"` when empty.
pub fn format_translate_history(history: &[ConversationMessage]) -> String {
    let capped = cap_history_turns(history);
    if capped.is_empty() {
        return "keine Historie".to_string();
    }

    let mut last_partner_num: Option<u32> = None;
    let mut blocks = Vec::with_capacity(capped.len());

    for entry in capped {
        let prefix = match entry.role {
            Role::User => {
                last_partner_num = None;
                "User:".to_string()
            }
            Role::Partner => {
                last_partner_num = entry.partner_number;
                match entry.partner_number {
                    Some(num) => format!("Partner {num}:"),
                    None => "Partner:".to_string(),
                }
            }
            Role::PartnerSimple => {
                let n = entry.partner_number.or(last_partner_num);
                match n {
                    Some(num) => format!("Partner {num} (Leichte Sprache):"),
                    None => "Partner (Leichte Sprache):".to_string(),
                }
            }
        };
        blocks.push(format!("{} {}", prefix, sanitize(&entry.message)));
    }

    blocks.join("\n\n")
}

/// Renders the user prompt for `/translate` without few-shot examples (zero-shot fallback).
pub fn build_translate_prompt(history: &[ConversationMessage], input: &str) -> String {
    build_translate_prompt_with_few_shots(history, input, &[])
}

/// Renders the user prompt for `/translate` by injecting the retrieved dynamic few-shot examples,
/// the formatted `history` (or `"keine Historie"`), and the sanitized `input`.
/// Falls back to [`TRANSLATE_PROMPT_TEMPLATE`] when `examples` is empty.
pub fn build_translate_prompt_with_few_shots(
    history: &[ConversationMessage],
    input: &str,
    examples: &[FewShotEntry],
) -> String {
    build_translate_prompt_with_few_shots_for_partner(history, input, None, examples)
}

/// Renders the user prompt for `/translate` with an optional speaking `partner_number` (`Partner {n}:` header inside `input`).
/// Falls back to [`TRANSLATE_PROMPT_TEMPLATE`] when `examples` is empty.
pub fn build_translate_prompt_with_few_shots_for_partner(
    history: &[ConversationMessage],
    input: &str,
    partner_number: Option<u32>,
    examples: &[FewShotEntry],
) -> String {
    let history_value = format_translate_history(history);
    let sanitized_input = sanitize(input);
    let input_partner_header = match partner_number {
        Some(n) => format!("Partner {n}:\n"),
        None => String::new(),
    };

    if examples.is_empty() {
        return TRANSLATE_PROMPT_TEMPLATE
            .replace("%HISTORY%", &history_value)
            .replace("%INPUT_PARTNER_HEADER%", &input_partner_header)
            .replace("%INPUT%", &sanitized_input);
    }

    let few_shots_str = examples
        .iter()
        .map(|ex| ex.sample.trim())
        .collect::<Vec<_>>()
        .join("\n\n");

    TRANSLATE_DYNAMIC_FEW_SHOTS_PROMPT_TEMPLATE
        .replace("%FEW_SHOTS%", &few_shots_str)
        .replace("%HISTORY%", &history_value)
        .replace("%INPUT_PARTNER_HEADER%", &input_partner_header)
        .replace("%INPUT%", &sanitized_input)
}

/// Parses the raw output from the `/translate` model into a [`TranslationType`] and Markdown content.
///
/// The model is instructed to output `ÜBERSETZUNG` or `FORMATIERUNG` on the first line,
/// followed by the prepared Markdown content on subsequent lines.
pub fn parse_translate_output(raw: &str) -> (TranslationType, String) {
    let trimmed = raw.trim();
    if let Some((first_line, rest)) = trimmed.split_once('\n') {
        let header = first_line.trim();
        if header == "FORMATIERUNG" {
            return (TranslationType::Formatting, rest.trim().to_string());
        }
        if header == "ÜBERSETZUNG" {
            return (TranslationType::Translation, rest.trim().to_string());
        }
    } else if trimmed == "FORMATIERUNG" {
        return (TranslationType::Formatting, String::new());
    } else if trimmed == "ÜBERSETZUNG" {
        return (TranslationType::Translation, String::new());
    }

    (TranslationType::Translation, trimmed.to_string())
}

/// Renders the user prompt for the dialog classification step (`prompts/classify-dialog.md`)
/// by injecting the formatted `history`, sanitized original `input`, and sanitized `translation`.
pub fn build_classify_dialog_prompt(
    history: &[ConversationMessage],
    input: &str,
    translation: &str,
) -> String {
    build_classify_dialog_prompt_for_partner(history, input, translation, None)
}

/// Renders the user prompt for the dialog classification step (`prompts/classify-dialog.md`)
/// with an optional speaking `partner_number` (`Partner {n}:` and `Partner {n} (leichte Sprache):`).
pub fn build_classify_dialog_prompt_for_partner(
    history: &[ConversationMessage],
    input: &str,
    translation: &str,
    partner_number: Option<u32>,
) -> String {
    let history_value = format_translate_history(history);
    let (partner_label, partner_simple_label) = match partner_number {
        Some(n) => (
            format!("Partner {n}"),
            format!("Partner {n} (leichte Sprache)"),
        ),
        None => (
            "Partner".to_string(),
            "Partner (leichte Sprache)".to_string(),
        ),
    };
    CLASSIFY_DIALOG_PROMPT_TEMPLATE
        .replace("%HISTORY%", &history_value)
        .replace("%PARTNER_LABEL%", &partner_label)
        .replace("%PARTNER_SIMPLE_LABEL%", &partner_simple_label)
        .replace("%INPUT%", &sanitize(input))
        .replace("%TRANSLATION%", &sanitize(translation))
}

/// Parses and validates the JSON output of the dialog classification model into a [`DialogClassification`].
///
/// Strips optional surrounding Markdown code fences (e.g. ` ```json ... ``` `), deserializes the JSON
/// payload, and enforces the `(type, category)` hierarchy defined in `prompts/classify-dialog-schema.json`.
pub fn parse_classify_dialog_output(raw: &str) -> Result<DialogClassification, PromptError> {
    let mut trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        if let Some(inner) = rest.strip_suffix("```") {
            trimmed = inner.trim();
        }
    }

    let parsed: DialogClassification = serde_json::from_str(trimmed).map_err(|e| {
        PromptError(format!(
            "Failed to parse dialog classification JSON output: {e}"
        ))
    })?;

    if !parsed.is_valid_hierarchy() {
        return Err(PromptError(format!(
            "Invalid classification hierarchy pair: ({}, {})",
            parsed.r#type.as_str(),
            parsed.category.as_str()
        )));
    }

    Ok(parsed)
}

/// Parses an embedded static warning Markdown file (`<headline>\n\n<body>`) into its plain-text
/// headline and Leichte Sprache Markdown body while preserving internal double-space line breaks.
pub fn parse_warning_markdown(raw: &str) -> (String, String) {
    let normalized = raw.replace("\r\n", "\n");
    if let Some((headline, body)) = normalized.split_once("\n\n") {
        (headline.trim().to_string(), body.trim().to_string())
    } else {
        (normalized.trim().to_string(), String::new())
    }
}

/// Builds a [`Warning`] struct from an embedded static warning Markdown template, severity level, and `revealable` flag.
fn build_static_warning(
    raw_markdown: &str,
    warning_type: WarningType,
    revealable: bool,
) -> Warning {
    let (headline, body) = parse_warning_markdown(raw_markdown);
    Warning {
        headline,
        body,
        r#type: warning_type,
        revealable,
    }
}

/// Applies the [`DialogClassification`] policy to the prepared translation:
/// - `pass` (`ok`): omits `warning` and keeps `translated_content` immediately visible.
/// - `info` (`uncertain`): attaches an `info` box with `revealable: false` and keeps `translated_content` visible.
/// - `warning` (`medical`, `financial`, `juridical`, `authorities`): attaches an EU AI Act / RDG `warning` box with `revealable: true`.
/// - `danger` (`hate-speech`, `injection`): attaches a `danger` box with `revealable: true`.
/// - `danger` (`safety`, `youth-protection`): blocks the translation by returning an empty `content` (`""`),
///   `TranslationType::Translation`, and a `danger` box with `revealable: false`.
pub fn apply_classification(
    classification: &DialogClassification,
    model_type: TranslationType,
    translated_content: String,
) -> (TranslationType, String, Option<Warning>) {
    match (classification.r#type, classification.category) {
        (ClassificationLevel::Pass, _) => (model_type, translated_content, None),
        (ClassificationLevel::Info, _) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_UNCERTAIN_MD,
                WarningType::Info,
                false,
            )),
        ),
        (ClassificationLevel::Warning, ClassificationCategory::Medical) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_MEDICAL_MD,
                WarningType::Warning,
                true,
            )),
        ),
        (ClassificationLevel::Warning, ClassificationCategory::Financial) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_FINANCIAL_MD,
                WarningType::Warning,
                true,
            )),
        ),
        (ClassificationLevel::Warning, ClassificationCategory::Juridical) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_JURIDICAL_MD,
                WarningType::Warning,
                true,
            )),
        ),
        (ClassificationLevel::Warning, _) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_AUTHORITIES_MD,
                WarningType::Warning,
                true,
            )),
        ),
        (ClassificationLevel::Danger, ClassificationCategory::Safety) => (
            TranslationType::Translation,
            String::new(),
            Some(build_static_warning(
                WARNING_SAFETY_MD,
                WarningType::Danger,
                false,
            )),
        ),
        (ClassificationLevel::Danger, ClassificationCategory::YouthProtection) => (
            TranslationType::Translation,
            String::new(),
            Some(build_static_warning(
                WARNING_YOUTH_PROTECTION_MD,
                WarningType::Danger,
                false,
            )),
        ),
        (ClassificationLevel::Danger, ClassificationCategory::Injection) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_INJECTION_MD,
                WarningType::Danger,
                true,
            )),
        ),
        (ClassificationLevel::Danger, _) => (
            model_type,
            translated_content,
            Some(build_static_warning(
                WARNING_HATE_SPEECH_MD,
                WarningType::Danger,
                true,
            )),
        ),
    }
}

/// Parsed staging directives and the message text with directive tokens stripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingDirectives {
    /// Optional override for the response [`TranslationType`].
    pub r#type: Option<TranslationType>,
    /// Optional content warning constructed from `%warning...%` directives.
    pub warning: Option<Warning>,
    /// Message text with staging directive markers removed (or original if stripping leaves it blank).
    pub cleaned_message: String,
}

/// Extracts staging test directives (`%type=...%`, `%warning.type=...%`, `%warning.revealable%`)
/// when `is_staging` is `true`, and strips those tokens from `cleaned_message` before sending to the LLM.
/// When `is_staging` is `false`, directives are ignored and the original message is returned unchanged.
pub fn extract_staging_directives(message: &str, is_staging: bool) -> StagingDirectives {
    if !is_staging {
        return StagingDirectives {
            r#type: None,
            warning: None,
            cleaned_message: message.to_string(),
        };
    }

    let r#type = if message.contains("%type=formatting%") {
        Some(TranslationType::Formatting)
    } else if message.contains("%type=translation%") {
        Some(TranslationType::Translation)
    } else {
        None
    };

    let warning_type = if message.contains("%warning.type=danger%") {
        Some(WarningType::Danger)
    } else if message.contains("%warning.type=warning%") {
        Some(WarningType::Warning)
    } else if message.contains("%warning.type=info%") {
        Some(WarningType::Info)
    } else {
        None
    };

    let revealable = if message.contains("%warning.revealable%")
        || message.contains("%warning.revealable=true%")
    {
        Some(true)
    } else if message.contains("%warning.revealable=false%") {
        Some(false)
    } else {
        None
    };

    let warning = if warning_type.is_some() || revealable.is_some() {
        Some(Warning {
            headline: "Zu Ihrer Information".to_string(),
            body: "Passen Sie auf sich auf!".to_string(),
            r#type: warning_type.unwrap_or(WarningType::Info),
            revealable: revealable.unwrap_or(false),
        })
    } else {
        None
    };

    let mut cleaned = message.to_string();
    for token in STAGING_DIRECTIVE_TOKENS {
        cleaned = cleaned.replace(token, "");
    }
    let cleaned_trimmed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned_message = if cleaned_trimmed.is_empty() {
        message.to_string()
    } else {
        cleaned_trimmed
    };

    StagingDirectives {
        r#type,
        warning,
        cleaned_message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `sanitize` replaces triple backticks with triple single quotes.
    #[test]
    fn test_sanitize() {
        assert_eq!(sanitize("normal text"), "normal text");
        assert_eq!(sanitize("code ```block``` here"), "code '''block''' here");
    }

    /// Verifies that `build_correct_transcript_prompt` and `build_correct_transcript_contextual_prompt`
    /// inject sanitized history, acoustic metadata, and input with no unreplaced placeholders.
    #[test]
    fn test_build_correct_transcript_prompt() {
        let prompt = build_correct_transcript_prompt("hallo ```welt```");
        assert!(prompt.contains("```history\nkeine Historie\n```"));
        assert!(prompt.contains("- Vorläufige Rolle: partner"));
        assert!(prompt.contains("- Akustische Cluster-ID: unbekannt"));
        assert!(prompt.contains("- Akustische Nutzer-Wahrscheinlichkeit (`userLikelihood`): 0.00"));
        assert!(prompt.contains("- Sprechpause zur vorherigen Aussage (`pauseMs`): unbekannt"));
        assert!(prompt.contains("```input\nhallo '''welt'''\n```"));
        assert!(!prompt.contains("%INPUT%"));
        assert!(!prompt.contains("%HISTORY%"));
        assert!(!prompt.contains("%PROVISIONAL_ROLE%"));
        assert!(!prompt.contains("%CLUSTER_ID%"));
        assert!(!prompt.contains("%USER_LIKELIHOOD%"));
        assert!(!prompt.contains("%PAUSE_MS%"));

        let prior = vec![
            CorrectTranscriptHistoryItem {
                role: Role::Partner,
                message: "Nehmen Sie Ibuprofen oder L-Thyroxin?".to_string(),
                partner_number: None,
                cluster_id: Some("cluster_1".to_string()),
                user_likelihood: Some(0.06),
                pause_ms: None,
            },
            CorrectTranscriptHistoryItem {
                role: Role::PartnerSimple,
                message: "Nehmen Sie Ibuprofen oder L-Thyroxin?".to_string(),
                partner_number: None,
                cluster_id: None,
                user_likelihood: None,
                pause_ms: None,
            },
        ];
        let target = CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "ich nehme l tür rock sin".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_0".to_string()),
            user_likelihood: Some(0.29),
            pause_ms: Some(520),
        };
        let ctx_prompt = build_correct_transcript_contextual_prompt(&prior, &target);
        assert!(ctx_prompt.contains(
            "Partner [clusterId=cluster_1, userLikelihood=0.06]: Nehmen Sie Ibuprofen oder L-Thyroxin?"
        ));
        assert!(
            ctx_prompt.contains("Partner (Leichte Sprache): Nehmen Sie Ibuprofen oder L-Thyroxin?")
        );
        assert!(ctx_prompt.contains("- Vorläufige Rolle: partner"));
        assert!(ctx_prompt.contains("- Akustische Cluster-ID: cluster_0"));
        assert!(
            ctx_prompt.contains("- Akustische Nutzer-Wahrscheinlichkeit (`userLikelihood`): 0.29")
        );
        assert!(ctx_prompt.contains("- Sprechpause zur vorherigen Aussage (`pauseMs`): 520 ms"));
        assert!(ctx_prompt.contains("```input\nich nehme l tür rock sin\n```"));

        // Numbered partner in history and target
        let numbered_prior = vec![
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
        ];
        let numbered_target = CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "und hier ist ihr rezept".to_string(),
            partner_number: Some(2),
            cluster_id: Some("cluster_2".to_string()),
            user_likelihood: Some(0.08),
            pause_ms: Some(610),
        };
        let numbered_prompt =
            build_correct_transcript_contextual_prompt(&numbered_prior, &numbered_target);
        assert!(numbered_prompt
            .contains("Partner 1 [clusterId=cluster_1, userLikelihood=0.05]: Guten Tag."));
        assert!(numbered_prompt.contains("Partner 1 (Leichte Sprache): Guten Tag."));
        assert!(numbered_prompt.contains("- Vorläufige Rolle: partner (Partner 2)"));
    }

    /// Verifies that `parse_correct_transcript_output` and `enforce_correct_transcript_guardrails`
    /// enforce acoustic locks (`>= 0.80` -> `user`, `<= 0.10` -> `partner`), allow borderline flips
    /// (`0.10 < p < 0.80`), and veto illegal or contradictory `mergeWithPrevious` flags.
    #[test]
    fn test_parse_and_enforce_correct_transcript_guardrails() {
        let schema = &*CORRECT_TRANSCRIPT_RESPONSE_SCHEMA;
        assert_eq!(schema["title"], "CorrectTranscriptOutput");

        // 1. Parse raw & fenced JSON; reject unknown fields or invalid roles
        let parsed_fenced = parse_correct_transcript_output(
            "```json\n{\"message\":\"Ich nehme L-Thyroxin.\",\"resolvedRole\":\"user\",\"mergeWithPrevious\":false}\n```",
        )
        .unwrap();
        assert_eq!(parsed_fenced.message, "Ich nehme L-Thyroxin.");
        assert_eq!(parsed_fenced.resolved_role, ResolvedSpeakerRole::User);
        assert!(!parsed_fenced.merge_with_previous);

        assert!(parse_correct_transcript_output(
            r#"{"message":"Hallo","resolvedRole":"partner_simple","mergeWithPrevious":false}"#
        )
        .is_err());
        assert!(parse_correct_transcript_output(
            r#"{"message":"Hallo","resolvedRole":"user","mergeWithPrevious":false,"extra":1}"#
        )
        .is_err());

        // 2. Legacy / no-diarization target forces merge = false and locks role via mapped 0.0 / 1.0 confidence
        let legacy_target = CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "roher text".to_string(),
            partner_number: None,
            cluster_id: None,
            user_likelihood: None,
            pause_ms: None,
        };
        let (msg, role, merge) = enforce_correct_transcript_guardrails(
            CorrectTranscriptModelOutput {
                message: "Korrigierter Text.".to_string(),
                resolved_role: ResolvedSpeakerRole::User, // hallucinated flip
                merge_with_previous: true,                // hallucinated merge
            },
            &[],
            &legacy_target,
        );
        assert_eq!(msg, "Korrigierter Text.");
        assert_eq!(role, Role::Partner);
        assert!(!merge);

        // 3. Uncertain zone (0.29) allows LLM role flip (partner -> user)
        let prior = vec![CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "Wie kann ich helfen?".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_1".to_string()),
            user_likelihood: Some(0.05),
            pause_ms: None,
        }];
        let uncertain_target = CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "ich habe einen termin".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_0".to_string()),
            user_likelihood: Some(0.29),
            pause_ms: Some(480),
        };
        let (msg2, role2, merge2) = enforce_correct_transcript_guardrails(
            CorrectTranscriptModelOutput {
                message: "Ich habe einen Termin.".to_string(),
                resolved_role: ResolvedSpeakerRole::User,
                merge_with_previous: false,
            },
            &prior,
            &uncertain_target,
        );
        assert_eq!(msg2, "Ich habe einen Termin.");
        assert_eq!(role2, Role::User);
        assert!(!merge2);

        // 4. High-confidence user (0.88) locks Role::User even if LLM returns Partner
        let locked_user_target = CorrectTranscriptHistoryItem {
            role: Role::User,
            message: "ja genau".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_0".to_string()),
            user_likelihood: Some(0.88),
            pause_ms: Some(300),
        };
        let (_, role3, _) = enforce_correct_transcript_guardrails(
            CorrectTranscriptModelOutput {
                message: "Ja, genau.".to_string(),
                resolved_role: ResolvedSpeakerRole::Partner,
                merge_with_previous: false,
            },
            &prior,
            &locked_user_target,
        );
        assert_eq!(role3, Role::User);

        // 5. Valid split-turn merge (prev 0.85 User + target 0.34 uncertain -> merged Role::User)
        let prior_user = vec![CorrectTranscriptHistoryItem {
            role: Role::User,
            message: "Warum kann er nur Sailor, aber nicht.".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_0".to_string()),
            user_likelihood: Some(0.85),
            pause_ms: None,
        }];
        let split_tail = CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "sailors".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_1".to_string()),
            user_likelihood: Some(0.34),
            pause_ms: Some(350),
        };
        let (msg4, role4, merge4) = enforce_correct_transcript_guardrails(
            CorrectTranscriptModelOutput {
                message: "Warum kann er nur Sailor, aber nicht Sailors?".to_string(),
                resolved_role: ResolvedSpeakerRole::Partner, // locked prev 0.85 forces User!
                merge_with_previous: true,
            },
            &prior_user,
            &split_tail,
        );
        assert_eq!(msg4, "Warum kann er nur Sailor, aber nicht Sailors?");
        assert_eq!(role4, Role::User);
        assert!(merge4);

        // 6. Contradictory acoustic locks (prev 0.85 User vs target 0.04 Partner) veto merge and fall back to target message
        let contradictory_tail = CorrectTranscriptHistoryItem {
            role: Role::Partner,
            message: "Das weiß ich nicht.".to_string(),
            partner_number: None,
            cluster_id: Some("cluster_1".to_string()),
            user_likelihood: Some(0.04),
            pause_ms: Some(250),
        };
        let (msg5, role5, merge5) = enforce_correct_transcript_guardrails(
            CorrectTranscriptModelOutput {
                message: "Warum kann er nur Sailor, aber nicht Das weiß ich nicht.".to_string(),
                resolved_role: ResolvedSpeakerRole::User,
                merge_with_previous: true,
            },
            &prior_user,
            &contradictory_tail,
        );
        assert_eq!(msg5, "Das weiß ich nicht.");
        assert_eq!(role5, Role::Partner);
        assert!(!merge5);
    }

    /// Verifies `format_translate_history`, `count_history_turns`, `determine_history_bucket`, `partner_simple` capping,
    /// and raw Speech-to-Text guidance in the zero-shot fallback prompt.
    #[test]
    fn test_build_translate_prompt_and_partner_simple_history() {
        assert_eq!(format_translate_history(&[]), "keine Historie");
        assert_eq!(determine_history_bucket(&[]), 0);

        let history = vec![
            ConversationMessage {
                role: Role::User,
                message: "Frage 1".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Antwort 1".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "ÜBERSETZUNG\nEinfache Antwort 1".to_string(),
                partner_number: None,
            },
        ];
        // partner_simple does not increment turn count
        assert_eq!(count_history_turns(&history), 2);
        assert_eq!(determine_history_bucket(&history), 2);

        let prompt = build_translate_prompt(&history, "Neue Aussage");
        assert!(prompt.contains("rohes, unkorrigiertes Speech-to-Text-Transkript"));
        assert!(prompt.contains(
            "User: Frage 1\n\nPartner: Antwort 1\n\nPartner (Leichte Sprache): ÜBERSETZUNG\nEinfache Antwort 1"
        ));
        assert!(prompt.contains("```input\nNeue Aussage\n```"));
        assert!(!prompt.contains("%HISTORY%"));
        assert!(!prompt.contains("%INPUT_PARTNER_HEADER%"));
        assert!(!prompt.contains("%INPUT%"));

        // Numbered partners in history and target
        let numbered_history = vec![
            ConversationMessage {
                role: Role::Partner,
                message: "Antwort von Partner 1".to_string(),
                partner_number: Some(1),
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "ÜBERSETZUNG\nEinfache Antwort 1".to_string(),
                partner_number: None,
            },
        ];
        let numbered_prompt = build_translate_prompt_with_few_shots_for_partner(
            &numbered_history,
            "Aussage von Partner 2",
            Some(2),
            &[],
        );
        assert!(numbered_prompt.contains(
            "Partner 1: Antwort von Partner 1\n\nPartner 1 (Leichte Sprache): ÜBERSETZUNG\nEinfache Antwort 1"
        ));
        assert!(numbered_prompt.contains("Partner 2:\n```input\nAussage von Partner 2\n```"));
    }

    /// Verifies that `cap_history_turns` retains at most the last 5 `user`/`partner` turns along with their `partner_simple` translations.
    #[test]
    fn test_cap_history_turns_to_last_5() {
        let history = vec![
            ConversationMessage {
                role: Role::User,
                message: "Turn 1 (dropped)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Turn 2 (dropped)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "Simple 2 (dropped)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::User,
                message: "Turn 3 (kept 1)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Turn 4 (kept 2)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "Simple 4 (kept)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::User,
                message: "Turn 5 (kept 3)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Turn 6 (kept 4)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "Simple 6 (kept)".to_string(),
                partner_number: None,
            },
            ConversationMessage {
                role: Role::User,
                message: "Turn 7 (kept 5)".to_string(),
                partner_number: None,
            },
        ];

        assert_eq!(count_history_turns(&history), 7);
        assert_eq!(determine_history_bucket(&history), 5);

        let formatted = format_translate_history(&history);
        assert!(!formatted.contains("Turn 1 (dropped)"));
        assert!(!formatted.contains("Turn 2 (dropped)"));
        assert!(!formatted.contains("Simple 2 (dropped)"));
        assert!(formatted.contains("User: Turn 3 (kept 1)"));
        assert!(formatted.contains("Partner (Leichte Sprache): Simple 4 (kept)"));
        assert!(formatted.contains("User: Turn 7 (kept 5)"));
    }

    /// Verifies that `build_translate_prompt_with_few_shots` injects `%FEW_SHOTS%`, contrasts clean few-shot transcripts
    /// with raw STT input, and that `EchoPromptRunner` extracts the final `input` block rather than a few-shot's `input` block.
    #[tokio::test]
    async fn test_build_translate_prompt_with_few_shots_and_echo() {
        let sample_entry = FewShotEntry {
            id: "dialog-01_00".to_string(),
            dialog: "dialog-01.txt".to_string(),
            exchange_idx: 0,
            turn_idx: 0,
            bucket: 0,
            kind: "ÜBERSETZUNG".to_string(),
            input: "Beispiel Eingabe".to_string(),
            output: "ÜBERSETZUNG\nBeispiel Ausgabe".to_string(),
            history: "keine Historie".to_string(),
            sample: "### Beispiel\n\n```input\nBeispiel Eingabe\n```\n\n```output\nÜBERSETZUNG\nBeispiel Ausgabe\n```".to_string(),
            tokens: 20,
            embedding: vec![],
        };

        let rendered =
            build_translate_prompt_with_few_shots(&[], "Echte Nutzer Eingabe", &[sample_entry]);
        assert!(rendered
            .contains("Hier sind Beispiele für Übersetzungen von Dialogen in Leichte Sprache:"));
        assert!(rendered.contains("Beispiel Eingabe"));
        assert!(rendered.contains("# Deine Übersetzung"));
        assert!(rendered.contains(
            "In den Beispielen oben liegen alle Transkripte bereits sprachlich bereinigt vor."
        ));
        assert!(rendered.contains("rohes, unkorrigiertes Speech-to-Text-Transkript"));
        assert!(rendered.contains("```input\nEchte Nutzer Eingabe\n```"));

        let runner = EchoPromptRunner;
        let out = runner
            .run_prompt(TRANSLATE_SYSTEM_PROMPT, &rendered, "medium")
            .await
            .unwrap();
        assert_eq!(out.text, "FORMATIERUNG\nEchte Nutzer Eingabe");
    }

    /// Verifies that `TRANSLATE_SYSTEM_PROMPT` contains the raw STT transcript correction guidance
    /// and core translation rules without unreplaced placeholders or direct references to model reasoning.
    #[test]
    fn test_translate_system_prompt() {
        assert!(TRANSLATE_SYSTEM_PROMPT.contains("# Eingabe (Rohes Sprach-Transkript)"));
        assert!(TRANSLATE_SYSTEM_PROMPT
            .contains("Prüfe auf Basis der sprachlich korrigierten Aussage von Partner"));
        assert!(TRANSLATE_SYSTEM_PROMPT.contains("# Regeln für Leichte Sprache"));
        assert!(TRANSLATE_SYSTEM_PROMPT.contains("ÜBERSETZUNG oder FORMATIERUNG"));
        assert!(!TRANSLATE_SYSTEM_PROMPT.contains("%translate-few-shots.md%"));

        for template in [
            TRANSLATE_SYSTEM_PROMPT,
            TRANSLATE_PROMPT_TEMPLATE,
            TRANSLATE_DYNAMIC_FEW_SHOTS_PROMPT_TEMPLATE,
        ] {
            let lower = template.to_lowercase();
            assert!(!lower.contains("reasoning"));
            assert!(!lower.contains("chain-of-thought"));
        }
    }

    /// Verifies that `parse_translate_output` extracts `TranslationType` from the first line (`ÜBERSETZUNG` vs `FORMATIERUNG`).
    #[test]
    fn test_parse_translate_output() {
        let (t1, c1) = parse_translate_output("ÜBERSETZUNG\nEinfacher Satz 1.\nEinfacher Satz 2.");
        assert_eq!(t1, TranslationType::Translation);
        assert_eq!(c1, "Einfacher Satz 1.\nEinfacher Satz 2.");

        let (t2, c2) = parse_translate_output("FORMATIERUNG\nOriginaler Satz.");
        assert_eq!(t2, TranslationType::Formatting);
        assert_eq!(c2, "Originaler Satz.");

        let (t3, c3) = parse_translate_output("Ohne Präfix Text");
        assert_eq!(t3, TranslationType::Translation);
        assert_eq!(c3, "Ohne Präfix Text");
    }

    /// Verifies that `CLASSIFY_DIALOG_SYSTEM_PROMPT` expands schema and few-shot placeholders and that
    /// `CLASSIFY_DIALOG_RESPONSE_SCHEMA` parses cleanly with all required categories and topics.
    #[test]
    fn test_classify_dialog_system_prompt_and_schema() {
        let sys = CLASSIFY_DIALOG_SYSTEM_PROMPT.as_str();
        assert!(!sys.contains("%classify-dialog-schema.json%"));
        assert!(!sys.contains("%classify-dialog-few-shots.md%"));
        assert!(sys.contains("\"DialogClassification\""));
        assert!(sys.contains("\"juridical\""));
        assert!(sys.contains("\"authorities\""));
        assert!(sys.contains("\"injection\""));
        assert!(sys.contains("\"mobility\""));
        assert!(sys.contains("## Eingabe 1"));
        assert!(sys.contains("\"topic\": \"medical\""));

        let schema = &*CLASSIFY_DIALOG_RESPONSE_SCHEMA;
        assert_eq!(schema["title"], "DialogClassification");
        let topics = schema["$defs"]["topic"]["enum"]
            .as_array()
            .expect("topic enum array");
        assert_eq!(topics.len(), 10);
    }

    /// Verifies that `build_classify_dialog_prompt` injects sanitized history, input, and translation without leftover placeholders.
    #[test]
    fn test_build_classify_dialog_prompt() {
        let history = vec![ConversationMessage {
            role: Role::User,
            message: "Hallo ```Welt```".to_string(),
            partner_number: None,
        }];
        let rendered = build_classify_dialog_prompt(
            &history,
            "Original ```input```",
            "Einfache ```Übersetzung```",
        );
        assert!(rendered.contains("User: Hallo '''Welt'''"));
        assert!(rendered.contains("Partner:\n```input\nOriginal '''input'''\n```"));
        assert!(rendered.contains(
            "Partner (leichte Sprache):\n```translation\nEinfache '''Übersetzung'''\n```"
        ));
        assert!(!rendered.contains("%HISTORY%"));
        assert!(!rendered.contains("%PARTNER_LABEL%"));
        assert!(!rendered.contains("%PARTNER_SIMPLE_LABEL%"));
        assert!(!rendered.contains("%INPUT%"));
        assert!(!rendered.contains("%TRANSLATION%"));

        let numbered = build_classify_dialog_prompt_for_partner(
            &history,
            "Original",
            "Einfache Übersetzung",
            Some(2),
        );
        assert!(numbered.contains("Partner 2:\n```input\nOriginal\n```"));
        assert!(numbered
            .contains("Partner 2 (leichte Sprache):\n```translation\nEinfache Übersetzung\n```"));
    }

    /// Verifies that `parse_classify_dialog_output` accepts all valid `(type, category, topic)` combinations
    /// (including optional Markdown ```json fences and `partnerRole`) and rejects invalid pairs, unknown fields, or malformed JSON.
    #[test]
    fn test_parse_classify_dialog_output() {
        let valid_cases = [
            (
                r#"{"type":"pass","category":"ok","topic":"other"}"#,
                ClassificationLevel::Pass,
                ClassificationCategory::Ok,
                DialogTopic::Other,
            ),
            (
                "```json\n{\"type\": \"info\", \"category\": \"uncertain\", \"topic\": \"shopping\", \"partnerRole\": {\"roleType\": \"VerkäuferIn\", \"confidence\": 0.91}}\n```",
                ClassificationLevel::Info,
                ClassificationCategory::Uncertain,
                DialogTopic::Shopping,
            ),
            (
                r#"{"type":"warning","category":"medical","topic":"medical","partnerRole":{"roleType":"Arzt","confidence":0.95}}"#,
                ClassificationLevel::Warning,
                ClassificationCategory::Medical,
                DialogTopic::Medical,
            ),
            (
                r#"{"type":"warning","category":"financial","topic":"financial"}"#,
                ClassificationLevel::Warning,
                ClassificationCategory::Financial,
                DialogTopic::Financial,
            ),
            (
                r#"{"type":"warning","category":"juridical","topic":"juridical"}"#,
                ClassificationLevel::Warning,
                ClassificationCategory::Juridical,
                DialogTopic::Juridical,
            ),
            (
                r#"{"type":"warning","category":"authorities","topic":"authorities"}"#,
                ClassificationLevel::Warning,
                ClassificationCategory::Authorities,
                DialogTopic::Authorities,
            ),
            (
                r#"{"type":"danger","category":"safety","topic":"emergency"}"#,
                ClassificationLevel::Danger,
                ClassificationCategory::Safety,
                DialogTopic::Emergency,
            ),
            (
                r#"{"type":"danger","category":"hate-speech","topic":"social"}"#,
                ClassificationLevel::Danger,
                ClassificationCategory::HateSpeech,
                DialogTopic::Social,
            ),
            (
                r#"{"type":"danger","category":"youth-protection","topic":"education"}"#,
                ClassificationLevel::Danger,
                ClassificationCategory::YouthProtection,
                DialogTopic::Education,
            ),
            (
                r#"{"type":"danger","category":"injection","topic":"mobility"}"#,
                ClassificationLevel::Danger,
                ClassificationCategory::Injection,
                DialogTopic::Mobility,
            ),
        ];

        for (raw, expected_type, expected_cat, expected_topic) in valid_cases {
            let parsed = parse_classify_dialog_output(raw).expect(raw);
            assert_eq!(parsed.r#type, expected_type);
            assert_eq!(parsed.category, expected_cat);
            assert_eq!(parsed.topic, expected_topic);
        }

        // Verify normalized_partner_role sanitization
        let with_role = parse_classify_dialog_output(
            r#"{"type":"warning","category":"medical","topic":"medical","partnerRole":{"roleType":"  ÄrztIn  ","confidence":0.92}}"#,
        )
        .unwrap();
        assert_eq!(
            with_role.normalized_partner_role(),
            Some(PartnerRole {
                role_type: "ÄrztIn".to_string(),
                confidence: 0.92,
            })
        );

        let generic_placeholder = parse_classify_dialog_output(
            r#"{"type":"pass","category":"ok","topic":"other","partnerRole":{"roleType":"GesprächspartnerIn","confidence":0.95}}"#,
        )
        .unwrap();
        assert_eq!(generic_placeholder.normalized_partner_role(), None);

        let zero_confidence = parse_classify_dialog_output(
            r#"{"type":"pass","category":"ok","topic":"other","partnerRole":{"roleType":null,"confidence":0.0}}"#,
        )
        .unwrap();
        assert_eq!(zero_confidence.normalized_partner_role(), None);

        // Invalid hierarchy pairings must fail closed
        assert!(parse_classify_dialog_output(
            r#"{"type":"pass","category":"medical","topic":"other"}"#
        )
        .is_err());
        assert!(parse_classify_dialog_output(
            r#"{"type":"warning","category":"safety","topic":"other"}"#
        )
        .is_err());
        assert!(parse_classify_dialog_output(
            r#"{"type":"danger","category":"ok","topic":"other"}"#
        )
        .is_err());
        // Unknown topic or extra property must fail closed
        assert!(parse_classify_dialog_output(
            r#"{"type":"pass","category":"ok","topic":"sports"}"#
        )
        .is_err());
        assert!(parse_classify_dialog_output(
            r#"{"type":"pass","category":"ok","topic":"other","extra":true}"#
        )
        .is_err());
    }

    /// Verifies that all 9 embedded `prompts/warnings/*.md` files parse into non-empty Leichte Sprache
    /// headlines and bodies with preserved Markdown double-space line breaks, and that `apply_classification`
    /// enforces visibility (`revealable`) and empty-answer blocking (`safety`, `youth-protection`).
    #[test]
    fn test_static_warnings_and_apply_classification() {
        let all_warning_files = [
            WARNING_UNCERTAIN_MD,
            WARNING_MEDICAL_MD,
            WARNING_FINANCIAL_MD,
            WARNING_JURIDICAL_MD,
            WARNING_AUTHORITIES_MD,
            WARNING_HATE_SPEECH_MD,
            WARNING_INJECTION_MD,
            WARNING_SAFETY_MD,
            WARNING_YOUTH_PROTECTION_MD,
        ];
        for md in all_warning_files {
            let (headline, body) = parse_warning_markdown(md);
            assert!(!headline.is_empty(), "Warning headline must not be empty");
            assert!(
                !headline.contains('\n'),
                "Headline must be a single plain-text line"
            );
            assert!(!body.is_empty(), "Warning body must not be empty");
            assert!(
                body.contains("  \n"),
                "Leichte Sprache warning body should preserve trailing double-space line breaks"
            );
            assert!(
                !headline.contains('!') && !body.contains('!'),
                "Leichte Sprache warning texts should avoid exclamation marks"
            );
        }

        // 1. pass / ok -> no warning, content preserved
        let (t, c, w) = apply_classification(
            &DialogClassification {
                r#type: ClassificationLevel::Pass,
                category: ClassificationCategory::Ok,
                topic: DialogTopic::Medical,
                partner_role: None,
            },
            TranslationType::Formatting,
            "Einfacher Text".to_string(),
        );
        assert_eq!(t, TranslationType::Formatting);
        assert_eq!(c, "Einfacher Text");
        assert!(w.is_none());

        // 2. info / uncertain -> Info box, revealable = false, content preserved
        let (t, c, w) = apply_classification(
            &DialogClassification {
                r#type: ClassificationLevel::Info,
                category: ClassificationCategory::Uncertain,
                topic: DialogTopic::Other,
                partner_role: None,
            },
            TranslationType::Translation,
            "Unsicherer Text".to_string(),
        );
        assert_eq!(t, TranslationType::Translation);
        assert_eq!(c, "Unsicherer Text");
        let w = w.expect("Expected info warning");
        assert_eq!(w.r#type, WarningType::Info);
        assert!(!w.revealable);
        assert_eq!(w.headline, "Unsichere Übersetzung");

        // 3. warning / medical, financial, juridical, authorities -> Warning box, revealable = true, content preserved
        for cat in [
            ClassificationCategory::Medical,
            ClassificationCategory::Financial,
            ClassificationCategory::Juridical,
            ClassificationCategory::Authorities,
        ] {
            let (t, c, w) = apply_classification(
                &DialogClassification {
                    r#type: ClassificationLevel::Warning,
                    category: cat,
                    topic: DialogTopic::Juridical,
                    partner_role: None,
                },
                TranslationType::Translation,
                "Erklärender Text".to_string(),
            );
            assert_eq!(t, TranslationType::Translation);
            assert_eq!(c, "Erklärender Text");
            let w = w.expect("Expected EU AI Act warning");
            assert_eq!(w.r#type, WarningType::Warning);
            assert!(w.revealable);
        }

        // 4. danger / hate-speech & injection -> Danger box, revealable = true, content preserved
        for cat in [
            ClassificationCategory::HateSpeech,
            ClassificationCategory::Injection,
        ] {
            let (t, c, w) = apply_classification(
                &DialogClassification {
                    r#type: ClassificationLevel::Danger,
                    category: cat,
                    topic: DialogTopic::Social,
                    partner_role: None,
                },
                TranslationType::Translation,
                "Ausgeblendeter Text".to_string(),
            );
            assert_eq!(t, TranslationType::Translation);
            assert_eq!(c, "Ausgeblendeter Text");
            let w = w.expect("Expected revealable danger warning");
            assert_eq!(w.r#type, WarningType::Danger);
            assert!(w.revealable);
        }

        // 5. danger / safety & youth-protection -> empty content "", forced TranslationType::Translation, revealable = false
        for cat in [
            ClassificationCategory::Safety,
            ClassificationCategory::YouthProtection,
        ] {
            let (t, c, w) = apply_classification(
                &DialogClassification {
                    r#type: ClassificationLevel::Danger,
                    category: cat,
                    topic: DialogTopic::Emergency,
                    partner_role: None,
                },
                TranslationType::Formatting,
                "Verbotener Text".to_string(),
            );
            assert_eq!(t, TranslationType::Translation);
            assert_eq!(c, "");
            let w = w.expect("Expected blocking danger warning");
            assert_eq!(w.r#type, WarningType::Danger);
            assert!(!w.revealable);
        }
    }

    /// Verifies that `extract_staging_directives` parses and strips directives in staging and leaves production messages untouched.
    #[test]
    fn test_extract_staging_directives() {
        let msg = "Bitte beachten %type=translation% %warning.type=warning% %warning.revealable%";

        let prod = extract_staging_directives(msg, false);
        assert_eq!(prod.r#type, None);
        assert_eq!(prod.warning, None);
        assert_eq!(prod.cleaned_message, msg);

        let staging = extract_staging_directives(msg, true);
        assert_eq!(staging.r#type, Some(TranslationType::Translation));
        assert_eq!(
            staging.warning,
            Some(Warning {
                headline: "Zu Ihrer Information".to_string(),
                body: "Passen Sie auf sich auf!".to_string(),
                r#type: WarningType::Warning,
                revealable: true,
            })
        );
        assert_eq!(staging.cleaned_message, "Bitte beachten");
    }

    /// Verifies that `EchoPromptRunner` accurately counts Gemma 4 tokens, mocks reasoning thoughts,
    /// and emits valid structured JSON for both `CORRECT_TRANSCRIPT_SYSTEM_PROMPT` and `CLASSIFY_DIALOG_SYSTEM_PROMPT`.
    #[tokio::test]
    async fn test_echo_prompt_runner_tokens_and_reasoning() {
        let runner = EchoPromptRunner;
        let prompt = build_correct_transcript_prompt("Hallo Welt");
        let output = runner
            .run_prompt_with_schema(
                CORRECT_TRANSCRIPT_SYSTEM_PROMPT,
                &prompt,
                "minimal",
                &CORRECT_TRANSCRIPT_RESPONSE_SCHEMA,
            )
            .await
            .unwrap();

        let parsed_ct = parse_correct_transcript_output(&output.text).unwrap();
        assert_eq!(parsed_ct.message, "Hallo Welt");
        assert_eq!(parsed_ct.resolved_role, ResolvedSpeakerRole::Partner);
        assert!(!parsed_ct.merge_with_previous);
        assert_eq!(
            output.reasoning,
            Some("I am thinking about 'Hallo Welt'".to_string())
        );
        assert!(output.usage.input_tokens > 0);
        assert!(output.usage.reasoning_tokens > 0);
        assert!(output.usage.output_tokens > 0);

        let classify_prompt = build_classify_dialog_prompt(&[], "Hallo Welt", "Hallo Welt");
        let classify_out = runner
            .run_prompt_with_schema(
                &CLASSIFY_DIALOG_SYSTEM_PROMPT,
                &classify_prompt,
                "low",
                &CLASSIFY_DIALOG_RESPONSE_SCHEMA,
            )
            .await
            .unwrap();
        let parsed = parse_classify_dialog_output(&classify_out.text).unwrap();
        assert_eq!(parsed.r#type, ClassificationLevel::Pass);
        assert_eq!(parsed.category, ClassificationCategory::Ok);
        assert_eq!(parsed.topic, DialogTopic::Other);
    }
}
