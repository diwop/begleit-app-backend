use crate::ring_buffer::SAMPLE_RATE;
use crate::vad::SpeechSegment;

/// Fixed input audio window length expected by `pyannote/segmentation-3.0` (`10.0 s @ 16 kHz = 160,000 samples`).
pub const PYANNOTE_WINDOW_SAMPLES: usize = SAMPLE_RATE * 10;

/// Number of powerset output classes emitted per frame by `pyannote/segmentation-3.0`:
/// `0 = silence`, `1 = spk1`, `2 = spk2`, `3 = spk3`, `4 = spk1+spk2`, `5 = spk1+spk3`, `6 = spk2+spk3`.
pub const PYANNOTE_NUM_CLASSES: usize = 7;

/// Minimum contiguous sub-turn duration (`400 ms = 6,400 samples` @ 16 kHz) required to split a
/// multi-speaker VAD block or retain an overlap-masked sample buffer for embedding extraction.
pub const MIN_SUBTURN_SAMPLES: usize = (SAMPLE_RATE * 400) / 1_000;

/// Overlap posterior threshold (`p[4] + p[5] + p[6] >= 0.35`) above which a frame is treated as
/// simultaneous multi-speaker overlap and masked out prior to speaker embedding extraction.
const OVERLAP_MASK_THRESHOLD: f32 = 0.35;

/// Minimum smoothed single-speaker marginal probability required to mark a frame as voiced by a local speaker.
const SPEAKER_ACTIVE_THRESHOLD: f32 = 0.40;

/// Smoothing half-window (`2` frames on each side = `5` frames ≈ `85 ms`) used to suppress single-frame jitter.
const SMOOTH_HALF_WINDOW: usize = 2;

/// Maximum gap in frames (`18 frames ≈ 304 ms`) bridged when the same local speaker resumes after a brief dip.
const MAX_SAME_SPEAKER_GAP_FRAMES: usize = 18;

/// Minimum contiguous frames (`24 frames ≈ 405 ms`) required for a secondary local speaker island to trigger a split.
const MIN_SPLIT_ISLAND_FRAMES: usize = 24;

/// Numerically stable softmax over a 7-element powerset logit or log-probability vector.
///
/// Because $\text{softmax}(\log p) = p$, this function returns exact class probabilities `[0.0, 1.0]`
/// whether the ONNX graph terminates in raw logits or `LogSoftmax`.
#[inline]
pub fn powerset_softmax(logits: &[f32; PYANNOTE_NUM_CLASSES]) -> [f32; PYANNOTE_NUM_CLASSES] {
    let max_val = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut exps = [0.0_f32; PYANNOTE_NUM_CLASSES];
    let mut sum = 0.0_f32;
    for (i, &val) in logits.iter().enumerate() {
        let e = (val - max_val).exp();
        exps[i] = e;
        sum += e;
    }
    if sum > 1e-12 {
        for e in &mut exps {
            *e /= sum;
        }
    }
    exps
}

/// Smoothed per-frame speaker and overlap posteriors derived from the 7-class powerset distribution.
#[derive(Debug, Clone, Copy)]
struct FramePosterior {
    /// Smoothed marginal activity for local speaker 1, 2, and 3.
    spk: [f32; 3],
    /// Smoothed probability of overlapping speech (`p[4] + p[5] + p[6]`).
    overlap: f32,
}

/// Refines Silero VAD speech segments using `pyannote/segmentation-3.0` frame-level powerset posteriors
/// to split zero-gap speaker switches (`< 350 ms` pause) and mask out overlapping speech frames.
#[derive(Debug, Default, Clone)]
pub struct PyannoteSegmenter;

impl PyannoteSegmenter {
    /// Creates a new [`PyannoteSegmenter`].
    pub fn new() -> Self {
        Self
    }

    /// Refines a candidate [`SpeechSegment`] using the flattened `[num_frames, 7]` powerset output from
    /// `pyannote/segmentation-3.0` evaluated on a `160,000`-sample (`10.0 s`) window starting at `segment.start_time`.
    ///
    /// - If only a single local speaker is active, returns one [`SpeechSegment`] preserving `[start_time, end_time]`
    ///   while masking out overlapping speech frames (`p_overlap >= 0.35`) when at least `400 ms` of pure speech remains.
    /// - If two or more local speakers speak back-to-back (`>= 400 ms` each), splits the segment into separate
    ///   chronological [`SpeechSegment`] sub-turns with overlap-masked audio samples.
    pub fn refine_segment(
        &self,
        segment: &SpeechSegment,
        powerset_flat: &[f32],
        num_frames: usize,
    ) -> Vec<SpeechSegment> {
        let total_samples = segment.samples.len().min(PYANNOTE_WINDOW_SAMPLES);
        if num_frames == 0
            || powerset_flat.len() < num_frames * PYANNOTE_NUM_CLASSES
            || total_samples < MIN_SUBTURN_SAMPLES
        {
            return vec![segment.clone()];
        }

        // Determine how many output frames fall within `total_samples` of the 10.0s (160,000-sample) window
        let valid_frames =
            ((total_samples * num_frames) / PYANNOTE_WINDOW_SAMPLES).clamp(1, num_frames);

        // 1. Decode powerset probabilities per frame
        let mut raw_posteriors = Vec::with_capacity(valid_frames);
        for t in 0..valid_frames {
            let base = t * PYANNOTE_NUM_CLASSES;
            let mut row = [0.0_f32; PYANNOTE_NUM_CLASSES];
            row.copy_from_slice(&powerset_flat[base..base + PYANNOTE_NUM_CLASSES]);
            let p = powerset_softmax(&row);

            // Weight pure single-speaker classes full and overlap classes at 0.5 for boundary attribution
            let spk1 = p[1] + 0.5 * (p[4] + p[5]);
            let spk2 = p[2] + 0.5 * (p[4] + p[6]);
            let spk3 = p[3] + 0.5 * (p[5] + p[6]);
            let overlap = (p[4] + p[5] + p[6]).clamp(0.0, 1.0);

            raw_posteriors.push(FramePosterior {
                spk: [spk1, spk2, spk3],
                overlap,
            });
        }

        // 2. Apply 5-frame (~85 ms) moving-average smoothing
        let mut smoothed = Vec::with_capacity(valid_frames);
        for t in 0..valid_frames {
            let start = t.saturating_sub(SMOOTH_HALF_WINDOW);
            let end = (t + SMOOTH_HALF_WINDOW + 1).min(valid_frames);
            let count = (end - start) as f32;
            let mut sum_spk = [0.0_f32; 3];
            let mut sum_ovl = 0.0_f32;
            for item in &raw_posteriors[start..end] {
                sum_spk[0] += item.spk[0];
                sum_spk[1] += item.spk[1];
                sum_spk[2] += item.spk[2];
                sum_ovl += item.overlap;
            }
            smoothed.push(FramePosterior {
                spk: [sum_spk[0] / count, sum_spk[1] / count, sum_spk[2] / count],
                overlap: sum_ovl / count,
            });
        }

        // 3. Assign dominant local speaker label (0 = silence/ambiguous, 1..=3 = local speaker ID)
        let mut dom_spk: Vec<u8> = smoothed
            .iter()
            .map(|fp| {
                let (best_idx, best_val) = fp
                    .spk
                    .iter()
                    .copied()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or((0, 0.0));
                if best_val >= SPEAKER_ACTIVE_THRESHOLD && fp.overlap < 0.65 {
                    (best_idx + 1) as u8
                } else {
                    0
                }
            })
            .collect();

        // 4. Bridge short silence/overlap dips (<= 18 frames) between identical speakers
        let mut i = 0;
        while i < valid_frames {
            if dom_spk[i] == 0 {
                let gap_start = i;
                while i < valid_frames && dom_spk[i] == 0 {
                    i += 1;
                }
                let gap_len = i - gap_start;
                if gap_start > 0
                    && i < valid_frames
                    && dom_spk[gap_start - 1] == dom_spk[i]
                    && gap_len <= MAX_SAME_SPEAKER_GAP_FRAMES
                {
                    let fill = dom_spk[gap_start - 1];
                    for slot in &mut dom_spk[gap_start..i] {
                        *slot = fill;
                    }
                }
            } else {
                i += 1;
            }
        }

        // 5. Absorb micro-islands (< 24 frames ≈ 405 ms) into surrounding speaker to prevent false splits
        let mut runs = Self::collect_runs(&dom_spk);
        if runs.len() > 1 {
            for r_idx in 0..runs.len() {
                let (spk, start_f, end_f) = runs[r_idx];
                if spk != 0 && (end_f - start_f) < MIN_SPLIT_ISLAND_FRAMES {
                    let replacement = if r_idx > 0 && runs[r_idx - 1].0 != 0 {
                        runs[r_idx - 1].0
                    } else if r_idx + 1 < runs.len() && runs[r_idx + 1].0 != 0 {
                        runs[r_idx + 1].0
                    } else {
                        0
                    };
                    if replacement != 0 {
                        for slot in &mut dom_spk[start_f..end_f] {
                            *slot = replacement;
                        }
                    }
                }
            }
            runs = Self::collect_runs(&dom_spk);
        }

        // Filter to active speaker runs (spk != 0) that meet `MIN_SUBTURN_SAMPLES`
        let voiced_runs: Vec<(u8, usize, usize)> = runs
            .into_iter()
            .filter(|&(spk, start_f, end_f)| {
                if spk == 0 {
                    return false;
                }
                let start_s = Self::frame_to_sample(start_f, num_frames, total_samples);
                let end_s = Self::frame_to_sample(end_f, num_frames, total_samples);
                end_s.saturating_sub(start_s) >= MIN_SUBTURN_SAMPLES
            })
            .collect();

        // Check if multiple distinct local speakers are present
        let first_spk = voiced_runs.first().map(|r| r.0).unwrap_or(0);
        let has_speaker_switch =
            voiced_runs.len() >= 2 && voiced_runs.iter().any(|&(spk, _, _)| spk != first_spk);

        if !has_speaker_switch {
            // Single speaker turn: preserve original [start_time, end_time] and mask out overlap frames
            let clean_samples = Self::extract_non_overlapping_samples(
                &segment.samples[..total_samples],
                &smoothed,
                0,
                valid_frames,
                num_frames,
            );
            return vec![SpeechSegment {
                start_time: segment.start_time,
                end_time: segment.end_time,
                is_checkpoint: segment.is_checkpoint,
                samples: clean_samples,
            }];
        }

        // Multiple speakers detected inside a single VAD segment: emit split sub-turns
        let mut sub_segments = Vec::with_capacity(voiced_runs.len());
        for (_spk, start_f, end_f) in voiced_runs {
            let start_s = Self::frame_to_sample(start_f, num_frames, total_samples);
            let end_s = Self::frame_to_sample(end_f, num_frames, total_samples);
            if end_s.saturating_sub(start_s) < MIN_SUBTURN_SAMPLES {
                continue;
            }
            let sub_start_time = segment.start_time + (start_s as f64 / SAMPLE_RATE as f64);
            let sub_end_time = segment.start_time + (end_s as f64 / SAMPLE_RATE as f64);
            let clean_samples = Self::extract_non_overlapping_samples(
                &segment.samples[..total_samples],
                &smoothed,
                start_f,
                end_f,
                num_frames,
            );

            sub_segments.push(SpeechSegment {
                start_time: sub_start_time,
                end_time: sub_end_time,
                is_checkpoint: segment.is_checkpoint,
                samples: clean_samples,
            });
        }

        if sub_segments.is_empty() {
            vec![segment.clone()]
        } else {
            sub_segments
        }
    }

    /// Maps a frame index `frame_idx` in `0..=num_frames` to a sample offset clamped to `total_samples`.
    #[inline]
    fn frame_to_sample(frame_idx: usize, num_frames: usize, total_samples: usize) -> usize {
        ((frame_idx * PYANNOTE_WINDOW_SAMPLES) / num_frames).min(total_samples)
    }

    /// Collects contiguous `(speaker_label, start_frame, end_frame)` runs from `dom_spk`.
    fn collect_runs(dom_spk: &[u8]) -> Vec<(u8, usize, usize)> {
        if dom_spk.is_empty() {
            return Vec::new();
        }
        let mut runs = Vec::new();
        let mut curr = dom_spk[0];
        let mut start = 0;
        for (i, &s) in dom_spk.iter().enumerate().skip(1) {
            if s != curr {
                runs.push((curr, start, i));
                curr = s;
                start = i;
            }
        }
        runs.push((curr, start, dom_spk.len()));
        runs
    }

    /// Extracts pure single-speaker PCM samples across `start_f..end_f` by excluding frames where
    /// `smoothed[t].overlap >= OVERLAP_MASK_THRESHOLD`. Falls back to the unmasked slice if masking
    /// leaves fewer than [`MIN_SUBTURN_SAMPLES`] (`400 ms`).
    fn extract_non_overlapping_samples(
        samples: &[f32],
        smoothed: &[FramePosterior],
        start_f: usize,
        end_f: usize,
        num_frames: usize,
    ) -> Vec<f32> {
        let total_samples = samples.len();
        let span_start = Self::frame_to_sample(start_f, num_frames, total_samples);
        let span_end = Self::frame_to_sample(end_f, num_frames, total_samples);

        let mut clean = Vec::with_capacity(span_end.saturating_sub(span_start));
        for (t, fp) in smoothed.iter().enumerate().take(end_f).skip(start_f) {
            if fp.overlap < OVERLAP_MASK_THRESHOLD {
                let s0 = Self::frame_to_sample(t, num_frames, total_samples);
                let s1 = Self::frame_to_sample(t + 1, num_frames, total_samples);
                if s1 > s0 {
                    clean.extend_from_slice(&samples[s0..s1]);
                }
            }
        }

        if clean.len() >= MIN_SUBTURN_SAMPLES {
            clean
        } else {
            samples[span_start..span_end].to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic `[num_frames, 7]` powerset tensor where `active_class` has logit `+6.0`
    /// and all other classes have logit `-6.0`.
    fn make_powerset_frames(num_frames: usize, class_fn: impl Fn(usize) -> usize) -> Vec<f32> {
        let mut flat = vec![-6.0_f32; num_frames * PYANNOTE_NUM_CLASSES];
        for t in 0..num_frames {
            let c = class_fn(t).min(PYANNOTE_NUM_CLASSES - 1);
            flat[t * PYANNOTE_NUM_CLASSES + c] = 6.0;
        }
        flat
    }

    /// Verifies that `powerset_softmax` produces identical probability distributions for raw logits
    /// and log-softmax inputs.
    #[test]
    fn test_powerset_softmax_invariance() {
        let logits = [0.0_f32, 2.0, -1.0, -3.0, 0.5, -2.0, -4.0];
        let probs = powerset_softmax(&logits);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);

        let mut log_probs = [0.0_f32; PYANNOTE_NUM_CLASSES];
        for i in 0..PYANNOTE_NUM_CLASSES {
            log_probs[i] = probs[i].ln();
        }
        let probs_from_log = powerset_softmax(&log_probs);
        for (a, b) in probs.iter().zip(probs_from_log.iter()) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    /// Verifies that `PyannoteSegmenter` splits a zero-gap two-speaker 2.4s VAD block into two
    /// separate `SpeechSegment` sub-turns while masking out an overlapping transition region.
    #[test]
    fn test_refine_segment_splits_zero_gap_speaker_switch_and_masks_overlap() {
        let segmenter = PyannoteSegmenter::new();
        let num_frames = 589; // Standard 10.0s pyannote frame count

        // 2.4 seconds of audio (38,400 samples): first half has amplitude +0.25, second half has -0.75
        let mut samples = vec![0.25_f32; 19_200];
        samples.extend(std::iter::repeat_n(-0.75_f32, 19_200));

        let seg = SpeechSegment {
            start_time: 1.0,
            end_time: 3.4,
            is_checkpoint: false,
            samples,
        };

        // Frame 0..65 (~0.0s..1.1s): Local Speaker 1 (class 1)
        // Frame 65..76 (~1.1s..1.29s): Overlap Speaker 1 + Speaker 2 (class 4)
        // Frame 76..142 (~1.29s..2.4s): Local Speaker 2 (class 2)
        let powerset = make_powerset_frames(num_frames, |t| {
            if t < 65 {
                1
            } else if t < 76 {
                4
            } else if t < 142 {
                2
            } else {
                0
            }
        });

        let refined = segmenter.refine_segment(&seg, &powerset, num_frames);
        assert_eq!(
            refined.len(),
            2,
            "Zero-gap speaker handoff should split into 2 sub-segments"
        );

        // Sub-segment 1 should start at 1.0s and end around ~2.1s..2.2s
        assert!((refined[0].start_time - 1.0).abs() < 0.05);
        assert!(refined[0].end_time > 2.0 && refined[0].end_time < 2.35);
        assert!(refined[0].samples.len() >= MIN_SUBTURN_SAMPLES);
        assert!(
            refined[0].samples.iter().all(|&x| x > 0.0),
            "First sub-segment should contain only Speaker 1 samples (+0.25)"
        );

        // Sub-segment 2 should cover the second speaker (~2.2s..3.4s)
        assert!(refined[1].start_time >= refined[0].end_time - 0.05);
        assert!((refined[1].end_time - 3.4).abs() < 0.05);
        assert!(refined[1].samples.len() >= MIN_SUBTURN_SAMPLES);
        assert!(
            refined[1].samples.iter().all(|&x| x < 0.0),
            "Second sub-segment should contain only Speaker 2 samples (-0.75)"
        );
    }

    /// Verifies that a single-speaker segment with a brief overlapping backchannel keeps its
    /// [start_time, end_time] intact while stripping the overlapped frames from `samples`.
    #[test]
    fn test_refine_segment_masks_overlap_in_single_speaker_turn() {
        let segmenter = PyannoteSegmenter::new();
        let num_frames = 589;

        // 2.0 seconds (32,000 samples ≈ 117 frames)
        let mut samples = vec![0.4_f32; 32_000];
        // Mark frames 40..55 (~10,860..14,930) with corrupted overlap value 99.0
        for slot in &mut samples[11_500..14_200] {
            *slot = 99.0;
        }

        let seg = SpeechSegment {
            start_time: 0.0,
            end_time: 2.0,
            is_checkpoint: false,
            samples,
        };

        let powerset = make_powerset_frames(num_frames, |t| {
            if (38..58).contains(&t) {
                4 // Overlap spk1 + spk2
            } else if t < 118 {
                1 // Pure spk1
            } else {
                0
            }
        });

        let refined = segmenter.refine_segment(&seg, &powerset, num_frames);
        assert_eq!(refined.len(), 1);
        assert_eq!(refined[0].start_time, 0.0);
        assert_eq!(refined[0].end_time, 2.0);
        assert!(refined[0].samples.len() < 32_000);
        assert!(refined[0].samples.len() >= MIN_SUBTURN_SAMPLES);
        assert!(
            refined[0].samples.iter().all(|&v| (v - 0.4).abs() < 1e-5),
            "Corrupted overlap region (99.0) must be masked out of clean samples"
        );
    }
}
