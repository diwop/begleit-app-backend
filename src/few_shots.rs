use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::xlm_roberta::{Config as XlmRobertaConfig, XLMRobertaModel};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// Lazily-loaded Gemma 4 tokenizer decompressed from bundled assets.
static GEMMA_TOKENIZER: LazyLock<tokenizers::Tokenizer> = LazyLock::new(|| {
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

/// Sample template used to format each dynamic few-shot demonstration block.
pub const FEW_SHOT_SAMPLE_TEMPLATE: &str =
    include_str!("../prompts/translate-dynamic-few-shots-sample.md");

/// Default directory containing the raw DVC dialog `.txt` files.
pub const DEFAULT_RAW_DIALOGS_DIR: &str = "data/raw_dialogs";

/// Default directory where `intfloat/multilingual-e5-base` model files are stored.
pub const DEFAULT_EMBEDDING_MODEL_DIR: &str = "models/multilingual-e5-base";

/// Default path where the precomputed 6-bucket vector index is persisted.
pub const DEFAULT_FEW_SHOTS_INDEX_PATH: &str = "data/few_shots_index.json.gz";

/// Hugging Face repository ID for the multilingual E5 base embedding model.
pub const HF_MULTILINGUAL_E5_BASE_REPO: &str = "intfloat/multilingual-e5-base";

/// Default number of few-shot demonstrations retrieved per `/translate` request.
pub const DEFAULT_FEW_SHOT_K: usize = 3;

/// Number of history-length buckets (`0..=5`).
pub const NUM_BUCKETS: usize = 6;

/// Embedding dimensionality for `intfloat/multilingual-e5-base`.
pub const EMBEDDING_DIM: usize = 768;

/// Represents a single parsed turn from a raw dialog `.txt` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawDialogTurn {
    /// Spoken contribution from the user (`<user>...</user>`).
    User { text: String },
    /// Spoken contribution from the partner paired with its simple language translation or formatting.
    Partner {
        text: String,
        kind: String,
        translation: String,
    },
}

/// A prepared few-shot example entry belonging to one of the 6 history-length buckets (`0..=5`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FewShotEntry {
    /// Unique identifier (`<dialog_stem>_<exchange_idx:02>`).
    pub id: String,
    /// Source dialog filename.
    pub dialog: String,
    /// Zero-based index of the partner exchange within the dialog.
    pub exchange_idx: usize,
    /// Zero-based index of the turn within all turns of the dialog.
    pub turn_idx: usize,
    /// History-length bucket index (`0..=5`).
    pub bucket: usize,
    /// Output classification (`"ÜBERSETZUNG"` or `"FORMATIERUNG"`).
    pub kind: String,
    /// Raw partner statement in standard German (`Standardsprache`).
    pub input: String,
    /// Target output (`ÜBERSETZUNG\n...` or `FORMATIERUNG\n...`).
    pub output: String,
    /// Formatted history string preceding this turn (`"keine Historie"` when `bucket == 0`).
    pub history: String,
    /// Complete rendered Markdown block ready to be injected into `%FEW_SHOTS%`.
    pub sample: String,
    /// Token count of `sample` measured with the Gemma 4 tokenizer.
    pub tokens: u64,
    /// L2-normalized embedding vector of `"passage: <input>"`.
    pub embedding: Vec<f32>,
}

/// Serializable container holding the precomputed entries across all 6 history-length buckets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedBucketIndex {
    /// Model identifier used to compute the embeddings.
    pub model_name: String,
    /// Entries partitioned into buckets `0..=5`.
    pub buckets: Vec<Vec<FewShotEntry>>,
}

/// Trait abstracting text embedding generation so production uses `intfloat/multilingual-e5-base`
/// via Candle and unit tests can use a fast deterministic embedder when weights are not on disk.
pub trait TextEmbedder: Send + Sync {
    /// Computes an L2-normalized embedding vector for the given input text (e.g. `"query: ..."` or `"passage: ..."`).
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
}

/// Pure-Rust `intfloat/multilingual-e5-base` embedding model powered by Hugging Face `candle` and `tokenizers`.
pub struct CandleE5Embedder {
    model: Mutex<XLMRobertaModel>,
    tokenizer: tokenizers::Tokenizer,
    device: Device,
    max_seq_len: usize,
}

impl CandleE5Embedder {
    /// Loads `intfloat/multilingual-e5-base` from `model_dir` containing `config.json`, `tokenizer.json`, and `model.safetensors`.
    pub fn load(model_dir: &Path) -> Result<Self, String> {
        let config_path = model_dir.join("config.json");
        let tokenizer_path = model_dir.join("tokenizer.json");
        let weights_path = model_dir.join("model.safetensors");

        if !config_path.exists() || !tokenizer_path.exists() || !weights_path.exists() {
            return Err(format!(
                "Missing model files in {} (expected config.json, tokenizer.json, model.safetensors)",
                model_dir.display()
            ));
        }

        let config_bytes = std::fs::read(&config_path)
            .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
        let config: XlmRobertaConfig = serde_json::from_slice(&config_bytes)
            .map_err(|e| format!("Failed to parse {}: {e}", config_path.display()))?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| format!("Failed to load tokenizer {}: {e}", tokenizer_path.display()))?;
        tokenizer.with_padding(None);

        let device = Device::Cpu;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                std::slice::from_ref(&weights_path),
                DType::F32,
                &device,
            )
            .map_err(|e| format!("Failed to mmap {}: {e}", weights_path.display()))?
        };

        let model_vb = if vb.contains_tensor("embeddings.word_embeddings.weight") {
            vb
        } else {
            vb.pp("roberta")
        };

        let max_seq_len = config.max_position_embeddings.saturating_sub(2).max(128);
        let model = XLMRobertaModel::new(&config, model_vb)
            .map_err(|e| format!("Failed to initialize XLMRobertaModel: {e}"))?;

        Ok(Self {
            model: Mutex::new(model),
            tokenizer,
            device,
            max_seq_len,
        })
    }
}

impl TextEmbedder for CandleE5Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("Tokenization failed: {e}"))?;

        let mut ids = encoding.get_ids().to_vec();
        if ids.len() > self.max_seq_len {
            ids.truncate(self.max_seq_len);
        }
        if ids.is_empty() {
            return Ok(vec![0.0; EMBEDDING_DIM]);
        }

        let seq_len = ids.len();
        let input_ids = Tensor::new(&ids[..], &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| format!("Failed to build input_ids tensor: {e}"))?;
        let attention_mask = Tensor::ones((1, seq_len), DType::U32, &self.device)
            .map_err(|e| format!("Failed to build attention_mask tensor: {e}"))?;
        let token_type_ids = Tensor::zeros((1, seq_len), DType::U32, &self.device)
            .map_err(|e| format!("Failed to build token_type_ids tensor: {e}"))?;

        let hidden_states = {
            let model = self
                .model
                .lock()
                .map_err(|e| format!("Model mutex poisoned: {e}"))?;
            model
                .forward(
                    &input_ids,
                    &attention_mask,
                    &token_type_ids,
                    None,
                    None,
                    None,
                )
                .map_err(|e| format!("XLMRoberta forward pass failed: {e}"))?
        };

        let pooled = hidden_states
            .squeeze(0)
            .and_then(|t| t.mean(0))
            .and_then(|t| t.to_vec1::<f32>())
            .map_err(|e| format!("Mean pooling failed: {e}"))?;

        Ok(l2_normalize(&pooled))
    }
}

/// Fast deterministic character/word n-gram feature hashing embedder used as an offline fallback
/// in unit and integration tests when the 1.1 GB `multilingual-e5-base` weights are not present.
#[derive(Debug, Default, Clone)]
pub struct DeterministicHashEmbedder;

impl TextEmbedder for DeterministicHashEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let stripped = text
            .strip_prefix("query: ")
            .or_else(|| text.strip_prefix("passage: "))
            .unwrap_or(text)
            .to_lowercase();

        let mut vec = vec![0.0f32; EMBEDDING_DIM];
        for word in stripped.split(|c: char| !c.is_alphanumeric()) {
            if word.is_empty() {
                continue;
            }
            let h = fnv1a_hash(word.as_bytes());
            let idx = (h as usize) % EMBEDDING_DIM;
            let sign = if (h >> 32) & 1 == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign;

            // Also hash character trigrams for subword similarity on German compounds
            let chars: Vec<char> = word.chars().collect();
            for window in chars.windows(3) {
                let mut buf = [0u8; 12];
                let mut len = 0;
                for ch in window {
                    let mut tmp = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut tmp);
                    buf[len..len + encoded.len()].copy_from_slice(encoded.as_bytes());
                    len += encoded.len();
                }
                let th = fnv1a_hash(&buf[..len]);
                let tidx = (th as usize) % EMBEDDING_DIM;
                let tsign = if (th >> 32) & 1 == 0 { 0.5 } else { -0.5 };
                vec[tidx] += tsign;
            }
        }

        Ok(l2_normalize(&vec))
    }
}

/// Computes a 64-bit FNV-1a hash for deterministic feature hashing.
fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// L2-normalizes a vector in-place (returning a unit-length vector, or zeros if norm is 0).
pub fn l2_normalize(vec: &[f32]) -> Vec<f32> {
    let norm_sq: f32 = vec.iter().map(|x| x * x).sum();
    let norm = norm_sq.sqrt();
    if norm > 1e-12 {
        vec.iter().map(|x| x / norm).collect()
    } else {
        vec.to_vec()
    }
}

/// Computes the cosine similarity (dot product) between two L2-normalized vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// In-memory semantic vector index across all 6 history-length buckets (`0..=5`).
pub struct FewShotBucketIndex {
    buckets: [Vec<FewShotEntry>; NUM_BUCKETS],
    embedder: Arc<dyn TextEmbedder>,
}

impl FewShotBucketIndex {
    /// Creates a new [`FewShotBucketIndex`] from pre-partitioned bucket entries and an embedder.
    pub fn new(buckets: [Vec<FewShotEntry>; NUM_BUCKETS], embedder: Arc<dyn TextEmbedder>) -> Self {
        Self { buckets, embedder }
    }

    /// Builds a [`FewShotBucketIndex`] directly from a directory of raw dialog `.txt` files
    /// using the provided [`TextEmbedder`].
    pub fn build_from_raw_dir(
        raw_dir: &Path,
        embedder: Arc<dyn TextEmbedder>,
    ) -> Result<Self, String> {
        let mut buckets = extract_few_shot_buckets_from_dir(raw_dir)?;
        for bucket_entries in &mut buckets {
            for entry in bucket_entries.iter_mut() {
                let passage = format!("passage: {}", entry.input);
                entry.embedding = embedder.embed(&passage)?;
            }
        }
        Ok(Self { buckets, embedder })
    }

    /// Saves the precomputed bucket entries and embeddings as a gzip-compressed JSON file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
        }
        let payload = SerializedBucketIndex {
            model_name: HF_MULTILINGUAL_E5_BASE_REPO.to_string(),
            buckets: self.buckets.to_vec(),
        };
        let json_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize bucket index: {e}"))?;

        let file = std::fs::File::create(path)
            .map_err(|e| format!("Failed to create {}: {e}", path.display()))?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder
            .write_all(&json_bytes)
            .map_err(|e| format!("Failed to write compressed index: {e}"))?;
        encoder
            .finish()
            .map_err(|e| format!("Failed to finish gzip stream: {e}"))?;
        Ok(())
    }

    /// Loads a precomputed [`FewShotBucketIndex`] from a gzip-compressed JSON file.
    pub fn load_from_file(path: &Path, embedder: Arc<dyn TextEmbedder>) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|e| format!("Failed to open index file {}: {e}", path.display()))?;
        let mut decoder = GzDecoder::new(file);
        let mut json_bytes = Vec::new();
        decoder
            .read_to_end(&mut json_bytes)
            .map_err(|e| format!("Failed to decompress index file {}: {e}", path.display()))?;

        let payload: SerializedBucketIndex = serde_json::from_slice(&json_bytes)
            .map_err(|e| format!("Failed to parse index JSON {}: {e}", path.display()))?;

        let mut buckets: [Vec<FewShotEntry>; NUM_BUCKETS] = Default::default();
        for (i, b) in payload.buckets.into_iter().take(NUM_BUCKETS).enumerate() {
            buckets[i] = b;
        }

        Ok(Self { buckets, embedder })
    }

    /// Loads the production index and `CandleE5Embedder` if available on disk, or falls back
    /// to building an in-memory index from `data/raw_dialogs` or `tests/fixtures/raw_dialogs`.
    pub fn load_default_or_fallback() -> Result<Self, String> {
        let model_dir = PathBuf::from(
            std::env::var("EMBEDDING_MODEL_DIR")
                .unwrap_or_else(|_| DEFAULT_EMBEDDING_MODEL_DIR.to_string()),
        );
        let index_path = PathBuf::from(
            std::env::var("FEW_SHOTS_INDEX_PATH")
                .unwrap_or_else(|_| DEFAULT_FEW_SHOTS_INDEX_PATH.to_string()),
        );

        let embedder: Arc<dyn TextEmbedder> = if model_dir.join("model.safetensors").exists() {
            tracing::info!(
                model_dir = %model_dir.display(),
                "Loading CandleE5Embedder (intfloat/multilingual-e5-base)"
            );
            Arc::new(CandleE5Embedder::load(&model_dir)?)
        } else {
            tracing::info!(
                model_dir = %model_dir.display(),
                "Embedding model weights not found on disk; using DeterministicHashEmbedder fallback"
            );
            Arc::new(DeterministicHashEmbedder)
        };

        if index_path.exists() {
            tracing::info!(
                index_path = %index_path.display(),
                "Loading precomputed few-shot bucket index"
            );
            return Self::load_from_file(&index_path, embedder);
        }

        if let Ok(explicit_raw_dir) = std::env::var("RAW_DIALOGS_DIR") {
            let raw_dir = PathBuf::from(explicit_raw_dir);
            if raw_dir.exists() && has_txt_files(&raw_dir) {
                tracing::info!(
                    raw_dir = %raw_dir.display(),
                    "Building few-shot bucket index from configured RAW_DIALOGS_DIR"
                );
                return Self::build_from_raw_dir(&raw_dir, embedder);
            }
        }

        let fixture_dir = Path::new("tests/fixtures/raw_dialogs");
        if fixture_dir.exists() && has_txt_files(fixture_dir) {
            tracing::info!(
                fixture_dir = %fixture_dir.display(),
                "Building few-shot bucket index from test fixtures directory"
            );
            return Self::build_from_raw_dir(fixture_dir, embedder);
        }

        let raw_dir = Path::new(DEFAULT_RAW_DIALOGS_DIR);
        if raw_dir.exists() && has_txt_files(raw_dir) {
            tracing::info!(
                raw_dir = %raw_dir.display(),
                "Building few-shot bucket index from raw dialogs directory"
            );
            return Self::build_from_raw_dir(raw_dir, embedder);
        }

        Ok(Self::new(Default::default(), embedder))
    }

    /// Returns a process-wide cached `Arc<FewShotBucketIndex>` initialized via [`Self::load_default_or_fallback`].
    pub fn default_shared() -> Result<Arc<Self>, String> {
        static SHARED_INDEX: LazyLock<Result<Arc<FewShotBucketIndex>, String>> =
            LazyLock::new(|| FewShotBucketIndex::load_default_or_fallback().map(Arc::new));
        SHARED_INDEX.clone()
    }

    /// Returns the total number of indexed partner turns across all 6 buckets.
    pub fn total_entries(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    /// Returns the entries stored in a specific bucket (`0..=5`).
    pub fn bucket_entries(&self, bucket: usize) -> &[FewShotEntry] {
        &self.buckets[bucket.min(NUM_BUCKETS - 1)]
    }

    /// Retrieves the top `k` semantically closest few-shot examples from `bucket` (`0..=5`) for `query`.
    pub fn get_closest_examples(
        &self,
        query: &str,
        bucket: usize,
        k: usize,
    ) -> Result<Vec<FewShotEntry>, String> {
        let trimmed = query.trim();
        let clamped_bucket = bucket.min(NUM_BUCKETS - 1);
        let entries = &self.buckets[clamped_bucket];
        if trimmed.is_empty() || entries.is_empty() || k == 0 {
            return Ok(Vec::new());
        }

        let formatted_query = format!("query: {trimmed}");
        let query_vec = self.embedder.embed(&formatted_query)?;

        let mut scored: Vec<(f32, usize)> = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| (cosine_similarity(&query_vec, &entry.embedding), idx))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scored
            .into_iter()
            .take(k)
            .map(|(_, idx)| entries[idx].clone())
            .collect())
    }
}

/// Checks whether a directory contains at least one `.txt` dialog file.
fn has_txt_files(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "txt")
        })
}

/// Parses a raw dialog `.txt` file content into ordered [`RawDialogTurn`] exchanges.
///
/// Matches tags `<user>...</user>`, `<partner>...</partner>`, `<partner simple translation>...</partner>`,
/// and `<partner simple formatting>...</partner>`, pairing each `<partner>` tag with its subsequent
/// simple translation or formatting tag exactly like `prepare_few_shots_dialogs.py`.
pub fn parse_dialog_content(content: &str) -> Vec<RawDialogTurn> {
    let raw_tags = extract_raw_tags(content);
    let mut turns = Vec::new();
    let mut i = 0;

    while i < raw_tags.len() {
        let (tag, text) = &raw_tags[i];
        if tag == "user" {
            turns.push(RawDialogTurn::User {
                text: text.trim().to_string(),
            });
            i += 1;
        } else if tag == "partner" {
            if i + 1 < raw_tags.len()
                && (raw_tags[i + 1].0 == "partner simple translation"
                    || raw_tags[i + 1].0 == "partner simple formatting")
            {
                let (simple_tag, simple_text) = &raw_tags[i + 1];
                let kind = if simple_tag == "partner simple translation" {
                    "ÜBERSETZUNG"
                } else {
                    "FORMATIERUNG"
                };
                turns.push(RawDialogTurn::Partner {
                    text: text.trim().to_string(),
                    kind: kind.to_string(),
                    translation: simple_text.trim().to_string(),
                });
                i += 2;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    turns
}

/// Scans raw dialog text for `<user>`, `<partner>`, `<partner simple translation>`, and `<partner simple formatting>` tags.
fn extract_raw_tags(content: &str) -> Vec<(String, String)> {
    const ALLOWED_OPEN_TAGS: &[&str] = &[
        "user",
        "partner",
        "partner simple translation",
        "partner simple formatting",
    ];

    let mut results = Vec::new();
    let mut rest = content;

    while let Some(open_start) = rest.find('<') {
        let after_lt = &rest[open_start + 1..];
        let Some(open_end) = after_lt.find('>') else {
            break;
        };
        let tag_name = &after_lt[..open_end];
        let after_open = &after_lt[open_end + 1..];

        if ALLOWED_OPEN_TAGS.contains(&tag_name) {
            // Find the earliest closing </user> or </partner>
            let close_user = after_open.find("</user>");
            let close_partner = after_open.find("</partner>");
            let close_match = match (close_user, close_partner) {
                (Some(u), Some(p)) => {
                    if u < p {
                        Some((u, "</user>".len()))
                    } else {
                        Some((p, "</partner>".len()))
                    }
                }
                (Some(u), None) => Some((u, "</user>".len())),
                (None, Some(p)) => Some((p, "</partner>".len())),
                (None, None) => None,
            };

            if let Some((close_idx, close_len)) = close_match {
                let inner_text = &after_open[..close_idx];
                results.push((tag_name.to_string(), inner_text.to_string()));
                rest = &after_open[close_idx + close_len..];
                continue;
            }
        }

        rest = after_open;
    }

    results
}

/// Formats preceding [`RawDialogTurn`]s into the history string structure expected by the prompt templates.
pub fn format_raw_turns_history(turns_before: &[RawDialogTurn]) -> String {
    if turns_before.is_empty() {
        return "keine Historie".to_string();
    }

    let mut blocks = Vec::new();
    for turn in turns_before {
        match turn {
            RawDialogTurn::User { text } => {
                blocks.push(format!("User: {text}"));
            }
            RawDialogTurn::Partner {
                text,
                kind,
                translation,
            } => {
                blocks.push(format!("Partner: {text}"));
                blocks.push(format!("Partner (Leichte Sprache): {kind}\n{translation}"));
            }
        }
    }

    if blocks.is_empty() {
        "keine Historie".to_string()
    } else {
        blocks.join("\n\n")
    }
}

/// Extracts all partner turns from a directory of raw dialog `.txt` files into 6 history-length buckets (`0..=5`).
///
/// Emulates `src-eval/prepare_few_shots_dialogs.py` on the full dialog set:
/// - Bucket 0: `history_len == 0` (`"keine Historie"`)
/// - Bucket 1..=4: `history_len == N`
/// - Bucket 5: `history_len >= 5` (history capped to the last 5 turns)
pub fn extract_few_shot_buckets_from_dir(
    raw_dir: &Path,
) -> Result<[Vec<FewShotEntry>; NUM_BUCKETS], String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(raw_dir)
        .map_err(|e| format!("Failed to read directory {}: {e}", raw_dir.display()))?
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("txt")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|name| !name.starts_with('.'))
        })
        .collect();

    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let sample_template = FEW_SHOT_SAMPLE_TEMPLATE.trim();
    let mut buckets: [Vec<FewShotEntry>; NUM_BUCKETS] = Default::default();

    for filepath in files {
        let content = std::fs::read_to_string(&filepath)
            .map_err(|e| format!("Failed to read {}: {e}", filepath.display()))?;
        let turns = parse_dialog_content(&content);
        let file_name = filepath
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let doc_stem = filepath
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();

        let mut partner_count = 0usize;
        for (idx, turn) in turns.iter().enumerate() {
            if let RawDialogTurn::Partner {
                text,
                kind,
                translation,
            } = turn
            {
                let history_len = idx;
                let bucket_idx = history_len.min(NUM_BUCKETS - 1);

                let turns_for_history = if bucket_idx == 5 && idx > 5 {
                    &turns[idx - 5..idx]
                } else {
                    &turns[..idx]
                };

                let history_str = format_raw_turns_history(turns_for_history);
                let few_shot_output = format!("{kind}\n{translation}");
                let sample_text = sample_template
                    .replace("%FEW_SHOT_HISTORY%", &history_str)
                    .replace("%FEW_SHOT_INPUT%", text)
                    .replace("%FEW_SHOT_OUTPUT%", &few_shot_output);
                let tok_count = if cfg!(debug_assertions) {
                    sample_text.split_whitespace().count().max(1) as u64
                } else {
                    count_gemma_tokens(&sample_text)
                };
                let sample_id = format!("{doc_stem}_{partner_count:02}");

                buckets[bucket_idx].push(FewShotEntry {
                    id: sample_id,
                    dialog: file_name.clone(),
                    exchange_idx: partner_count,
                    turn_idx: idx,
                    bucket: bucket_idx,
                    kind: kind.clone(),
                    input: text.clone(),
                    output: few_shot_output,
                    history: history_str,
                    sample: sample_text,
                    tokens: tok_count,
                    embedding: Vec::new(),
                });
                partner_count += 1;
            }
        }
    }

    Ok(buckets)
}

/// Ensures `config.json`, `tokenizer.json`, and `model.safetensors` for `intfloat/multilingual-e5-base`
/// are present in `model_dir`, downloading any missing files from Hugging Face Hub.
pub async fn ensure_e5_model_files(model_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(model_dir)
        .map_err(|e| format!("Failed to create model dir {}: {e}", model_dir.display()))?;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    for filename in ["config.json", "tokenizer.json", "model.safetensors"] {
        let dest = model_dir.join(filename);
        if dest.exists() {
            tracing::info!(file = %dest.display(), "Model file already present");
            continue;
        }

        let url = format!(
            "https://huggingface.co/{HF_MULTILINGUAL_E5_BASE_REPO}/resolve/main/{filename}"
        );
        tracing::info!(url = %url, dest = %dest.display(), "Downloading model file from Hugging Face Hub...");

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to download {url}: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Hugging Face download failed for {url}: HTTP {}",
                resp.status()
            ));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response bytes for {url}: {e}"))?;

        std::fs::write(&dest, &bytes)
            .map_err(|e| format!("Failed to write {}: {e}", dest.display()))?;
        tracing::info!(
            file = %dest.display(),
            bytes = bytes.len(),
            "Successfully downloaded model file"
        );
    }

    Ok(())
}

/// Executes the build-time preparation workflow (`diwop-begleitapp --prepare-few-shots`):
///
/// 1. Ensures `intfloat/multilingual-e5-base` weights are downloaded in `EMBEDDING_MODEL_DIR`.
/// 2. Parses all raw dialog `.txt` files from `RAW_DIALOGS_DIR` into the 6 history-length buckets (`0..=5`).
/// 3. Computes L2-normalized E5 embeddings for all passages and saves the compressed index to `FEW_SHOTS_INDEX_PATH`.
pub async fn run_prepare_few_shots_cli() -> Result<(), String> {
    let raw_dir = PathBuf::from(
        std::env::var("RAW_DIALOGS_DIR").unwrap_or_else(|_| DEFAULT_RAW_DIALOGS_DIR.to_string()),
    );
    let model_dir = PathBuf::from(
        std::env::var("EMBEDDING_MODEL_DIR")
            .unwrap_or_else(|_| DEFAULT_EMBEDDING_MODEL_DIR.to_string()),
    );
    let output_path = PathBuf::from(
        std::env::var("FEW_SHOTS_INDEX_PATH")
            .unwrap_or_else(|_| DEFAULT_FEW_SHOTS_INDEX_PATH.to_string()),
    );

    tracing::info!(
        raw_dir = %raw_dir.display(),
        model_dir = %model_dir.display(),
        output_path = %output_path.display(),
        "Starting build-time few-shot dialog preparation and embedding indexing"
    );

    ensure_e5_model_files(&model_dir).await?;
    let embedder = Arc::new(CandleE5Embedder::load(&model_dir)?);
    let index = FewShotBucketIndex::build_from_raw_dir(&raw_dir, embedder)?;

    for b in 0..NUM_BUCKETS {
        tracing::info!(
            bucket = b,
            entries = index.bucket_entries(b).len(),
            "Prepared few-shot bucket"
        );
    }
    tracing::info!(
        total_entries = index.total_entries(),
        "Saving precomputed few-shot bucket index"
    );

    index.save_to_file(&output_path)?;
    tracing::info!(
        output_path = %output_path.display(),
        "Successfully completed --prepare-few-shots indexing"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `parse_dialog_content` extracts `<user>` and `<partner>` turns with both
    /// `<partner simple translation>` and `<partner simple formatting>`.
    #[test]
    fn test_parse_dialog_content() {
        let raw = r#"
<user>
Hallo Frau Doktor.
</user>
<partner>
Guten Tag! Nehmen Sie Platz.
</partner>
<partner simple translation>
Guten Tag.
Setzen Sie sich bitte.
</partner>
<user>
Danke.
</user>
<partner>
Wie geht es Ihnen?
</partner>
<partner simple formatting>
Wie geht es Ihnen?
</partner>
"#;
        let turns = parse_dialog_content(raw);
        assert_eq!(turns.len(), 4);
        assert_eq!(
            turns[0],
            RawDialogTurn::User {
                text: "Hallo Frau Doktor.".to_string()
            }
        );
        assert_eq!(
            turns[1],
            RawDialogTurn::Partner {
                text: "Guten Tag! Nehmen Sie Platz.".to_string(),
                kind: "ÜBERSETZUNG".to_string(),
                translation: "Guten Tag.\nSetzen Sie sich bitte.".to_string(),
            }
        );
        assert_eq!(
            turns[3],
            RawDialogTurn::Partner {
                text: "Wie geht es Ihnen?".to_string(),
                kind: "FORMATIERUNG".to_string(),
                translation: "Wie geht es Ihnen?".to_string(),
            }
        );
    }

    /// Verifies that `extract_few_shot_buckets_from_dir` populates all 6 buckets (`0..=5`) from the 10 test fixture dialogs
    /// and preserves `Partner (Leichte Sprache):` in `%FEW_SHOT_HISTORY%`.
    #[test]
    fn test_extract_buckets_from_10_fixture_dialogs() {
        let fixture_dir = Path::new("tests/fixtures/raw_dialogs");
        let buckets = extract_few_shot_buckets_from_dir(fixture_dir).unwrap();

        for (bucket_idx, entries) in buckets.iter().enumerate() {
            assert!(
                !entries.is_empty(),
                "Bucket {bucket_idx} should be non-empty across the 10 fixture dialogs"
            );
        }

        // Bucket 0 must have "keine Historie"
        for entry in &buckets[0] {
            assert_eq!(entry.history, "keine Historie");
            assert!(entry.sample.contains("```history\nkeine Historie\n```"));
        }

        // Bucket 2+ must include Partner (Leichte Sprache): in history
        let b2_entry = &buckets[2][0];
        assert!(
            b2_entry.history.contains("Partner (Leichte Sprache):"),
            "History for bucket >= 2 should contain 'Partner (Leichte Sprache):'"
        );
        assert!(b2_entry.tokens > 0);
    }

    /// Verifies building a `FewShotBucketIndex` from the 10 fixture dialogs, saving/loading it,
    /// and querying the most similar examples per bucket.
    #[test]
    fn test_bucket_index_lookup_and_roundtrip() {
        let fixture_dir = Path::new("tests/fixtures/raw_dialogs");
        let embedder = Arc::new(DeterministicHashEmbedder);
        let index = FewShotBucketIndex::build_from_raw_dir(fixture_dir, embedder.clone()).unwrap();

        assert!(index.total_entries() >= 50);

        // Query a statement from bucket 1 ("Guten Tag! Schön, dass Sie da sind. Nehmen Sie gerne Platz.")
        let target_query = "Guten Tag! Schön, dass Sie da sind. Nehmen Sie gerne Platz.";
        let top_k = index.get_closest_examples(target_query, 1, 3).unwrap();
        assert_eq!(top_k.len(), 3);
        assert!(
            top_k[0].input.contains("Nehmen Sie gerne Platz"),
            "Top match should be the semantically matching greeting exchange, got: {}",
            top_k[0].input
        );

        // Verify serialization round-trip
        let tmp_path = std::env::temp_dir().join("test_few_shots_index.json.gz");
        index.save_to_file(&tmp_path).unwrap();
        let loaded = FewShotBucketIndex::load_from_file(&tmp_path, embedder).unwrap();
        assert_eq!(loaded.total_entries(), index.total_entries());
        let _ = std::fs::remove_file(&tmp_path);
    }
}
