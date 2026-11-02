use crate::ring_buffer::{
    AudioFrame, SAMPLE_RATE, VAD_CONTEXT_SAMPLES, VAD_FRAME_SAMPLES, VAD_WINDOW_SAMPLES,
};

/// Flattened element count of Silero VAD v5 recurrent LSTM state tensor (`[2, 1, 128]`).
pub const SILERO_STATE_LEN: usize = 2 * 128;

/// Number of pre-roll frames (`2 * 32 ms = 64 ms`) retained prior to speech trigger so
/// word-initial consonants/plosives are included in speaker embedding segments.
const PRE_ROLL_FRAMES: usize = 2;

/// Configuration parameters controlling Voice Activity Detection segmentation and chunking.
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// Speech probability threshold (`0.0..=1.0`) required to trigger speech onset.
    pub speech_threshold: f32,
    /// Hysteresis probability threshold below which active speech begins counting toward silence.
    pub silence_threshold: f32,
    /// Minimum contiguous speech duration (in samples @ 16 kHz) required for a valid speaker segment.
    pub min_speech_samples: usize,
    /// Contiguous silence duration (in samples @ 16 kHz) required to finalize an active speech segment.
    pub min_silence_samples: usize,
    /// Early checkpoint duration (in samples @ 16 kHz) at which an interim speaker identification
    /// segment is emitted during an ongoing utterance so the UI can label the live card quickly.
    pub early_checkpoint_samples: usize,
    /// Maximum segment length (in samples @ 16 kHz, ~3.0 s) before emitting a chunk and continuing.
    pub max_segment_samples: usize,
}

impl Default for VadConfig {
    /// Returns balanced real-time defaults for 16 kHz conversational speech:
    /// - `speech_threshold = 0.50`, `silence_threshold = 0.35`
    /// - `min_speech = 400 ms` (`6,400` samples)
    /// - `min_silence = 352 ms` (`5,632` samples = 11 frames)
    /// - `early_checkpoint = 1.44 s` (`23,040` samples)
    /// - `max_segment = 3.0 s` (`48,000` samples)
    fn default() -> Self {
        Self {
            speech_threshold: 0.50,
            silence_threshold: 0.35,
            min_speech_samples: 6_400,
            min_silence_samples: 5_632,
            early_checkpoint_samples: 23_040,
            max_segment_samples: 48_000,
        }
    }
}

/// A contiguous speech audio segment isolated by [`VadStateMachine`], ready for speaker embedding extraction.
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Start timestamp in seconds from the beginning of the recording session.
    pub start_time: f64,
    /// End timestamp in seconds from the beginning of the recording session.
    pub end_time: f64,
    /// `true` when emitted as an early mid-utterance checkpoint before silence has occurred.
    pub is_checkpoint: bool,
    /// Isolated 16 kHz mono PCM speech samples.
    pub samples: Vec<f32>,
}

/// Stateful Voice Activity Detection processor managing Silero VAD v5 context, recurrent LSTM state,
/// pre-roll buffering, hysteresis smoothing, and segment emission.
pub struct VadStateMachine {
    config: VadConfig,
    /// Silero VAD v5 recurrent LSTM state (`[2, 1, 128]` = 256 floats).
    lstm_state: [f32; SILERO_STATE_LEN],
    /// Trailing 64 samples from the previous frame prepended to the next 512-sample window.
    context_overlap: [f32; VAD_CONTEXT_SAMPLES],
    /// Scratch `[1, 576]` window buffer (`64` context + `512` new samples).
    window_buf: [f32; VAD_WINDOW_SAMPLES],
    /// Rolling pre-roll frames kept while in silence to capture speech onset cleanly.
    pre_roll: Vec<AudioFrame>,
    /// Whether the VAD is currently inside an active speech utterance.
    in_speech: bool,
    /// Session sample offset where the current active speech segment started.
    segment_start_sample: u64,
    /// Session sample offset of the most recent frame with `prob >= silence_threshold`.
    last_voiced_end_sample: u64,
    /// Number of consecutive trailing silent samples observed inside `in_speech`.
    trailing_silence_samples: usize,
    /// Whether the early checkpoint has already fired for the current contiguous segment window.
    emitted_early_checkpoint: bool,
    /// Pre-allocated speech sample accumulator for the active segment.
    active_samples: Vec<f32>,
    /// Last speech probability returned by the VAD model.
    last_probability: f32,
}

impl VadStateMachine {
    /// Creates a new [`VadStateMachine`] with pre-allocated segment buffers and zeroed LSTM state.
    pub fn new(config: VadConfig) -> Self {
        let pre_alloc = config.max_segment_samples + PRE_ROLL_FRAMES * VAD_FRAME_SAMPLES + 1024;
        Self {
            config,
            lstm_state: [0.0; SILERO_STATE_LEN],
            context_overlap: [0.0; VAD_CONTEXT_SAMPLES],
            window_buf: [0.0; VAD_WINDOW_SAMPLES],
            pre_roll: Vec::with_capacity(PRE_ROLL_FRAMES),
            in_speech: false,
            segment_start_sample: 0,
            last_voiced_end_sample: 0,
            trailing_silence_samples: 0,
            emitted_early_checkpoint: false,
            active_samples: Vec::with_capacity(pre_alloc),
            last_probability: 0.0,
        }
    }

    /// Resets recurrent LSTM state, context overlap, and active speech buffers for a new session.
    pub fn reset(&mut self) {
        self.lstm_state.fill(0.0);
        self.context_overlap.fill(0.0);
        self.window_buf.fill(0.0);
        self.pre_roll.clear();
        self.in_speech = false;
        self.segment_start_sample = 0;
        self.last_voiced_end_sample = 0;
        self.trailing_silence_samples = 0;
        self.emitted_early_checkpoint = false;
        self.active_samples.clear();
        self.last_probability = 0.0;
    }

    /// Returns a reference to the active VAD configuration parameters.
    pub fn config(&self) -> &VadConfig {
        &self.config
    }

    /// Returns whether the state machine is currently tracking an active speech segment.
    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// Returns the most recent speech probability (`0.0..=1.0`).
    pub fn last_probability(&self) -> f32 {
        self.last_probability
    }

    /// Prepares the `[1, 576]` Silero VAD v5 input window (`64` context samples + `512` frame samples)
    /// and returns slices to `(window_576, current_lstm_state_256)` for ONNX inference.
    pub fn prepare_silero_input(
        &mut self,
        frame: &AudioFrame,
    ) -> (&[f32; VAD_WINDOW_SAMPLES], &[f32; SILERO_STATE_LEN]) {
        self.window_buf[..VAD_CONTEXT_SAMPLES].copy_from_slice(&self.context_overlap);
        self.window_buf[VAD_CONTEXT_SAMPLES..].copy_from_slice(&frame.samples);
        self.context_overlap
            .copy_from_slice(&frame.samples[VAD_FRAME_SAMPLES - VAD_CONTEXT_SAMPLES..]);
        (&self.window_buf, &self.lstm_state)
    }

    /// Updates the internal `[2, 1, 128]` recurrent LSTM state from Silero VAD's `stateN` output.
    pub fn update_lstm_state(&mut self, next_state: &[f32]) {
        if next_state.len() == SILERO_STATE_LEN {
            self.lstm_state.copy_from_slice(next_state);
        }
    }

    /// Advances the VAD state machine by one 512-sample `frame` given its Silero speech `probability`,
    /// returning `Some(SpeechSegment)` whenever an early checkpoint, max-duration window, or completed
    /// utterance is ready for speaker embedding extraction.
    pub fn accept_frame_probability(
        &mut self,
        frame: AudioFrame,
        probability: f32,
    ) -> Option<SpeechSegment> {
        self.last_probability = probability;
        let frame_end_sample = frame.start_sample + VAD_FRAME_SAMPLES as u64;

        if !self.in_speech {
            if probability >= self.config.speech_threshold {
                self.in_speech = true;
                self.trailing_silence_samples = 0;
                self.emitted_early_checkpoint = false;
                self.active_samples.clear();

                if let Some(first_pre) = self.pre_roll.first() {
                    self.segment_start_sample = first_pre.start_sample;
                    for pre_frame in &self.pre_roll {
                        self.active_samples.extend_from_slice(&pre_frame.samples);
                    }
                } else {
                    self.segment_start_sample = frame.start_sample;
                }
                self.pre_roll.clear();
                self.active_samples.extend_from_slice(&frame.samples);
                self.last_voiced_end_sample = frame_end_sample;
            } else {
                if self.pre_roll.len() == PRE_ROLL_FRAMES {
                    self.pre_roll.remove(0);
                }
                self.pre_roll.push(frame);
            }
            return None;
        }

        // Currently in active speech
        self.active_samples.extend_from_slice(&frame.samples);

        if probability >= self.config.silence_threshold {
            self.trailing_silence_samples = 0;
            self.last_voiced_end_sample = frame_end_sample;
        } else {
            self.trailing_silence_samples += VAD_FRAME_SAMPLES;
        }

        // 1. Silence hangover reached -> finalize utterance
        if self.trailing_silence_samples >= self.config.min_silence_samples {
            let total_len = self.active_samples.len();
            let voiced_len = total_len.saturating_sub(self.trailing_silence_samples);
            let start_sample = self.segment_start_sample;
            let end_sample = self
                .last_voiced_end_sample
                .max(start_sample + voiced_len as u64);

            // Seed pre_roll from the trailing frames of the silence hangover so back-to-back speech
            // immediately after hangover retains full 64 ms pre-roll.
            self.pre_roll.clear();
            let available_tail_frames = (total_len / VAD_FRAME_SAMPLES).min(PRE_ROLL_FRAMES);
            for k in (0..available_tail_frames).rev() {
                let tail_offset = total_len - (k + 1) * VAD_FRAME_SAMPLES;
                let mut samples = [0.0_f32; VAD_FRAME_SAMPLES];
                samples.copy_from_slice(
                    &self.active_samples[tail_offset..tail_offset + VAD_FRAME_SAMPLES],
                );
                let tail_start_sample = frame_end_sample - ((k + 1) * VAD_FRAME_SAMPLES) as u64;
                self.pre_roll.push(AudioFrame {
                    start_sample: tail_start_sample,
                    samples,
                });
            }

            self.in_speech = false;
            self.trailing_silence_samples = 0;
            self.emitted_early_checkpoint = false;

            if voiced_len >= self.config.min_speech_samples {
                let samples = self.active_samples[..voiced_len].to_vec();
                self.active_samples.clear();
                return Some(SpeechSegment {
                    start_time: start_sample as f64 / SAMPLE_RATE as f64,
                    end_time: end_sample as f64 / SAMPLE_RATE as f64,
                    is_checkpoint: false,
                    samples,
                });
            }

            self.active_samples.clear();
            return None;
        }

        // 2. Maximum segment length (~3.0 s) reached during continuous speech -> emit window and roll forward
        if self.active_samples.len() >= self.config.max_segment_samples {
            let start_sample = self.segment_start_sample;
            let end_sample = frame_end_sample;
            let samples = self.active_samples.clone();

            // Retain a 1.0 s (16,000-sample) acoustic overlap for smooth speaker continuity on the next chunk
            let overlap = SAMPLE_RATE.min(self.active_samples.len());
            let drain_count = self.active_samples.len() - overlap;
            self.active_samples.drain(..drain_count);
            self.segment_start_sample = end_sample - overlap as u64;
            self.emitted_early_checkpoint = true;

            return Some(SpeechSegment {
                start_time: start_sample as f64 / SAMPLE_RATE as f64,
                end_time: end_sample as f64 / SAMPLE_RATE as f64,
                is_checkpoint: false,
                samples,
            });
        }

        // 3. Early identification checkpoint (~1.44 s) so live speech gets a speaker label promptly
        if !self.emitted_early_checkpoint
            && self.trailing_silence_samples == 0
            && self.active_samples.len() >= self.config.early_checkpoint_samples
        {
            self.emitted_early_checkpoint = true;
            return Some(SpeechSegment {
                start_time: self.segment_start_sample as f64 / SAMPLE_RATE as f64,
                end_time: frame_end_sample as f64 / SAMPLE_RATE as f64,
                is_checkpoint: true,
                samples: self.active_samples.clone(),
            });
        }

        None
    }

    /// Flushes any remaining active speech segment when recording stops, provided it meets `min_speech_samples`.
    pub fn flush(&mut self) -> Option<SpeechSegment> {
        if !self.in_speech {
            return None;
        }
        let voiced_len = self
            .active_samples
            .len()
            .saturating_sub(self.trailing_silence_samples);
        let start_sample = self.segment_start_sample;
        let end_sample = self
            .last_voiced_end_sample
            .max(start_sample + voiced_len as u64);

        self.in_speech = false;
        self.trailing_silence_samples = 0;
        self.emitted_early_checkpoint = false;

        if voiced_len >= self.config.min_speech_samples {
            let samples = self.active_samples[..voiced_len].to_vec();
            self.active_samples.clear();
            Some(SpeechSegment {
                start_time: start_sample as f64 / SAMPLE_RATE as f64,
                end_time: end_sample as f64 / SAMPLE_RATE as f64,
                is_checkpoint: false,
                samples,
            })
        } else {
            self.active_samples.clear();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(idx: u64, value: f32) -> AudioFrame {
        AudioFrame {
            start_sample: idx * VAD_FRAME_SAMPLES as u64,
            samples: [value; VAD_FRAME_SAMPLES],
        }
    }

    /// Verifies Silero VAD v5 64-sample context overlap preparation across sequential 512-sample frames.
    #[test]
    fn test_prepare_silero_input_maintains_64_sample_context() {
        let mut vad = VadStateMachine::new(VadConfig::default());
        let mut f0 = make_frame(0, 0.25);
        f0.samples[VAD_FRAME_SAMPLES - 1] = 0.99;

        let (win0, _) = vad.prepare_silero_input(&f0);
        // First 64 context samples start at 0.0
        assert_eq!(win0[0], 0.0);
        assert_eq!(win0[63], 0.0);
        assert_eq!(win0[64], 0.25);
        assert_eq!(win0[575], 0.99);

        let f1 = make_frame(1, 0.5);
        let (win1, _) = vad.prepare_silero_input(&f1);
        // Trailing 64 samples of f0 become leading 64 context samples of win1
        assert_eq!(win1[0], 0.25);
        assert_eq!(win1[63], 0.99);
        assert_eq!(win1[64], 0.5);
    }

    /// Verifies pre-roll retention, short-noise rejection, and utterance completion on silence hangover.
    #[test]
    fn test_vad_segment_lifecycle_and_short_noise_rejection() {
        let mut vad = VadStateMachine::new(VadConfig::default());

        // 3 silent frames -> 2 frames retained in pre-roll (starting at sample 512)
        for i in 0..3 {
            assert!(vad
                .accept_frame_probability(make_frame(i, 0.01), 0.1)
                .is_none());
        }

        // Brief 3-frame noise spike (< 400 ms min_speech_samples) followed by 12 silent frames -> rejected
        for i in 3..6 {
            assert!(vad
                .accept_frame_probability(make_frame(i, 0.5), 0.8)
                .is_none());
        }
        for i in 6..18 {
            assert!(vad
                .accept_frame_probability(make_frame(i, 0.0), 0.05)
                .is_none());
        }
        assert!(!vad.is_in_speech());

        // Real 1.0 s speech (32 frames = 16,384 samples) from frame 18..50, then 11 silent frames
        for i in 18..50 {
            assert!(vad
                .accept_frame_probability(make_frame(i, 0.4), 0.85)
                .is_none());
        }
        let mut completed = None;
        for i in 50..62 {
            if let Some(seg) = vad.accept_frame_probability(make_frame(i, 0.0), 0.05) {
                completed = Some(seg);
                break;
            }
        }

        let seg = completed.expect("Expected completed speech segment after silence hangover");
        assert!(!seg.is_checkpoint);
        // Pre-roll includes 2 frames prior to frame 18 -> frame 16 (16 * 512 / 16000 = 0.512 s)
        assert!((seg.start_time - 0.512).abs() < 1e-6);
        // Speech ended at end of frame 49 -> sample 50 * 512 = 25600 -> 1.600 s
        assert!((seg.end_time - 1.600).abs() < 1e-6);
        assert_eq!(seg.samples.len(), (50 - 16) * VAD_FRAME_SAMPLES);
    }

    /// Verifies early checkpoint at ~1.44 s and max-segment split at ~3.0 s during continuous speech.
    #[test]
    fn test_vad_early_checkpoint_and_max_segment_split() {
        let mut vad = VadStateMachine::new(VadConfig::default());
        let mut checkpoints = 0;
        let mut full_chunks = 0;

        // 105 frames of continuous speech (~3.36 s)
        for i in 0..105 {
            if let Some(seg) = vad.accept_frame_probability(make_frame(i, 0.3), 0.9) {
                if seg.is_checkpoint {
                    checkpoints += 1;
                } else {
                    full_chunks += 1;
                    assert!(seg.samples.len() >= 48_000);
                }
            }
        }

        assert_eq!(checkpoints, 1, "Should emit exactly one early checkpoint");
        assert_eq!(full_chunks, 1, "Should split at 3.0 s max segment boundary");

        // Flush remaining speech on stop
        let tail = vad.flush().expect("Expected trailing speech on flush");
        assert!(!tail.is_checkpoint);
    }
}
