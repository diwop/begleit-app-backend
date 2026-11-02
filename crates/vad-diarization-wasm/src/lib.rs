pub mod clustering;
pub mod fbank;
pub mod ring_buffer;
pub mod segmentation;
pub mod vad;

use clustering::{
    compute_centroid, AhcClusterer, ClusterRelabel, VoiceProfile,
    DEFAULT_CLUSTER_SIMILARITY_THRESHOLD,
};
use fbank::{FbankExtractor, FbankFeatures};
use ring_buffer::{AudioRingBuffer, SAMPLE_RATE, VAD_WINDOW_SAMPLES};
use segmentation::PyannoteSegmenter;
use serde::{Deserialize, Serialize};
use vad::{SpeechSegment, VadConfig, VadStateMachine, SILERO_STATE_LEN};

/// Default static ring buffer capacity (`4 seconds` @ 16 kHz = `64,000` samples = `256 KB`).
const DEFAULT_RING_BUFFER_CAPACITY: usize = SAMPLE_RATE * 4;

/// Onboarding calibration window length in samples (`1.5 seconds` @ 16 kHz = `24,000` samples).
const ONBOARDING_WINDOW_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 2;

/// Onboarding calibration window step / hop in samples (`0.5 seconds` @ 16 kHz = `8,000` samples).
const ONBOARDING_HOP_SAMPLES: usize = SAMPLE_RATE / 2;

/// Maximum tail span in samples (`2.5 seconds` @ 16 kHz = `40,000` samples) extracted on onboarding flush
/// so the voice profile centroid incorporates a multi-second prosodic span alongside 1.5s windows.
const ONBOARDING_FLUSH_SPAN_SAMPLES: usize = SAMPLE_RATE * 2 + SAMPLE_RATE / 2;

/// JSON payload emitted to the JavaScript main thread for each diarized speech segment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiarizedSegment {
    /// Segment start timestamp in seconds since session start.
    pub start_time: f64,
    /// Segment end timestamp in seconds since session start.
    pub end_time: f64,
    /// Stable acoustic cluster identifier (e.g., `"cluster_0"`, `"cluster_1"`).
    pub cluster_id: String,
    /// Assigned speaker cluster identifier (e.g., `"NutzerIn"`, `"Speaker 1"`, `"Speaker 2"`).
    pub speaker_id: String,
    /// `true` when emitted at an early mid-utterance checkpoint while speech is still ongoing.
    pub is_checkpoint: bool,
    /// Cosine similarity to the `"NutzerIn"` profile centroid, if a profile is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_similarity: Option<f32>,
    /// Calibrated user posterior likelihood in `[0.0, 1.0]`, if a profile is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_likelihood: Option<f32>,
    /// Optional retroactive cluster label updates when a new segment shifts an earlier cluster's role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_relabels: Option<Vec<ClusterRelabel>>,
}

/// Core hardware-independent state holding the ring buffer, VAD state machine, Pyannote segmenter,
/// Fbank extractor, and AHC clusterer.
pub struct DiarizationPipeline {
    ring_buffer: AudioRingBuffer,
    vad: VadStateMachine,
    segmenter: PyannoteSegmenter,
    fbank: FbankExtractor,
    clusterer: AhcClusterer,
    first_segment_after_reset: bool,
    onboarding_profile_id: Option<String>,
    onboarding_speech_samples: Vec<f32>,
    onboarding_next_window_offset: usize,
    onboarding_embeddings: Vec<Vec<f32>>,
}

impl DiarizationPipeline {
    /// Creates a new [`DiarizationPipeline`] with pre-allocated static audio and feature buffers.
    pub fn new() -> Self {
        Self {
            ring_buffer: AudioRingBuffer::with_capacity(DEFAULT_RING_BUFFER_CAPACITY),
            vad: VadStateMachine::new(VadConfig::default()),
            segmenter: PyannoteSegmenter::new(),
            fbank: FbankExtractor::new(),
            clusterer: AhcClusterer::new(DEFAULT_CLUSTER_SIMILARITY_THRESHOLD),
            first_segment_after_reset: true,
            onboarding_profile_id: None,
            onboarding_speech_samples: Vec::new(),
            onboarding_next_window_offset: 0,
            onboarding_embeddings: Vec::new(),
        }
    }

    /// Resets all ring buffers, VAD LSTM states, and session speaker clusters for a new recording.
    pub fn reset(&mut self) {
        self.ring_buffer.reset();
        self.vad.reset();
        self.clusterer.reset();
        self.first_segment_after_reset = true;
        self.onboarding_profile_id = None;
        self.onboarding_speech_samples.clear();
        self.onboarding_next_window_offset = 0;
        self.onboarding_embeddings.clear();
    }

    /// Starts a voice profile onboarding session for `profile_id`.
    pub fn start_onboarding(&mut self, profile_id: &str) {
        self.ring_buffer.reset();
        self.vad.reset();
        self.onboarding_profile_id = Some(profile_id.to_string());
        self.onboarding_speech_samples.clear();
        self.onboarding_next_window_offset = 0;
        self.onboarding_embeddings.clear();
    }

    /// Returns `true` if the pipeline is currently in voice onboarding mode.
    pub fn is_onboarding(&self) -> bool {
        self.onboarding_profile_id.is_some()
    }

    /// Returns total active speech recorded during onboarding in seconds.
    pub fn onboarding_speech_seconds(&self) -> f32 {
        self.onboarding_speech_samples.len() as f32 / SAMPLE_RATE as f32
    }

    /// Returns the number of embedding vectors extracted so far during onboarding.
    pub fn onboarding_embedding_count(&self) -> usize {
        self.onboarding_embeddings.len()
    }

    /// Completes a VAD step during onboarding. If the frame contains active speech,
    /// samples are accumulated into the onboarding buffer and 1-second overlapping windows
    /// (0.5s hop) are converted into `FbankFeatures`.
    pub fn complete_onboarding_vad_step(
        &mut self,
        frame: ring_buffer::AudioFrame,
        probability: f32,
        next_state: &[f32],
    ) -> Vec<FbankFeatures> {
        self.vad.update_lstm_state(next_state);
        let _ = self
            .vad
            .accept_frame_probability(frame.clone(), probability);

        let mut extracted = Vec::new();
        if self.vad.is_in_speech() || probability >= self.vad.config().speech_threshold {
            self.onboarding_speech_samples
                .extend_from_slice(&frame.samples);

            while self.onboarding_speech_samples.len()
                >= self.onboarding_next_window_offset + ONBOARDING_WINDOW_SAMPLES
            {
                let window = &self.onboarding_speech_samples[self.onboarding_next_window_offset
                    ..self.onboarding_next_window_offset + ONBOARDING_WINDOW_SAMPLES];
                if let Some(feat) = self.fbank.extract(window) {
                    extracted.push(feat);
                }
                self.onboarding_next_window_offset += ONBOARDING_HOP_SAMPLES;
            }
        }
        extracted
    }

    /// Flushes any trailing speech samples upon ending onboarding.
    pub fn flush_onboarding_tail(&mut self) -> Option<FbankFeatures> {
        let total = self.onboarding_speech_samples.len();
        if total < 6_400 {
            return None;
        }

        if self.onboarding_embeddings.is_empty() {
            let slice = &self.onboarding_speech_samples[..total.min(ONBOARDING_FLUSH_SPAN_SAMPLES)];
            self.onboarding_next_window_offset = total;
            return self.fbank.extract(slice);
        }

        let start = total.saturating_sub(ONBOARDING_FLUSH_SPAN_SAMPLES);
        let slice = &self.onboarding_speech_samples[start..total];
        self.onboarding_next_window_offset = total;
        self.fbank.extract(slice)
    }

    /// Appends an L2-normalized speaker embedding vector to the onboarding collection.
    pub fn record_onboarding_embedding(&mut self, embedding: &[f32]) {
        if !embedding.is_empty() {
            self.onboarding_embeddings
                .push(clustering::l2_normalize(embedding));
        }
    }

    /// Concludes the voice profile onboarding session and computes the L2-normalized centroid.
    pub fn finish_onboarding(&mut self) -> Result<VoiceProfile, String> {
        let profile_id = self
            .onboarding_profile_id
            .take()
            .ok_or_else(|| "Not currently in onboarding mode".to_string())?;

        if self.onboarding_embeddings.is_empty() {
            self.onboarding_speech_samples.clear();
            self.onboarding_next_window_offset = 0;
            return Err("No speech embeddings captured for voice profile".to_string());
        }

        let centroid = compute_centroid(&self.onboarding_embeddings);
        let profile = VoiceProfile {
            id: profile_id.clone(),
            centroid: centroid.clone(),
            embedding_count: self.onboarding_embeddings.len(),
            speech_seconds: self.onboarding_speech_seconds(),
            model_id: None,
        };

        self.clusterer.upsert_known_profile(profile_id, centroid);
        self.ring_buffer.reset();
        self.vad.reset();
        self.onboarding_speech_samples.clear();
        self.onboarding_next_window_offset = 0;
        self.onboarding_embeddings.clear();

        Ok(profile)
    }

    /// Loads known voice profiles from a JSON string into the AHC clusterer.
    pub fn load_profiles_json(&mut self, profiles_json: &str) -> Result<usize, String> {
        self.clusterer.load_profiles_json(profiles_json)
    }

    /// Sets the cosine similarity threshold for recognizing known voice profiles.
    pub fn set_recognition_threshold(&mut self, threshold: f32) {
        self.clusterer.set_recognition_threshold(threshold);
    }

    /// Returns whether the VAD state machine currently detects active speech.
    pub fn is_speaking(&self) -> bool {
        self.vad.is_in_speech()
    }

    /// Returns the most recent Silero VAD speech probability (`0.0..=1.0`).
    pub fn last_vad_probability(&self) -> f32 {
        self.vad.last_probability()
    }

    /// Serializes the active speaker clusters into a JSON array string.
    pub fn current_clusters_json(&self) -> String {
        serde_json::to_string(self.clusterer.current_clusters())
            .unwrap_or_else(|_| "[]".to_string())
    }

    /// Pushes raw 16 kHz mono PCM samples into the static ring buffer.
    pub fn push_pcm(&mut self, pcm_data: &[f32]) {
        self.ring_buffer.push_slice(pcm_data);
    }

    /// Pops the next 512-sample frame if available and prepares the `[1, 576]` Silero VAD window
    /// and `[2, 1, 128]` recurrent state.
    pub fn next_vad_step(
        &mut self,
    ) -> Option<(
        ring_buffer::AudioFrame,
        [f32; VAD_WINDOW_SAMPLES],
        [f32; SILERO_STATE_LEN],
    )> {
        let frame = self.ring_buffer.pop_frame()?;
        let (win_ref, state_ref) = self.vad.prepare_silero_input(&frame);
        let win = *win_ref;
        let state = *state_ref;
        Some((frame, win, state))
    }

    /// Completes a VAD step with the model's `(probability, next_state)` output, returning a
    /// [`SpeechSegment`] together with its extracted [`FbankFeatures`] if a segment is ready.
    pub fn complete_vad_step(
        &mut self,
        frame: ring_buffer::AudioFrame,
        probability: f32,
        next_state: &[f32],
    ) -> Option<(SpeechSegment, FbankFeatures)> {
        let segment = self.complete_vad_step_raw(frame, probability, next_state)?;
        let features = self.extract_fbank_for_segment(&segment)?;
        Some((segment, features))
    }

    /// Completes a VAD step and returns the raw [`SpeechSegment`] before feature extraction,
    /// allowing `pyannote/segmentation-3.0` to split zero-gap speaker switches and mask overlap first.
    pub fn complete_vad_step_raw(
        &mut self,
        frame: ring_buffer::AudioFrame,
        probability: f32,
        next_state: &[f32],
    ) -> Option<SpeechSegment> {
        self.vad.update_lstm_state(next_state);
        self.vad.accept_frame_probability(frame, probability)
    }

    /// Flushes any trailing active speech at the end of a recording session into [`FbankFeatures`].
    pub fn flush_pending_segment(&mut self) -> Option<(SpeechSegment, FbankFeatures)> {
        let segment = self.flush_pending_segment_raw()?;
        let features = self.extract_fbank_for_segment(&segment)?;
        Some((segment, features))
    }

    /// Flushes any trailing active speech segment without immediately extracting Fbank features.
    pub fn flush_pending_segment_raw(&mut self) -> Option<SpeechSegment> {
        self.vad.flush()
    }

    /// Extracts [`FbankFeatures`] for `segment`, applying the `150 ms` WebRTC AGC cold-start
    /// warm-up guard on the first speech segment after session start or reset.
    pub fn extract_fbank_for_segment(&mut self, segment: &SpeechSegment) -> Option<FbankFeatures> {
        let apply_warmup_guard = self.first_segment_after_reset && segment.start_time <= 2.5;
        let features = self
            .fbank
            .extract_with_warmup_guard(&segment.samples, apply_warmup_guard)?;
        if !segment.is_checkpoint {
            self.first_segment_after_reset = false;
        }
        Some(features)
    }

    /// Refines `segment` using `pyannote/segmentation-3.0` powerset logits (`[num_frames, 7]`)
    /// to split zero-gap speaker switches and mask overlapping speech frames, then extracts
    /// [`FbankFeatures`] for each resulting sub-turn.
    pub fn refine_segment_with_pyannote(
        &mut self,
        segment: SpeechSegment,
        powerset_flat: &[f32],
        num_frames: usize,
    ) -> Vec<(SpeechSegment, FbankFeatures)> {
        let sub_segments = self
            .segmenter
            .refine_segment(&segment, powerset_flat, num_frames);
        let mut results = Vec::with_capacity(sub_segments.len());
        for sub in sub_segments {
            if let Some(fbank) = self.extract_fbank_for_segment(&sub) {
                results.push((sub, fbank));
            }
        }
        results
    }

    /// Assigns a speaker cluster to `segment` given its extracted speaker `embedding`,
    /// computing user similarity, calibrated competitive user likelihood, and any retroactive
    /// [`ClusterRelabel`] updates.
    pub fn diarize_segment(
        &mut self,
        segment: &SpeechSegment,
        embedding: &[f32],
    ) -> DiarizedSegment {
        let assignment = self.clusterer.assign_speaker_with_scores(
            embedding,
            segment.start_time,
            segment.end_time,
        );
        DiarizedSegment {
            start_time: segment.start_time,
            end_time: segment.end_time,
            cluster_id: assignment.cluster_id,
            speaker_id: assignment.speaker_id,
            is_checkpoint: segment.is_checkpoint,
            user_similarity: assignment.user_similarity,
            user_likelihood: assignment.user_likelihood,
            cluster_relabels: if assignment.cluster_relabels.is_empty() {
                None
            } else {
                Some(assignment.cluster_relabels)
            },
        }
    }
}

impl Default for DiarizationPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// WebAssembly Bindings (`wasm32-unknown-unknown`)
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod wasm_api {
    use super::*;
    use crate::segmentation::PYANNOTE_NUM_CLASSES;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Initializes ONNX Runtime Web sessions for Silero VAD v5, `pyannote/segmentation-3.0`,
        /// and the speaker embedding model, returning a session handle ID.
        #[wasm_bindgen(js_namespace = "__diwopOrtBridge", catch)]
        async fn ort_init_sessions(
            vad_model_bytes: &[u8],
            seg_model_bytes: &[u8],
            embed_model_bytes: &[u8],
        ) -> Result<JsValue, JsValue>;

        /// Runs one Silero VAD v5 step (`input: [1, 576]`, `state: [2, 1, 128]`, `sr: 16000`),
        /// returning a `Float32Array` of length `1 + 256` (`[prob, ...stateN]`).
        #[wasm_bindgen(js_namespace = "__diwopOrtBridge", catch)]
        async fn ort_run_vad(
            handle: &JsValue,
            window_576: &[f32],
            state_256: &[f32],
        ) -> Result<js_sys::Float32Array, JsValue>;

        /// Runs `pyannote/segmentation-3.0` on a speech segment's 16 kHz PCM samples (padded up to 10s),
        /// returning a `Float32Array` of `[num_frames, ...powerset_logits]`.
        #[wasm_bindgen(js_namespace = "__diwopOrtBridge", catch)]
        async fn ort_run_segmentation(
            handle: &JsValue,
            pcm_samples: &[f32],
        ) -> Result<js_sys::Float32Array, JsValue>;

        /// Runs the speaker embedding ONNX model on `[1, num_frames, feat_dim]` Fbank features,
        /// returning the raw embedding `Float32Array`.
        #[wasm_bindgen(js_namespace = "__diwopOrtBridge", catch)]
        async fn ort_run_embedding(
            handle: &JsValue,
            fbank_data: &[f32],
            num_frames: usize,
            feat_dim: usize,
        ) -> Result<js_sys::Float32Array, JsValue>;

        /// Returns the active execution provider label (`"WebGPU"` or `"WASM SIMD"`).
        #[wasm_bindgen(js_namespace = "__diwopOrtBridge")]
        fn ort_get_backend_name(handle: &JsValue) -> String;
    }

    /// WebAssembly VAD and Speaker Diarization engine exposed to browser JavaScript.
    ///
    /// Implements PRD-1, PRD-2, and PRD-3 while keeping active model byte buffers out of
    /// Rust linear memory after ONNX Runtime Web session creation.
    #[wasm_bindgen]
    pub struct DiarizationEngine {
        ort_handle: JsValue,
        pipeline: DiarizationPipeline,
    }

    #[wasm_bindgen]
    impl DiarizationEngine {
        /// Asynchronously initializes the Silero VAD, Pyannote Segmentation-3.0, and Speaker Embedding
        /// ONNX Runtime Web sessions from fetched model byte slices.
        pub async fn new(
            vad_model_bytes: &[u8],
            seg_model_bytes: &[u8],
            embed_model_bytes: &[u8],
        ) -> Result<DiarizationEngine, JsValue> {
            let ort_handle =
                ort_init_sessions(vad_model_bytes, seg_model_bytes, embed_model_bytes).await?;
            Ok(Self {
                ort_handle,
                pipeline: DiarizationPipeline::new(),
            })
        }

        /// Helper that refines a candidate `segment` via `pyannote/segmentation-3.0`, extracts
        /// Fbank features, runs the speaker embedding model, and clusters each sub-turn.
        async fn segment_and_diarize(
            &mut self,
            segment: SpeechSegment,
        ) -> Result<Vec<DiarizedSegment>, JsValue> {
            let seg_out = ort_run_segmentation(&self.ort_handle, &segment.samples).await?;
            let seg_vec = seg_out.to_vec();

            let refined_pairs = if seg_vec.len() > 1 {
                let num_frames = seg_vec[0] as usize;
                let powerset_flat = &seg_vec[1..];
                if num_frames > 0 && powerset_flat.len() >= num_frames * PYANNOTE_NUM_CLASSES {
                    self.pipeline
                        .refine_segment_with_pyannote(segment, powerset_flat, num_frames)
                } else if let Some(fb) = self.pipeline.extract_fbank_for_segment(&segment) {
                    vec![(segment, fb)]
                } else {
                    Vec::new()
                }
            } else if let Some(fb) = self.pipeline.extract_fbank_for_segment(&segment) {
                vec![(segment, fb)]
            } else {
                Vec::new()
            };

            let mut results = Vec::with_capacity(refined_pairs.len());
            for (sub_seg, fbank) in refined_pairs {
                let emb_arr = ort_run_embedding(
                    &self.ort_handle,
                    &fbank.data,
                    fbank.num_frames,
                    fbank.feat_dim,
                )
                .await?;
                let emb_vec = emb_arr.to_vec();
                results.push(self.pipeline.diarize_segment(&sub_seg, &emb_vec));
            }
            Ok(results)
        }

        /// Returns the active ONNX Runtime Web execution provider (`"WebGPU"` or `"WASM SIMD"`).
        pub fn backend_name(&self) -> String {
            ort_get_backend_name(&self.ort_handle)
        }

        /// Returns `true` when the VAD currently detects active speech.
        pub fn is_speaking(&self) -> bool {
            self.pipeline.is_speaking()
        }

        /// Returns the latest VAD speech probability in `[0.0, 1.0]`.
        pub fn last_vad_probability(&self) -> f32 {
            self.pipeline.last_vad_probability()
        }

        /// Feeds downsampled 16 kHz mono PCM `Float32Array` chunks into the engine, runs Silero VAD
        /// and (when speech segments mature) speaker embedding + AHC clustering, and returns a JSON
        /// array of [`DiarizedSegment`] objects. During onboarding, extracts overlapping 1s speech embeddings
        /// and returns an empty array `"[]"`.
        pub async fn process_audio_chunk(&mut self, pcm_data: &[f32]) -> Result<String, JsValue> {
            self.pipeline.push_pcm(pcm_data);

            if self.pipeline.is_onboarding() {
                while let Some((frame, win_576, state_256)) = self.pipeline.next_vad_step() {
                    let vad_out = ort_run_vad(&self.ort_handle, &win_576, &state_256).await?;
                    let out_vec = vad_out.to_vec();
                    if out_vec.len() < 1 + SILERO_STATE_LEN {
                        return Err(JsValue::from_str(
                            "Invalid Silero VAD output length from ONNX bridge",
                        ));
                    }
                    let prob = out_vec[0];
                    let next_state = &out_vec[1..1 + SILERO_STATE_LEN];

                    let fbanks = self
                        .pipeline
                        .complete_onboarding_vad_step(frame, prob, next_state);
                    for fbank in fbanks {
                        let emb_arr = ort_run_embedding(
                            &self.ort_handle,
                            &fbank.data,
                            fbank.num_frames,
                            fbank.feat_dim,
                        )
                        .await?;
                        self.pipeline.record_onboarding_embedding(&emb_arr.to_vec());
                    }
                }
                return Ok("[]".to_string());
            }

            let mut emitted: Vec<DiarizedSegment> = Vec::new();

            while let Some((frame, win_576, state_256)) = self.pipeline.next_vad_step() {
                let vad_out = ort_run_vad(&self.ort_handle, &win_576, &state_256).await?;
                let out_vec = vad_out.to_vec();
                if out_vec.len() < 1 + SILERO_STATE_LEN {
                    return Err(JsValue::from_str(
                        "Invalid Silero VAD output length from ONNX bridge",
                    ));
                }
                let prob = out_vec[0];
                let next_state = &out_vec[1..1 + SILERO_STATE_LEN];

                if let Some(segment) = self.pipeline.complete_vad_step_raw(frame, prob, next_state)
                {
                    let sub_diarized = self.segment_and_diarize(segment).await?;
                    emitted.extend(sub_diarized);
                }
            }

            serde_json::to_string(&emitted)
                .map_err(|e| JsValue::from_str(&format!("Failed to serialize segments: {e}")))
        }

        /// Flushes any trailing active speech segment when the user stops recording and returns
        /// a JSON array containing the final [`DiarizedSegment`] items (if any).
        pub async fn flush(&mut self) -> Result<String, JsValue> {
            if self.pipeline.is_onboarding() {
                if let Some(fbank) = self.pipeline.flush_onboarding_tail() {
                    let emb_arr = ort_run_embedding(
                        &self.ort_handle,
                        &fbank.data,
                        fbank.num_frames,
                        fbank.feat_dim,
                    )
                    .await?;
                    self.pipeline.record_onboarding_embedding(&emb_arr.to_vec());
                }
                return Ok("[]".to_string());
            }

            let mut emitted: Vec<DiarizedSegment> = Vec::new();
            if let Some(segment) = self.pipeline.flush_pending_segment_raw() {
                let sub_diarized = self.segment_and_diarize(segment).await?;
                emitted.extend(sub_diarized);
            }
            serde_json::to_string(&emitted)
                .map_err(|e| JsValue::from_str(&format!("Failed to serialize flush segments: {e}")))
        }

        /// Starts a voice profile onboarding (calibration) session for `profile_id`.
        pub fn start_onboarding(&mut self, profile_id: &str) {
            self.pipeline.start_onboarding(profile_id);
        }

        /// Returns `true` if the engine is currently in calibration/onboarding mode.
        pub fn is_onboarding(&self) -> bool {
            self.pipeline.is_onboarding()
        }

        /// Returns the total seconds of active speech captured during the current onboarding session.
        pub fn onboarding_speech_seconds(&self) -> f32 {
            self.pipeline.onboarding_speech_seconds()
        }

        /// Returns the number of 1-second overlapping embeddings captured during onboarding.
        pub fn onboarding_embedding_count(&self) -> usize {
            self.pipeline.onboarding_embedding_count()
        }

        /// Completes the voice profile onboarding session, calculates the L2-normalized centroid,
        /// registers it with the engine, and returns the serialized `VoiceProfile` JSON string.
        pub fn finish_onboarding(&mut self) -> Result<String, JsValue> {
            let profile = self
                .pipeline
                .finish_onboarding()
                .map_err(|e| JsValue::from_str(&e))?;
            serde_json::to_string(&profile)
                .map_err(|e| JsValue::from_str(&format!("Failed to serialize VoiceProfile: {e}")))
        }

        /// Loads saved voice profiles from a JSON array (`[{"id":"NutzerIn","centroid":[...]}]`)
        /// or JSON map (`{"NutzerIn":[...]}`) into the active diarization session.
        pub fn load_profiles(&mut self, profiles_json: &str) -> Result<(), JsValue> {
            self.pipeline
                .load_profiles_json(profiles_json)
                .map_err(|e| JsValue::from_str(&e))?;
            Ok(())
        }

        /// Fine-tunes the minimum cosine similarity threshold required to match a known profile (e.g. 0.72).
        pub fn set_recognition_threshold(&mut self, threshold: f32) {
            self.pipeline.set_recognition_threshold(threshold);
        }

        /// Retrieves the current speaker cluster state as a JSON array string.
        pub fn get_current_clusters(&self) -> String {
            self.pipeline.current_clusters_json()
        }

        /// Resets internal ring buffers, VAD recurrent state, and speaker clusters for a new session.
        pub fn reset(&mut self) {
            self.pipeline.reset();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_api::DiarizationEngine;

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Generates synthetic 16 kHz mono audio with a distinct fundamental frequency so mock/feature
    /// pipelines exercise realistic waveform -> ring buffer -> VAD -> Fbank -> AHC end-to-end flow.
    fn synth_tone(freq_hz: f32, duration_sec: f32) -> Vec<f32> {
        let count = (SAMPLE_RATE as f32 * duration_sec) as usize;
        (0..count)
            .map(|n| 0.4 * (2.0 * PI * freq_hz * (n as f32 / SAMPLE_RATE as f32)).sin())
            .collect()
    }

    /// End-to-end integration test of `DiarizationPipeline` simulating two speakers alternating turns.
    #[test]
    fn test_end_to_end_diarization_pipeline_two_speakers() {
        let mut pipeline = DiarizationPipeline::new();
        let dummy_state = [0.0_f32; SILERO_STATE_LEN];

        // Helper feeding PCM + simulated VAD probability and deterministic mock speaker embedding
        let mut feed_chunk =
            |pcm: &[f32], vad_prob: f32, mock_emb: &[f32]| -> Vec<DiarizedSegment> {
                pipeline.push_pcm(pcm);
                let mut results = Vec::new();
                while let Some((frame, _win, _state)) = pipeline.next_vad_step() {
                    if let Some((seg, fbank)) =
                        pipeline.complete_vad_step(frame, vad_prob, &dummy_state)
                    {
                        assert_eq!(fbank.feat_dim, 80);
                        assert!(fbank.num_frames > 0);
                        results.push(pipeline.diarize_segment(&seg, mock_emb));
                    }
                }
                results
            };

        let emb_speaker_1 = [0.98_f32, 0.05, 0.0, 0.0];
        let emb_speaker_2 = [0.02_f32, 0.99, 0.0, 0.0];

        // 1. Speaker 1 speaks for 1.0 s + 0.5 s silence
        assert!(feed_chunk(&synth_tone(180.0, 1.0), 0.9, &emb_speaker_1).is_empty());
        let segs_1 = feed_chunk(&vec![0.0; 8_000], 0.05, &emb_speaker_1);
        assert_eq!(segs_1.len(), 1);
        assert_eq!(segs_1[0].speaker_id, "Speaker 1");

        // 2. Speaker 2 speaks for 1.0 s + 0.5 s silence
        assert!(feed_chunk(&synth_tone(320.0, 1.0), 0.9, &emb_speaker_2).is_empty());
        let segs_2 = feed_chunk(&vec![0.0; 8_000], 0.05, &emb_speaker_2);
        assert_eq!(segs_2.len(), 1);
        assert_eq!(segs_2[0].speaker_id, "Speaker 2");

        // 3. Speaker 1 speaks again for 0.9 s and hits Stop (flush)
        assert!(feed_chunk(&synth_tone(185.0, 0.9), 0.88, &emb_speaker_1).is_empty());
        let (flushed_seg, flushed_fbank) = pipeline
            .flush_pending_segment()
            .expect("Expected trailing speech on flush");
        assert!(flushed_fbank.num_frames > 50);
        let seg_3 = pipeline.diarize_segment(&flushed_seg, &emb_speaker_1);
        assert_eq!(seg_3.speaker_id, "Speaker 1");

        let clusters_json = pipeline.current_clusters_json();
        assert!(clusters_json.contains("Speaker 1"));
        assert!(clusters_json.contains("Speaker 2"));

        pipeline.reset();
        assert_eq!(pipeline.current_clusters_json(), "[]");
    }

    /// Verifies voice onboarding (calibration) accumulating overlapping speech chunks,
    /// generating a `VoiceProfile` centroid, and recognizing "NutzerIn" vs an unknown speaker.
    #[test]
    fn test_end_to_end_onboarding_and_recognition_pipeline() {
        let mut pipeline = DiarizationPipeline::new();
        let dummy_state = [0.0_f32; SILERO_STATE_LEN];

        // 1. Start onboarding for "NutzerIn"
        pipeline.start_onboarding("NutzerIn");
        assert!(pipeline.is_onboarding());

        // Simulate 2.5 seconds of speech tone
        let speech = synth_tone(220.0, 2.5);
        pipeline.push_pcm(&speech);

        let user_mock_emb = [0.02_f32, 0.98, 0.01];

        while let Some((frame, _win, _state)) = pipeline.next_vad_step() {
            let fbanks = pipeline.complete_onboarding_vad_step(frame, 0.92, &dummy_state);
            for _ in fbanks {
                pipeline.record_onboarding_embedding(&user_mock_emb);
            }
        }

        if let Some(_tail_fbank) = pipeline.flush_onboarding_tail() {
            pipeline.record_onboarding_embedding(&user_mock_emb);
        }

        assert!(pipeline.onboarding_speech_seconds() >= 2.4);
        assert!(pipeline.onboarding_embedding_count() >= 3);

        let profile = pipeline
            .finish_onboarding()
            .expect("Should produce valid profile");
        assert_eq!(profile.id, "NutzerIn");
        assert!(!profile.centroid.is_empty());
        assert!(!pipeline.is_onboarding());

        // 2. Active diarization session with known profile loaded
        let mut feed_chunk =
            |pcm: &[f32], vad_prob: f32, mock_emb: &[f32]| -> Vec<DiarizedSegment> {
                pipeline.push_pcm(pcm);
                let mut results = Vec::new();
                while let Some((frame, _win, _state)) = pipeline.next_vad_step() {
                    if let Some((seg, _fbank)) =
                        pipeline.complete_vad_step(frame, vad_prob, &dummy_state)
                    {
                        results.push(pipeline.diarize_segment(&seg, mock_emb));
                    }
                }
                results
            };

        // Turn 1: Calibrated NutzerIn speaks for 1.0s + 0.5s silence
        assert!(feed_chunk(&synth_tone(220.0, 1.0), 0.9, &user_mock_emb).is_empty());
        let segs_user = feed_chunk(&vec![0.0; 8_000], 0.05, &user_mock_emb);
        assert_eq!(segs_user.len(), 1);
        assert_eq!(segs_user[0].cluster_id, "cluster_0");
        assert_eq!(segs_user[0].speaker_id, "NutzerIn");
        assert!(segs_user[0].user_likelihood.is_some());
        assert!(segs_user[0].user_likelihood.unwrap() > 0.90);

        // Turn 2: Unknown partner speaks for 1.0s + 0.5s silence -> falls back to Speaker 1
        let partner_mock_emb = [0.99_f32, 0.03, 0.0];
        assert!(feed_chunk(&synth_tone(150.0, 1.0), 0.9, &partner_mock_emb).is_empty());
        let segs_partner = feed_chunk(&vec![0.0; 8_000], 0.05, &partner_mock_emb);
        assert_eq!(segs_partner.len(), 1);
        assert_eq!(segs_partner[0].cluster_id, "cluster_1");
        assert_eq!(segs_partner[0].speaker_id, "Speaker 1");
        assert!(segs_partner[0].user_likelihood.is_some());
        assert!(segs_partner[0].user_likelihood.unwrap() < 0.10);
    }
}
