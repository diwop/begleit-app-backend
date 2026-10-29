use crate::api::{ConversationMessage, Role, TranslationType, Warning, WarningType};
use rig::client::CompletionClient;
use rig::completion::CompletionRequestBuilder;
use rig::providers::gemini;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

/// System prompt for `/correct-transcript`.
pub const CORRECT_TRANSCRIPT_SYSTEM_PROMPT: &str =
    include_str!("../prompts/correct-transcript-system.md");

/// User prompt template for `/correct-transcript`.
pub const CORRECT_TRANSCRIPT_PROMPT_TEMPLATE: &str =
    include_str!("../prompts/correct-transcript.md");

/// Raw system prompt template for `/translate` containing the `%translate-few-shots.md%` placeholder.
const TRANSLATE_SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../prompts/translate-system.md");

/// Few-shot examples injected into the `/translate` system prompt.
const TRANSLATE_FEW_SHOTS: &str = include_str!("../prompts/translate-few-shots.md");

/// Resolved system prompt for `/translate` with few-shot examples embedded.
pub static TRANSLATE_SYSTEM_PROMPT: LazyLock<String> = LazyLock::new(|| {
    TRANSLATE_SYSTEM_PROMPT_TEMPLATE.replace("%translate-few-shots.md%", TRANSLATE_FEW_SHOTS)
});

/// User prompt template for `/translate`.
pub const TRANSLATE_PROMPT_TEMPLATE: &str = include_str!("../prompts/translate.md");

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
    /// Executes a prompt with the given system preamble and thinking level (`"minimal"` or `"low"`).
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

/// Lazily-loaded Gemma 4 tokenizer decompressed from bundled assets.
static GEMMA_TOKENIZER: LazyLock<tokenizers::Tokenizer> = LazyLock::new(|| {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let compressed = include_bytes!("../assets/gemma-4-tokenizer.json.gz");
    let mut decoder = GzDecoder::new(&compressed[..]);
    let mut json_bytes = Vec::new();
    decoder
        .read_to_end(&mut json_bytes)
        .expect("Failed to decompress gemma-4 tokenizer");
    tokenizers::Tokenizer::from_bytes(&json_bytes).expect("Failed to parse gemma-4 tokenizer")
});

/// Counts the number of tokens in the given text using the Gemma 4 tokenizer.
pub fn count_gemma_tokens(text: &str) -> u64 {
    GEMMA_TOKENIZER
        .encode(text, true)
        .map(|encoding| encoding.get_ids().len() as u64)
        .unwrap_or(0)
}

/// Fallback [`PromptRunner`] used in local debug/test builds when `GEMINI_API_KEY` is not set.
/// Extracts the ```input block from the prompt and echoes it back deterministically,
/// tokenized using the Gemma 4 tokenizer.
#[derive(Debug, Default, Clone)]
pub struct EchoPromptRunner;

impl PromptRunner for EchoPromptRunner {
    fn run_prompt<'a>(
        &'a self,
        system_prompt: &'a str,
        prompt: &'a str,
        thinking_level: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PromptOutput, PromptError>> + Send + 'a>> {
        Box::pin(async move {
            let input = extract_input_block(prompt).unwrap_or(prompt);
            let mocked_reasoning = format!("I am thinking about '{input}'");
            let output_text = if thinking_level == "minimal" {
                input.to_string()
            } else {
                format!("FORMATIERUNG\n{input}")
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

/// Extracts the text inside the ` ```input ... ``` ` fence of a rendered prompt template.
fn extract_input_block(prompt: &str) -> Option<&str> {
    let marker = "```input\n";
    let start = prompt.find(marker)? + marker.len();
    let rest = &prompt[start..];
    let end = rest.rfind("\n```")?;
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

/// Formats the prior conversation messages into Markdown blocks labeled `User:` or `Partner:`.
pub fn format_translate_history(history: &[ConversationMessage]) -> String {
    history
        .iter()
        .map(|entry| {
            let headline = match entry.role {
                Role::User => "User:",
                Role::Partner => "Partner:",
            };
            format!("{}\n```\n{}\n```", headline, sanitize(&entry.message))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Renders the user prompt for `/translate` by injecting the formatted `history` and sanitized `input`.
pub fn build_translate_prompt(history: &[ConversationMessage], input: &str) -> String {
    let history_value = format_translate_history(history);
    TRANSLATE_PROMPT_TEMPLATE
        .replace("%HISTORY%", &history_value)
        .replace("%INPUT%", &sanitize(input))
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

    /// Verifies that `format_translate_history` and `build_translate_prompt` format `User:` and `Partner:` blocks properly.
    #[test]
    fn test_build_translate_prompt() {
        let history = vec![
            ConversationMessage {
                role: Role::User,
                message: "Frage 1".to_string(),
            },
            ConversationMessage {
                role: Role::Partner,
                message: "Antwort 1".to_string(),
            },
        ];
        let prompt = build_translate_prompt(&history, "Neue Aussage");
        assert!(prompt.contains("User:\n```\nFrage 1\n```\n\nPartner:\n```\nAntwort 1\n```"));
        assert!(prompt.contains("Partner:\n```input\nNeue Aussage\n```"));
        assert!(!prompt.contains("%HISTORY%"));
        assert!(!prompt.contains("%INPUT%"));
    }

    /// Verifies that `TRANSLATE_SYSTEM_PROMPT` embeds the few-shot examples without leftover placeholders.
    #[test]
    fn test_translate_system_prompt_includes_few_shots() {
        let sys = &*TRANSLATE_SYSTEM_PROMPT;
        assert!(sys.contains("## Eingabe 1"));
        assert!(sys.contains("ÜBERSETZUNG"));
        assert!(sys.contains("FORMATIERUNG"));
        assert!(!sys.contains("%translate-few-shots.md%"));
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
            .run_prompt("System Prompt", prompt, "minimal")
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
