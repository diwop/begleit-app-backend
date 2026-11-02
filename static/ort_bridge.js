/**
 * ONNX Runtime Web bridge invoked by the Rust/WASM `DiarizationEngine` via `globalThis.__diwopOrtBridge`.
 *
 * Manages three ONNX sessions:
 * 1. Silero VAD v5 (`WASM SIMD`)
 * 2. `pyannote/segmentation-3.0` (`WASM SIMD`, 10s / 160,000-sample powerset window)
 * 3. Speaker Embedding (`WeSpeaker SimAM-ResNet100` INT8 QDQ on `WASM SIMD` or FP32 on `WebGPU`/`WASM SIMD`)
 */

let ortModulePromise = null;
const sessionRegistry = new Map();
let nextHandleId = 1;

const PYANNOTE_WINDOW_SAMPLES = 160000;
const PYANNOTE_MAX_FRAMES = 589;
const PYANNOTE_NUM_CLASSES = 7;

async function loadOrt() {
  if (!ortModulePromise) {
    ortModulePromise = (async () => {
      let mod;
      try {
        mod = await import('/static/ort/ort.webgpu.min.mjs');
      } catch (_err) {
        mod = await import('/static/ort/ort.wasm.min.mjs');
      }
      const ort = mod.default || mod;
      ort.env.wasm.wasmPaths = '/static/ort/';
      ort.env.wasm.simd = true;
      ort.env.wasm.numThreads = globalThis.crossOriginIsolated
        ? Math.min(4, Math.max(1, navigator.hardwareConcurrency || 2))
        : 1;
      ort.env.logLevel = 'error';
      return ort;
    })();
  }
  return ortModulePromise;
}

/**
 * Heuristic check for whether an ONNX model binary contains QDQ / QLinear activation-quantized
 * operators (`QuantizeLinear` or `QLinearConv`).
 *
 * Note: Weight-only INT8 models (`DequantizeLinear` -> FP32 `Conv`/`Gemm`) ARE supported by
 * WebGPU and should try `['webgpu', 'wasm']` first.
 */
function isActivationQuantizedInt8Model(modelBytes) {
  try {
    const ascii = new TextDecoder('latin1').decode(
      modelBytes.subarray(0, Math.min(modelBytes.length, 65536)),
    );
    return ascii.includes('QuantizeLinear') || ascii.includes('QLinearConv');
  } catch (_err) {
    return false;
  }
}

async function hasUsableWebGpu() {
  if (typeof navigator === 'undefined' || !navigator.gpu) {
    return false;
  }
  try {
    const adapter = await navigator.gpu.requestAdapter();
    return Boolean(adapter);
  } catch (_err) {
    return false;
  }
}

async function ort_init_sessions(vadModelBytes, segModelBytes, embedModelBytes) {
  const ort = await loadOrt();

  // Copy slices into standalone Uint8Arrays only during InferenceSession creation so WASM memory
  // views aren't invalidated if WASM linear memory grows during async initialization.
  const vadCopy = new Uint8Array(vadModelBytes);
  const segCopy = new Uint8Array(segModelBytes);
  const embedCopy = new Uint8Array(embedModelBytes);

  // Silero VAD processes tiny 576-sample frames every 32ms and uses an LSTM op; WASM SIMD runs it
  // in ~0.15ms without GPU buffer round-trip overhead.
  const vadSession = await ort.InferenceSession.create(vadCopy, {
    executionProviders: ['wasm'],
    graphOptimizationLevel: 'all',
    logSeverityLevel: 3,
  });

  // pyannote/segmentation-3.0 uses SincNet + BiLSTM layers; WASM SIMD runs a 10s window in ~15-25ms.
  let segSession = null;
  if (segCopy.byteLength > 0) {
    segSession = await ort.InferenceSession.create(segCopy, {
      executionProviders: ['wasm'],
      graphOptimizationLevel: 'all',
      logSeverityLevel: 3,
    });
  }

  let embedSession = null;
  let backendName = 'WASM SIMD';
  const activationQuantizedInt8 = isActivationQuantizedInt8Model(embedCopy);

  if (!activationQuantizedInt8 && (await hasUsableWebGpu())) {
    try {
      embedSession = await ort.InferenceSession.create(embedCopy, {
        executionProviders: ['webgpu', 'wasm'],
        graphOptimizationLevel: 'all',
        logSeverityLevel: 3,
      });
      const testFeats = new Float32Array(98 * 80);
      const inputName = embedSession.inputNames[0] || 'feats';
      await embedSession.run({ [inputName]: new ort.Tensor('float32', testFeats, [1, 98, 80]) });
      backendName = 'WebGPU';
    } catch (_gpuErr) {
      console.warn('WebGPU test run failed for speaker embedding, falling back to WASM SIMD:', _gpuErr);
      embedSession = null;
    }
  }

  if (!embedSession) {
    embedSession = await ort.InferenceSession.create(embedCopy, {
      executionProviders: ['wasm'],
      graphOptimizationLevel: 'all',
      logSeverityLevel: 3,
    });
    backendName = 'WASM SIMD';
  }

  const srTensor = new ort.Tensor('int64', BigInt64Array.from([16000n]), [1]);
  const packedVadOut = new Float32Array(1 + 256);
  const segWindowBuffer = new Float32Array(PYANNOTE_WINDOW_SAMPLES);
  const packedSegOut = new Float32Array(1 + PYANNOTE_MAX_FRAMES * PYANNOTE_NUM_CLASSES);

  const id = nextHandleId++;
  sessionRegistry.set(id, {
    ort,
    vadSession,
    segSession,
    embedSession,
    embedCopy,
    srTensor,
    packedVadOut,
    segWindowBuffer,
    packedSegOut,
    backendName,
  });

  return id;
}

async function ort_run_vad(handleId, window576, state256) {
  const ctx = sessionRegistry.get(handleId);
  if (!ctx) {
    throw new Error('Invalid ORT session handle');
  }
  const { ort, vadSession, srTensor, packedVadOut } = ctx;

  // Clone out of WASM linear memory view before awaiting async ORT execution
  const inputArr = new Float32Array(window576);
  const stateArr = new Float32Array(state256);

  const feeds = {
    input: new ort.Tensor('float32', inputArr, [1, inputArr.length]),
    state: new ort.Tensor('float32', stateArr, [2, 1, 128]),
    sr: srTensor,
  };

  const outputs = await vadSession.run(feeds);
  const probName = vadSession.outputNames[0] || 'output';
  const stateOutName = vadSession.outputNames[1] || 'stateN';

  const probData = outputs[probName].data;
  const nextStateData = outputs[stateOutName].data;

  packedVadOut[0] = probData[0];
  packedVadOut.set(nextStateData, 1);
  return packedVadOut;
}

async function ort_run_segmentation(handleId, pcmSamples) {
  const ctx = sessionRegistry.get(handleId);
  if (!ctx || !ctx.segSession) {
    return new Float32Array([0]);
  }
  const { ort, segSession, segWindowBuffer, packedSegOut } = ctx;

  // Copy segment audio into fixed 10.0s (160,000-sample) zero-padded buffer
  segWindowBuffer.fill(0);
  const copyLen = Math.min(pcmSamples.length, PYANNOTE_WINDOW_SAMPLES);
  segWindowBuffer.set(pcmSamples.subarray(0, copyLen), 0);

  const inputName = segSession.inputNames[0] || 'x';
  const outputName = segSession.outputNames[0] || 'y';

  const feeds = {
    [inputName]: new ort.Tensor('float32', new Float32Array(segWindowBuffer), [
      1,
      1,
      PYANNOTE_WINDOW_SAMPLES,
    ]),
  };

  try {
    const outputs = await segSession.run(feeds);
    const outTensor = outputs[outputName];
    const dims = outTensor.dims || [1, PYANNOTE_MAX_FRAMES, PYANNOTE_NUM_CLASSES];
    const numFrames = dims.length >= 2 ? dims[1] : PYANNOTE_MAX_FRAMES;
    const rawData = outTensor.data;
    const neededLen = 1 + numFrames * PYANNOTE_NUM_CLASSES;
    const outBuf =
      neededLen <= packedSegOut.length ? packedSegOut.subarray(0, neededLen) : new Float32Array(neededLen);
    outBuf[0] = numFrames;
    outBuf.set(rawData.subarray(0, numFrames * PYANNOTE_NUM_CLASSES), 1);
    return outBuf;
  } catch (segErr) {
    console.warn('pyannote/segmentation-3.0 inference failed, falling back to unsplit VAD segment:', segErr);
    return new Float32Array([0]);
  }
}

async function ort_run_embedding(handleId, fbankData, numFrames, featDim) {
  const ctx = sessionRegistry.get(handleId);
  if (!ctx) {
    throw new Error('Invalid ORT session handle');
  }
  const { ort } = ctx;

  const featsArr = new Float32Array(fbankData);
  const inputName = ctx.embedSession.inputNames[0] || 'feats';
  const outputName = ctx.embedSession.outputNames[0] || 'embs';

  const feeds = {
    [inputName]: new ort.Tensor('float32', featsArr, [1, numFrames, featDim]),
  };

  let outputs;
  try {
    outputs = await ctx.embedSession.run(feeds);
  } catch (runErr) {
    console.warn(`Embedding inference failed on ${ctx.backendName}, attempting WASM SIMD fallback:`, runErr);
    ctx.embedSession = await ort.InferenceSession.create(ctx.embedCopy, {
      executionProviders: ['wasm'],
      graphOptimizationLevel: 'all',
      logSeverityLevel: 3,
    });
    ctx.backendName = 'WASM SIMD';
    outputs = await ctx.embedSession.run(feeds);
  }

  const rawOut = outputs[outputName];
  if (rawOut.data instanceof Float32Array) {
    return rawOut.data;
  }
  return Float32Array.from(rawOut.data);
}

function ort_get_backend_name(handleId) {
  const ctx = sessionRegistry.get(handleId);
  return ctx ? ctx.backendName : 'WASM SIMD';
}

globalThis.__diwopOrtBridge = {
  ort_init_sessions,
  ort_run_vad,
  ort_run_segmentation,
  ort_run_embedding,
  ort_get_backend_name,
};

export { loadOrt };
