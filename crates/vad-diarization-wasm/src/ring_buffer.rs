/// Sample rate expected across the entire VAD and diarization pipeline (16 kHz mono).
pub const SAMPLE_RATE: usize = 16_000;

/// Hop size (new PCM samples consumed per Silero VAD v5 step at 16 kHz: 32 ms).
pub const VAD_FRAME_SAMPLES: usize = 512;

/// Context overlap prepended to each 512-sample frame for Silero VAD v5 (`64 + 512 = 576`).
pub const VAD_CONTEXT_SAMPLES: usize = 64;

/// Total input tensor width passed to Silero VAD v5 (`[1, 576]`).
pub const VAD_WINDOW_SAMPLES: usize = VAD_CONTEXT_SAMPLES + VAD_FRAME_SAMPLES;

/// Pre-allocated fixed-capacity circular buffer for 16 kHz mono PCM ingestion.
///
/// Manages audio chunking statically in Rust linear memory without per-chunk heap
/// allocations, preventing garbage-collection spikes during real-time mobile recording.
pub struct AudioRingBuffer {
    storage: Vec<f32>,
    capacity: usize,
    read_pos: usize,
    write_pos: usize,
    len: usize,
    total_popped_samples: u64,
}

/// A fixed 512-sample (32 ms @ 16 kHz) audio frame popped from [`AudioRingBuffer`],
/// paired with its exact starting sample index since session start.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Exact 0-based sample offset from the beginning of the recording session.
    pub start_sample: u64,
    /// Contiguous 512 PCM samples in `[-1.0, 1.0]`.
    pub samples: [f32; VAD_FRAME_SAMPLES],
}

impl AudioRingBuffer {
    /// Allocates a new ring buffer with the given fixed sample `capacity`.
    ///
    /// Panics if `capacity < VAD_FRAME_SAMPLES`.
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity >= VAD_FRAME_SAMPLES,
            "Ring buffer capacity ({capacity}) must be at least {VAD_FRAME_SAMPLES} samples"
        );
        Self {
            storage: vec![0.0; capacity],
            capacity,
            read_pos: 0,
            write_pos: 0,
            len: 0,
            total_popped_samples: 0,
        }
    }

    /// Returns the number of unread samples currently queued in the ring buffer.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when no unread samples are buffered.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the total number of samples popped from the buffer since creation or last [`Self::reset`].
    pub fn total_popped_samples(&self) -> u64 {
        self.total_popped_samples
    }

    /// Clears all buffered audio and resets sample counters to zero without reallocating storage.
    pub fn reset(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
        self.len = 0;
        self.total_popped_samples = 0;
    }

    /// Appends incoming 16 kHz mono `samples` into the circular buffer.
    ///
    /// If incoming data exceeds remaining free space, the oldest unread samples are overwritten
    /// and `total_popped_samples` advances accordingly so timestamps remain monotonic and accurate.
    pub fn push_slice(&mut self, samples: &[f32]) {
        for &sample in samples {
            if self.len == self.capacity {
                // Advance read cursor when full so real-time audio never panics or reallocates.
                self.read_pos = (self.read_pos + 1) % self.capacity;
                self.total_popped_samples += 1;
            } else {
                self.len += 1;
            }
            self.storage[self.write_pos] = sample;
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
    }

    /// Pops the next contiguous 512-sample (`32 ms`) frame if at least `512` samples are available.
    pub fn pop_frame(&mut self) -> Option<AudioFrame> {
        if self.len < VAD_FRAME_SAMPLES {
            return None;
        }

        let start_sample = self.total_popped_samples;
        let mut frame = [0.0_f32; VAD_FRAME_SAMPLES];

        let first_chunk = (self.capacity - self.read_pos).min(VAD_FRAME_SAMPLES);
        frame[..first_chunk]
            .copy_from_slice(&self.storage[self.read_pos..self.read_pos + first_chunk]);

        let second_chunk = VAD_FRAME_SAMPLES - first_chunk;
        if second_chunk > 0 {
            frame[first_chunk..].copy_from_slice(&self.storage[..second_chunk]);
        }

        self.read_pos = (self.read_pos + VAD_FRAME_SAMPLES) % self.capacity;
        self.len -= VAD_FRAME_SAMPLES;
        self.total_popped_samples += VAD_FRAME_SAMPLES as u64;

        Some(AudioFrame {
            start_sample,
            samples: frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that uneven push sizes reassemble into exact 512-sample frames across wrap-around boundaries.
    #[test]
    fn test_ring_buffer_chunking_and_wraparound() {
        let mut rb = AudioRingBuffer::with_capacity(1024);
        assert!(rb.is_empty());

        // Push 300 samples -> not enough for a 512-sample frame yet
        let first: Vec<f32> = (0..300).map(|i| i as f32).collect();
        rb.push_slice(&first);
        assert_eq!(rb.len(), 300);
        assert!(rb.pop_frame().is_none());

        // Push 500 more samples (total 800) -> 1 frame of 512 popped, 288 remaining
        let second: Vec<f32> = (300..800).map(|i| i as f32).collect();
        rb.push_slice(&second);
        let f1 = rb.pop_frame().expect("Expected first 512-sample frame");
        assert_eq!(f1.start_sample, 0);
        assert_eq!(f1.samples[0], 0.0);
        assert_eq!(f1.samples[511], 511.0);
        assert_eq!(rb.len(), 288);

        // Push 600 more samples -> wraps around capacity=1024 boundary cleanly
        let third: Vec<f32> = (800..1400).map(|i| i as f32).collect();
        rb.push_slice(&third);
        assert_eq!(rb.len(), 888);

        let f2 = rb.pop_frame().expect("Expected second wrapped frame");
        assert_eq!(f2.start_sample, 512);
        for (idx, &val) in f2.samples.iter().enumerate() {
            assert_eq!(val, (512 + idx) as f32);
        }
        assert_eq!(rb.len(), 376);
    }

    /// Verifies overflow behavior keeps the newest samples and advances monotonic timestamps.
    #[test]
    fn test_ring_buffer_overflow_advances_sample_offset() {
        let mut rb = AudioRingBuffer::with_capacity(600);
        let input: Vec<f32> = (0..700).map(|i| i as f32).collect();
        rb.push_slice(&input);

        // Capacity is 600, so oldest 100 samples (0..100) were dropped
        assert_eq!(rb.len(), 600);
        assert_eq!(rb.total_popped_samples(), 100);

        let frame = rb.pop_frame().expect("Expected frame after overflow");
        assert_eq!(frame.start_sample, 100);
        assert_eq!(frame.samples[0], 100.0);
        assert_eq!(frame.samples[511], 611.0);

        rb.reset();
        assert!(rb.is_empty());
        assert_eq!(rb.total_popped_samples(), 0);
    }
}
