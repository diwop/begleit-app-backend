use crate::ring_buffer::SAMPLE_RATE;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

/// Number of Mel filterbank bins expected by WeSpeaker / 3D-Speaker ONNX models.
pub const NUM_MEL_BINS: usize = 80;

/// Frame window length at 16 kHz (`25 ms = 400 samples`).
const FRAME_LEN: usize = 400;

/// Frame shift (hop) at 16 kHz (`10 ms = 160 samples`).
const FRAME_SHIFT: usize = 160;

/// FFT length (`next_power_of_two(400) = 512`).
const FFT_LEN: usize = 512;

/// Number of non-redundant positive-frequency FFT bins (`512 / 2 + 1 = 257`).
const NUM_FFT_BINS: usize = FFT_LEN / 2 + 1;

/// Pre-emphasis coefficient matching Kaldi / WeSpeaker acoustic frontends.
const PREEMPH_COEFF: f32 = 0.97;

/// Lower frequency bound (Hz) of the Kaldi Mel filterbank.
const LOW_FREQ_HZ: f32 = 20.0;

/// Upper frequency bound (Hz) of the Kaldi Mel filterbank (`Nyquist - 400 Hz = 7600 Hz`).
const HIGH_FREQ_HZ: f32 = 7_600.0;

/// Duration of initial WebRTC `autoGainControl` / `noiseSuppression` cold-start ramp (`150 ms = 2,400 samples` @ 16 kHz).
pub const AGC_WARMUP_SAMPLES: usize = (SAMPLE_RATE * 150) / 1_000;

/// Minimum remaining speech samples (`400 ms = 6,400 samples` @ 16 kHz) required to trim [`AGC_WARMUP_SAMPLES`] outright.
const MIN_POST_WARMUP_SAMPLES: usize = (SAMPLE_RATE * 400) / 1_000;

/// Number of initial 10 ms Fbank frames (`15 frames = 150 ms`) skipped when computing the CMN mean on short cold-start turns.
const AGC_WARMUP_FRAMES: usize = 15;

/// Converts frequency in Hz to HTK/Kaldi Mel scale (`1127 * ln(1 + f / 700)`).
#[inline]
fn hz_to_mel(freq_hz: f32) -> f32 {
    1_127.0 * (1.0 + freq_hz / 700.0).ln()
}

/// Sparse representation of a single triangular Mel filterbank bin over positive FFT bins.
struct MelFilter {
    start_bin: usize,
    weights: Vec<f32>,
}

/// Pre-allocated Kaldi-compatible 80-bin log-Mel filterbank extractor (`16 kHz`)
/// with column-wise Cepstral Mean Normalization (`SubtractGlobalMean`).
pub struct FbankExtractor {
    fft: Arc<dyn Fft<f32>>,
    povey_window: [f32; FRAME_LEN],
    mel_filters: Vec<MelFilter>,
    fft_buf: [Complex32; FFT_LEN],
    power_spectrum: [f32; NUM_FFT_BINS],
}

/// Extracted `[num_frames, 80]` row-major `f32` feature matrix ready for ONNX speaker embedding inference.
#[derive(Debug, Clone)]
pub struct FbankFeatures {
    /// Number of time frames (`T`).
    pub num_frames: usize,
    /// Feature dimension (`80`).
    pub feat_dim: usize,
    /// Flattened row-major tensor data of length `num_frames * 80`.
    pub data: Vec<f32>,
}

impl FbankExtractor {
    /// Initializes precomputed Povey window weights, 80 triangular Mel filters, and a 512-point FFT plan.
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_LEN);

        // Kaldi Povey window: (0.5 - 0.5 * cos(2*pi*n / (N-1)))^0.85
        let mut povey_window = [0.0_f32; FRAME_LEN];
        let denom = (FRAME_LEN - 1) as f32;
        for (n, w) in povey_window.iter_mut().enumerate() {
            let hann = 0.5 - 0.5 * (2.0 * PI * (n as f32) / denom).cos();
            *w = hann.max(0.0).powf(0.85);
        }

        let mel_low = hz_to_mel(LOW_FREQ_HZ);
        let mel_high = hz_to_mel(HIGH_FREQ_HZ);
        let mel_delta = (mel_high - mel_low) / ((NUM_MEL_BINS + 1) as f32);
        let fft_bin_width = (SAMPLE_RATE as f32) / (FFT_LEN as f32);

        let mut mel_filters = Vec::with_capacity(NUM_MEL_BINS);
        for m in 0..NUM_MEL_BINS {
            let left_mel = mel_low + (m as f32) * mel_delta;
            let center_mel = mel_low + ((m + 1) as f32) * mel_delta;
            let right_mel = mel_low + ((m + 2) as f32) * mel_delta;

            let mut start_bin = None;
            let mut weights = Vec::new();

            for bin in 0..(FFT_LEN / 2) {
                let freq = (bin as f32) * fft_bin_width;
                let mel = hz_to_mel(freq);
                if mel > left_mel && mel < right_mel {
                    let weight = if mel <= center_mel {
                        (mel - left_mel) / (center_mel - left_mel)
                    } else {
                        (right_mel - mel) / (right_mel - center_mel)
                    };
                    if weight > 0.0 {
                        if start_bin.is_none() {
                            start_bin = Some(bin);
                        }
                        weights.push(weight);
                    }
                } else if start_bin.is_some() {
                    break;
                }
            }

            mel_filters.push(MelFilter {
                start_bin: start_bin.unwrap_or(0),
                weights,
            });
        }

        Self {
            fft,
            povey_window,
            mel_filters,
            fft_buf: [Complex32::new(0.0, 0.0); FFT_LEN],
            power_spectrum: [0.0; NUM_FFT_BINS],
        }
    }

    /// Computes mean-normalized 80-dimensional log-Mel filterbank features `[num_frames, 80]`
    /// from 16 kHz mono PCM `samples` (`[-1.0, 1.0]`).
    ///
    /// Returns `None` if `samples.len() < FRAME_LEN` (`400` samples = `25 ms`).
    pub fn extract(&mut self, samples: &[f32]) -> Option<FbankFeatures> {
        self.extract_with_warmup_guard(samples, false)
    }

    /// Computes mean-normalized 80-dimensional log-Mel filterbank features `[num_frames, 80]`
    /// from 16 kHz mono PCM `samples`, optionally applying a `150 ms` WebRTC AGC cold-start guard
    /// on the first speech segment after microphone stream initialization.
    ///
    /// When `apply_agc_warmup_guard` is `true`:
    /// - If `samples.len() >= 8,800` (`550 ms`), trims the leading `150 ms` (`2,400` samples) so
    ///   neither Cepstral Mean Normalization nor neural pooling is biased by initial AGC gain adaptation.
    /// - If `400 <= samples.len() < 8,800`, retains all frames for temporal pooling stability while
    ///   excluding the leading `min(15, num_frames / 3)` frames from the column-wise CMN mean calculation.
    pub fn extract_with_warmup_guard(
        &mut self,
        samples: &[f32],
        apply_agc_warmup_guard: bool,
    ) -> Option<FbankFeatures> {
        let effective_samples = if apply_agc_warmup_guard
            && samples.len() >= AGC_WARMUP_SAMPLES + MIN_POST_WARMUP_SAMPLES
        {
            &samples[AGC_WARMUP_SAMPLES..]
        } else {
            samples
        };

        if effective_samples.len() < FRAME_LEN {
            return None;
        }

        let num_frames = 1 + (effective_samples.len() - FRAME_LEN) / FRAME_SHIFT;
        let mut features = vec![0.0_f32; num_frames * NUM_MEL_BINS];
        let mut frame_buf = [0.0_f32; FRAME_LEN];

        for frame_idx in 0..num_frames {
            let offset = frame_idx * FRAME_SHIFT;

            // 1. Scale normalized [-1, 1] floats to 16-bit PCM range (Kaldi / sherpa-onnx convention)
            //    and compute frame DC offset.
            let mut mean = 0.0_f32;
            for (i, dst) in frame_buf.iter_mut().enumerate() {
                let scaled = effective_samples[offset + i] * 32_768.0;
                *dst = scaled;
                mean += scaled;
            }
            mean /= FRAME_LEN as f32;
            for dst in &mut frame_buf {
                *dst -= mean;
            }

            // 2. Apply pre-emphasis (in reverse so frame_buf[i-1] is un-modified) and Povey window
            for i in (1..FRAME_LEN).rev() {
                frame_buf[i] -= PREEMPH_COEFF * frame_buf[i - 1];
            }
            frame_buf[0] *= 1.0 - PREEMPH_COEFF;

            self.fft_buf.fill(Complex32::new(0.0, 0.0));
            for (i, &sample) in frame_buf.iter().enumerate() {
                self.fft_buf[i] = Complex32::new(sample * self.povey_window[i], 0.0);
            }

            // 3. 512-point forward FFT & power spectrum |X[k]|^2
            self.fft.process(&mut self.fft_buf);
            for k in 0..NUM_FFT_BINS {
                let c = self.fft_buf[k];
                self.power_spectrum[k] = c.re * c.re + c.im * c.im;
            }

            // 4. Project power spectrum onto 80 triangular Mel bins and take natural logarithm
            let row_offset = frame_idx * NUM_MEL_BINS;
            for (m, filter) in self.mel_filters.iter().enumerate() {
                let mut mel_energy = 0.0_f32;
                for (w_idx, &w) in filter.weights.iter().enumerate() {
                    mel_energy += w * self.power_spectrum[filter.start_bin + w_idx];
                }
                features[row_offset + m] = mel_energy.max(1e-10).ln();
            }
        }

        // 5. Column-wise global mean subtraction across time frames (SubtractGlobalMean).
        //    If the short-utterance AGC warm-up guard is active and samples weren't trimmed outright,
        //    compute the column mean starting after the initial ramp frames.
        let cmn_start_frame =
            if apply_agc_warmup_guard && effective_samples.len() == samples.len() && num_frames > 3
            {
                AGC_WARMUP_FRAMES.min(num_frames / 3)
            } else {
                0
            };
        let cmn_count = (num_frames - cmn_start_frame).max(1) as f64;

        for m in 0..NUM_MEL_BINS {
            let mut col_sum = 0.0_f64;
            for t in cmn_start_frame..num_frames {
                col_sum += features[t * NUM_MEL_BINS + m] as f64;
            }
            let col_mean = (col_sum / cmn_count) as f32;
            for t in 0..num_frames {
                features[t * NUM_MEL_BINS + m] -= col_mean;
            }
        }

        Some(FbankFeatures {
            num_frames,
            feat_dim: NUM_MEL_BINS,
            data: features,
        })
    }
}

impl Default for FbankExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies output dimensions, finite values, zero column mean, and amplitude scale invariance.
    #[test]
    fn test_fbank_dimensions_cmn_and_scale_invariance() {
        let mut extractor = FbankExtractor::new();

        // Too short (< 400 samples) -> None
        assert!(extractor.extract(&[0.0; 399]).is_none());

        // 1 second (16,000 samples) of a two-tone harmonic signal (220 Hz + 880 Hz)
        let signal_a: Vec<f32> = (0..16_000)
            .map(|n| {
                let t = n as f32 / SAMPLE_RATE as f32;
                0.3 * (2.0 * PI * 220.0 * t).sin() + 0.15 * (2.0 * PI * 880.0 * t).sin()
            })
            .collect();
        // Same signal attenuated by 0.25x
        let signal_b: Vec<f32> = signal_a.iter().map(|&x| x * 0.25).collect();

        let fb_a = extractor.extract(&signal_a).expect("Expected fbank A");
        let fb_b = extractor.extract(&signal_b).expect("Expected fbank B");

        // 1 + (16000 - 400) / 160 = 98 frames
        assert_eq!(fb_a.num_frames, 98);
        assert_eq!(fb_a.feat_dim, 80);
        assert_eq!(fb_a.data.len(), 98 * 80);
        assert!(fb_a.data.iter().all(|v| v.is_finite()));

        // Verify column-wise mean is ~0.0 due to SubtractGlobalMean
        for col in 0..NUM_MEL_BINS {
            let mean: f32 = (0..fb_a.num_frames)
                .map(|t| fb_a.data[t * NUM_MEL_BINS + col])
                .sum::<f32>()
                / (fb_a.num_frames as f32);
            assert!(
                mean.abs() < 1e-4,
                "Expected zero mean for bin {col}, got {mean}"
            );
        }

        // Verify scale invariance: multiplying waveform by 0.25 leaves CMN log-Mel features unchanged
        for (a, b) in fb_a.data.iter().zip(fb_b.data.iter()) {
            assert!(
                (a - b).abs() < 1e-3,
                "Expected scale-invariant features, got {a} vs {b}"
            );
        }
    }

    /// Verifies that `extract_with_warmup_guard` trims the leading 150 ms AGC ramp-up on segments
    /// `>= 550 ms` and excludes initial ramp frames from the CMN mean on short `400..550 ms` segments.
    #[test]
    fn test_fbank_agc_warmup_guard_removes_initial_ramp_bias() {
        let mut extractor = FbankExtractor::new();

        // 1. Segment >= 550 ms (1.0 s = 16,000 samples): first 150 ms (2,400 samples) corrupted by noise/ramp,
        //    followed by 850 ms (13,600 samples) of clean harmonic speech.
        let clean_tail: Vec<f32> = (0..13_600)
            .map(|n| {
                let t = n as f32 / SAMPLE_RATE as f32;
                0.3 * (2.0 * PI * 240.0 * t).sin() + 0.1 * (2.0 * PI * 720.0 * t).sin()
            })
            .collect();
        let mut ramped_1s = vec![0.0_f32; 2_400];
        for (i, s) in ramped_1s.iter_mut().enumerate() {
            let t = i as f32 / SAMPLE_RATE as f32;
            // Distorted high-frequency buzz during AGC cold start
            *s = 0.95 * (2.0 * PI * 3_500.0 * t).sin();
        }
        ramped_1s.extend_from_slice(&clean_tail);

        let fb_guarded = extractor
            .extract_with_warmup_guard(&ramped_1s, true)
            .expect("Expected guarded fbank");
        let fb_clean_ref = extractor
            .extract(&clean_tail)
            .expect("Expected clean reference fbank");

        assert_eq!(fb_guarded.num_frames, fb_clean_ref.num_frames);
        for (g, r) in fb_guarded.data.iter().zip(fb_clean_ref.data.iter()) {
            assert!(
                (g - r).abs() < 1e-5,
                "Guarded >= 550ms extraction should match clean post-warmup audio"
            );
        }

        // 2. Short segment (450 ms = 7,200 samples < 8,800): keeps all frames while computing CMN
        //    mean over steady post-ramp frames (t >= min(15, num_frames / 3)).
        let short_samples: Vec<f32> = (0..7_200)
            .map(|n| {
                let t = n as f32 / SAMPLE_RATE as f32;
                let gain = if n < 1_600 { 0.02 } else { 0.40 };
                gain * (2.0 * PI * 200.0 * t).sin()
            })
            .collect();
        let fb_short = extractor
            .extract_with_warmup_guard(&short_samples, true)
            .expect("Expected short guarded fbank");
        assert_eq!(fb_short.num_frames, 1 + (7_200 - FRAME_LEN) / FRAME_SHIFT);
        let skip = AGC_WARMUP_FRAMES.min(fb_short.num_frames / 3);
        for col in 0..NUM_MEL_BINS {
            let steady_mean: f32 = (skip..fb_short.num_frames)
                .map(|t| fb_short.data[t * NUM_MEL_BINS + col])
                .sum::<f32>()
                / ((fb_short.num_frames - skip) as f32);
            assert!(
                steady_mean.abs() < 1e-4,
                "Post-warmup frames should have zero mean for bin {col}, got {steady_mean}"
            );
        }
    }
}
