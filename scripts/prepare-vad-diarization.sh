#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

WASM_BINDGEN_VERSION="0.2.130"
ORT_VERSION="1.31.1"
MODELS_DIR="$ROOT_DIR/models/vad-diarization"
ORT_DIR="$ROOT_DIR/static/ort"
WASM_OUT_DIR="$ROOT_DIR/static/wasm"
CACHE_BIN_DIR="$ROOT_DIR/.cache/bin"

mkdir -p "$MODELS_DIR" "$ORT_DIR" "$WASM_OUT_DIR" "$CACHE_BIN_DIR"

# 1. Download ONNX models:
#    - Silero VAD v5 (~2.2 MB)
#    - pyannote/segmentation-3.0 (~5.8 MB)
#    - WeSpeaker SimAM-ResNet100 (VoxBlink2 + VoxCeleb2 LM) INT8 (~50.3 MB)
if [ ! -s "$MODELS_DIR/silero_vad_v5.onnx" ]; then
  echo "Downloading silero_vad_v5.onnx..."
  curl -fL "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad_v5.onnx" \
    -o "$MODELS_DIR/silero_vad_v5.onnx.tmp"
  mv "$MODELS_DIR/silero_vad_v5.onnx.tmp" "$MODELS_DIR/silero_vad_v5.onnx"
fi

if [ ! -s "$MODELS_DIR/pyannote_segmentation_3_0.onnx" ]; then
  echo "Downloading pyannote/segmentation-3.0..."
  TMP_SEG_TBZ="$(mktemp -t pyannote-seg.XXXXXX.tar.bz2)"
  TMP_SEG_DIR="$(mktemp -d -t pyannote-seg-dir.XXXXXX)"
  curl -fL "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2" \
    -o "$TMP_SEG_TBZ"
  tar -xjf "$TMP_SEG_TBZ" -C "$TMP_SEG_DIR"
  cp "$TMP_SEG_DIR/sherpa-onnx-pyannote-segmentation-3-0/model.onnx" "$MODELS_DIR/pyannote_segmentation_3_0.onnx.tmp"
  mv "$MODELS_DIR/pyannote_segmentation_3_0.onnx.tmp" "$MODELS_DIR/pyannote_segmentation_3_0.onnx"
  rm -rf "$TMP_SEG_TBZ" "$TMP_SEG_DIR"
fi

if [ ! -s "$MODELS_DIR/wespeaker_voxblink2_samresnet100_LM.int8.onnx" ]; then
  FP32_CACHE="$MODELS_DIR/.voxblink2_samresnet100_ft.fp32.onnx"
  if [ ! -s "$FP32_CACHE" ]; then
    echo "Downloading WeSpeaker voxblink2_samresnet100_ft FP32 ONNX for INT8 quantization..."
    curl -fL "https://modelscope.cn/datasets/wenet/wespeaker_pretrained_models/resolve/master/voxblink2_samresnet100_ft.onnx" \
      -o "${FP32_CACHE}.tmp"
    mv "${FP32_CACHE}.tmp" "$FP32_CACHE"
  fi
  uv run --python 3.11 --with onnx --with onnxruntime --with numpy \
    python3 "$ROOT_DIR/scripts/quantize_voxblink2_int8.py" \
    "$FP32_CACHE" \
    "$MODELS_DIR/wespeaker_voxblink2_samresnet100_LM.int8.onnx.tmp"
  mv "$MODELS_DIR/wespeaker_voxblink2_samresnet100_LM.int8.onnx.tmp" \
    "$MODELS_DIR/wespeaker_voxblink2_samresnet100_LM.int8.onnx"
  rm -f "$FP32_CACHE"
fi

if [ ! -s "$MODELS_DIR/wespeaker_en_voxceleb_resnet152_LM.onnx" ]; then
  echo "Downloading wespeaker_en_voxceleb_resnet152_LM.onnx..."
  curl -fL "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet152_LM.onnx" \
    -o "$MODELS_DIR/wespeaker_en_voxceleb_resnet152_LM.onnx.tmp"
  mv "$MODELS_DIR/wespeaker_en_voxceleb_resnet152_LM.onnx.tmp" "$MODELS_DIR/wespeaker_en_voxceleb_resnet152_LM.onnx"
fi

# 2. Download and extract self-hosted onnxruntime-web runtime files
if [ ! -s "$ORT_DIR/ort.webgpu.min.mjs" ] || [ ! -s "$ORT_DIR/ort.wasm.min.mjs" ]; then
  echo "Downloading onnxruntime-web@${ORT_VERSION}..."
  TMP_TGZ="$(mktemp -t ort-web.XXXXXX.tgz)"
  TMP_EXTRACT="$(mktemp -d -t ort-extract.XXXXXX)"
  curl -fsSL "https://registry.npmjs.org/onnxruntime-web/-/onnxruntime-web-${ORT_VERSION}.tgz" -o "$TMP_TGZ"
  tar -xzf "$TMP_TGZ" -C "$TMP_EXTRACT"
  cp "$TMP_EXTRACT"/package/dist/ort.webgpu.min.mjs "$ORT_DIR/"
  cp "$TMP_EXTRACT"/package/dist/ort.wasm.min.mjs "$ORT_DIR/"
  for f in "$TMP_EXTRACT"/package/dist/ort-wasm-simd-threaded*.mjs "$TMP_EXTRACT"/package/dist/ort-wasm-simd-threaded*.wasm; do
    if [ -f "$f" ]; then
      cp "$f" "$ORT_DIR/"
    fi
  done
  rm -rf "$TMP_TGZ" "$TMP_EXTRACT"
fi

# 3. Ensure wasm32-unknown-unknown Rust target is installed
if ! rustup target list --installed | grep -q "^wasm32-unknown-unknown$"; then
  echo "Installing Rust target wasm32-unknown-unknown..."
  rustup target add wasm32-unknown-unknown
fi

# 4. Ensure matching wasm-bindgen CLI binary (0.2.130) is cached locally
WASM_BINDGEN_BIN="$CACHE_BIN_DIR/wasm-bindgen-${WASM_BINDGEN_VERSION}"
if [ ! -x "$WASM_BINDGEN_BIN" ]; then
  OS_NAME="$(uname -s)"
  ARCH_NAME="$(uname -m)"
  case "${OS_NAME}-${ARCH_NAME}" in
    Darwin-arm64|Darwin-aarch64) TARGET_TRIPLE="aarch64-apple-darwin" ;;
    Darwin-x86_64)               TARGET_TRIPLE="x86_64-apple-darwin" ;;
    Linux-aarch64|Linux-arm64)   TARGET_TRIPLE="aarch64-unknown-linux-musl" ;;
    Linux-x86_64)                TARGET_TRIPLE="x86_64-unknown-linux-musl" ;;
    *)
      echo "Unsupported host platform ${OS_NAME}-${ARCH_NAME} for prebuilt wasm-bindgen" >&2
      exit 1
      ;;
  esac

  echo "Downloading wasm-bindgen ${WASM_BINDGEN_VERSION} (${TARGET_TRIPLE})..."
  TMP_WB_TGZ="$(mktemp -t wasm-bindgen.XXXXXX.tar.gz)"
  TMP_WB_DIR="$(mktemp -d -t wasm-bindgen-dir.XXXXXX)"
  curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-${TARGET_TRIPLE}.tar.gz" \
    -o "$TMP_WB_TGZ"
  tar -xzf "$TMP_WB_TGZ" -C "$TMP_WB_DIR"
  cp "$TMP_WB_DIR/wasm-bindgen-${WASM_BINDGEN_VERSION}-${TARGET_TRIPLE}/wasm-bindgen" "$WASM_BINDGEN_BIN"
  chmod +x "$WASM_BINDGEN_BIN"
  rm -rf "$TMP_WB_TGZ" "$TMP_WB_DIR"
fi

# 5. Compile crates/vad-diarization-wasm and generate browser ES module bindings
echo "Building vad-diarization-wasm (release)..."
cargo build --release -p vad-diarization-wasm --target wasm32-unknown-unknown
"$WASM_BINDGEN_BIN" \
  --target web \
  --no-typescript \
  --out-dir "$WASM_OUT_DIR" \
  "$ROOT_DIR/target/wasm32-unknown-unknown/release/vad_diarization_wasm.wasm"

echo "VAD + Diarization WASM & ONNX assets ready in static/wasm, static/ort, and models/vad-diarization."
