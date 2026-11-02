/**
 * AudioWorkletProcessor that captures native microphone audio, mixes down to mono,
 * downsamples to 16 kHz with box-averaging anti-aliasing, and streams zero-GC chunks
 * via a lock-free SharedArrayBuffer ring buffer (or fixed 512-sample MessagePort frames
 * when SharedArrayBuffer is unavailable).
 */

const TARGET_SAMPLE_RATE = 16000;
const FRAME_SAMPLES = 512;

class VadAudioWorkletProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = (options && options.processorOptions) || {};

    this.useSab = Boolean(opts.sab);
    if (this.useSab) {
      // Header layout in SharedArrayBuffer:
      // Int32[0] = writePos (monotonic sample counter)
      // Int32[1] = readPos  (monotonic sample counter)
      // Float32[2..] = ring storage of length opts.ringCapacity
      this.indices = new Int32Array(opts.sab, 0, 2);
      this.ring = new Float32Array(opts.sab, 8, opts.ringCapacity);
      this.ringCapacity = opts.ringCapacity;
    } else {
      this.fallbackFrame = new Float32Array(FRAME_SAMPLES);
      this.fallbackOffset = 0;
    }

    // Fractional downsampling accumulator state (sampleRate -> 16000 Hz)
    this.ratio = sampleRate / TARGET_SAMPLE_RATE;
    this.srcCursor = 0.0;
    this.accSum = 0.0;
    this.accCount = 0;
  }

  emitSample16k(sample) {
    if (this.useSab) {
      const writeIdx = Atomics.load(this.indices, 0);
      const readIdx = Atomics.load(this.indices, 1);
      if (writeIdx - readIdx < this.ringCapacity) {
        const slot = ((writeIdx % this.ringCapacity) + this.ringCapacity) % this.ringCapacity;
        this.ring[slot] = sample;
        Atomics.store(this.indices, 0, (writeIdx + 1) | 0);
      }
    } else {
      this.fallbackFrame[this.fallbackOffset++] = sample;
      if (this.fallbackOffset >= FRAME_SAMPLES) {
        // Send a compact 512-sample copy only every 32ms (~31 messages/sec)
        this.port.postMessage(this.fallbackFrame.slice());
        this.fallbackOffset = 0;
      }
    }
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) {
      return true;
    }

    const ch0 = input[0];
    const ch1 = input.length > 1 ? input[1] : null;
    const len = ch0.length;

    if (Math.abs(this.ratio - 1.0) < 1e-3) {
      for (let i = 0; i < len; i++) {
        const mono = ch1 ? 0.5 * (ch0[i] + ch1[i]) : ch0[i];
        this.emitSample16k(mono);
      }
      return true;
    }

    // Box-filter averaging downsampler preventing high-frequency aliasing into 0..8 kHz speech band
    for (let i = 0; i < len; i++) {
      const mono = ch1 ? 0.5 * (ch0[i] + ch1[i]) : ch0[i];
      this.accSum += mono;
      this.accCount += 1;
      this.srcCursor += 1.0;

      if (this.srcCursor >= this.ratio) {
        this.srcCursor -= this.ratio;
        const downsampled = this.accCount > 0 ? this.accSum / this.accCount : mono;
        this.emitSample16k(downsampled);
        this.accSum = 0.0;
        this.accCount = 0;
      }
    }

    return true;
  }
}

registerProcessor('vad-audio-worklet', VadAudioWorkletProcessor);
