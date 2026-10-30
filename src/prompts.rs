use crate::api::{ConversationMessage, Role, TranslationType, Warning, WarningType};
pub use crate::few_shots::count_gemma_tokens;
use crate::few_shots::FewShotEntry;
use rig::client::CompletionClient;
use rig::completion::CompletionRequestBuilder;
use rig::providers::gemini;
use std::future::Future;
use std::pin::Pin;

/// System prompt for `/correct-transcript`.
pub const CORRECT_TRANSCRIPT_SYSTEM_PROMPT: &str =
    include_str!("../prompts/correct-transcript-system.md");

/// User prompt template for `/correct-transcript`.
pub const CORRECT_TRANSCRIPT_PROMPT_TEMPLATE: &str =
    include_str!("../prompts/correct-transcript.md");

/// System prompt for `/translate` (`prompts/translate-system.md`).
pub const TRANSLATE_SYSTEM_PROMPT: &str = include_str!("../prompts/translate-system.md");

/// Zero-shot user prompt template for `/translate` (`prompts/translate.md`).
pub const TRANSLATE_PROMPT_TEMPLATE: &str = include_str!("../prompts/translate.md");

/// Dynamic few-shot user prompt template for `/translate` (`prompts/translate-dynamic-few-shots.md`).
pub const TRANSLATE_DYNAMIC_FEW_SHOTS_PROMPT_TEMPLATE: &str =
    include_str!("../prompts/translate-dynamic-few-shots.md");

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
}

impl PromptRunner for GeminiPromptRunner {
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        Box::pin(async move {
            tracing::debug!(
                thinking_level,
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

/// Fallback [`PromptRunner`] used in local debug/test builds when `GEMINI_API_KEY` is not set.
/// Extracts the final ```input block from the prompt and echoes it back deterministically,
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

/// Renders the user prompt for `/correct-transcript` by injecting the sanitized `input` message.
pub fn build_correct_transcript_prompt(input: &str) -> String {
    CORRECT_TRANSCRIPT_PROMPT_TEMPLATE.replace("%INPUT%", &sanitize(input))
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

/// Formats the prior conversation messages into `User: ...`, `Partner: ...`, and
/// `Partner (Leichte Sprache): ...` blocks (capped to the last 5 `user`/`partner` turns),
/// or `"keine Historie"` when empty.
pub fn format_translate_history(history: &[ConversationMessage]) -> String {
    let capped = cap_history_turns(history);
    if capped.is_empty() {
        return "keine Historie".to_string();
    }

    capped
        .iter()
        .map(|entry| {
            let prefix = match entry.role {
                Role::User => "User:",
                Role::Partner => "Partner:",
                Role::PartnerSimple => "Partner (Leichte Sprache):",
            };
            format!("{} {}", prefix, sanitize(&entry.message))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
    let history_value = format_translate_history(history);
    let sanitized_input = sanitize(input);

    if examples.is_empty() {
        return TRANSLATE_PROMPT_TEMPLATE
            .replace("%HISTORY%", &history_value)
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

    /// Verifies that `build_correct_transcript_prompt` injects the sanitized input and has no unreplaced placeholders.
    #[test]
    fn test_build_correct_transcript_prompt() {
        let prompt = build_correct_transcript_prompt("hallo ```welt```");
        assert!(prompt.contains("```input\nhallo '''welt'''\n```"));
        assert!(!prompt.contains("%INPUT%"));
        assert!(!prompt.contains("%HISTORY%"));
    }

    /// Verifies `format_translate_history`, `count_history_turns`, `determine_history_bucket`, and `partner_simple` capping.
    #[test]
    fn test_build_translate_prompt_and_partner_simple_history() {
        assert_eq!(format_translate_history(&[]), "keine Historie");
        assert_eq!(determine_history_bucket(&[]), 0);

        let history = vec![
            ConversationMessage {
                role: Role::User,
                message: "Frage 1".to_string(),
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Antwort 1".to_string(),
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "ÜBERSETZUNG\nEinfache Antwort 1".to_string(),
            },
        ];
        // partner_simple does not increment turn count
        assert_eq!(count_history_turns(&history), 2);
        assert_eq!(determine_history_bucket(&history), 2);

        let prompt = build_translate_prompt(&history, "Neue Aussage");
        assert!(prompt.contains(
            "User: Frage 1\n\nPartner: Antwort 1\n\nPartner (Leichte Sprache): ÜBERSETZUNG\nEinfache Antwort 1"
        ));
        assert!(prompt.contains("```input\nNeue Aussage\n```"));
        assert!(!prompt.contains("%HISTORY%"));
        assert!(!prompt.contains("%INPUT%"));
    }

    /// Verifies that `cap_history_turns` retains at most the last 5 `user`/`partner` turns along with their `partner_simple` translations.
    #[test]
    fn test_cap_history_turns_to_last_5() {
        let history = vec![
            ConversationMessage {
                role: Role::User,
                message: "Turn 1 (dropped)".to_string(),
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Turn 2 (dropped)".to_string(),
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "Simple 2 (dropped)".to_string(),
            },
            ConversationMessage {
                role: Role::User,
                message: "Turn 3 (kept 1)".to_string(),
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Turn 4 (kept 2)".to_string(),
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "Simple 4 (kept)".to_string(),
            },
            ConversationMessage {
                role: Role::User,
                message: "Turn 5 (kept 3)".to_string(),
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Turn 6 (kept 4)".to_string(),
            },
            ConversationMessage {
                role: Role::PartnerSimple,
                message: "Simple 6 (kept)".to_string(),
            },
            ConversationMessage {
                role: Role::User,
                message: "Turn 7 (kept 5)".to_string(),
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

    /// Verifies that `build_translate_prompt_with_few_shots` injects `%FEW_SHOTS%` and that `EchoPromptRunner`
    /// still extracts the final `input` block rather than a few-shot's `input` block.
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
        assert!(rendered.contains("```input\nEchte Nutzer Eingabe\n```"));

        let runner = EchoPromptRunner;
        let out = runner
            .run_prompt(TRANSLATE_SYSTEM_PROMPT, &rendered, "medium")
            .await
            .unwrap();
        assert_eq!(out.text, "FORMATIERUNG\nEchte Nutzer Eingabe");
    }

    /// Verifies that `TRANSLATE_SYSTEM_PROMPT` contains the core translation rules without unreplaced placeholders.
    #[test]
    fn test_translate_system_prompt() {
        assert!(TRANSLATE_SYSTEM_PROMPT.contains("# Regeln für Leichte Sprache"));
        assert!(TRANSLATE_SYSTEM_PROMPT.contains("ÜBERSETZUNG oder FORMATIERUNG"));
        assert!(!TRANSLATE_SYSTEM_PROMPT.contains("%translate-few-shots.md%"));
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

    /// Verifies that `EchoPromptRunner` accurately counts Gemma 4 tokens and mocks reasoning thoughts.
    #[tokio::test]
    async fn test_echo_prompt_runner_tokens_and_reasoning() {
        let runner = EchoPromptRunner;
        let prompt = "```input\nHallo Welt\n```";
        let output = runner
            .run_prompt(CORRECT_TRANSCRIPT_SYSTEM_PROMPT, prompt, "low")
            .await
            .unwrap();

        assert_eq!(output.text, "Hallo Welt");
        assert_eq!(
            output.reasoning,
            Some("I am thinking about 'Hallo Welt'".to_string())
        );
        assert!(output.usage.input_tokens > 0);
        assert!(output.usage.reasoning_tokens > 0);
        assert!(output.usage.output_tokens > 0);
    }
}
