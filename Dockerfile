# Stage 1: Download static ML model weights and self-hosted ONNX Runtime Web assets in an independent cached stage
FROM debian:bookworm-slim AS models

ARG ORT_VERSION=1.31.1

COPY scripts/quantize_voxblink2_int8.py /tmp/quantize_voxblink2_int8.py
RUN apt-get update && apt-get install -y --no-install-recommends bzip2 ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR="/usr/local/bin" sh \
    && mkdir -p /models/multilingual-e5-base /models/vad-diarization /static/ort /tmp/ort-extract /tmp/pyannote-extract \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/config.json -o /models/multilingual-e5-base/config.json \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/tokenizer.json -o /models/multilingual-e5-base/tokenizer.json \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/model.safetensors -o /models/multilingual-e5-base/model.safetensors \
    && curl -fsSL https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad_v5.onnx -o /models/vad-diarization/silero_vad_v5.onnx \
    && curl -fsSL https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2 -o /tmp/pyannote.tar.bz2 \
    && tar -xjf /tmp/pyannote.tar.bz2 -C /tmp/pyannote-extract \
    && cp /tmp/pyannote-extract/sherpa-onnx-pyannote-segmentation-3-0/model.onnx /models/vad-diarization/pyannote_segmentation_3_0.onnx \
    && curl -fsSL "https://modelscope.cn/datasets/wenet/wespeaker_pretrained_models/resolve/master/voxblink2_samresnet100_ft.onnx" -o /tmp/voxblink2_fp32.onnx \
    && /usr/local/bin/uv run --python 3.11 --with onnx --with onnxruntime --with numpy python3 /tmp/quantize_voxblink2_int8.py /tmp/voxblink2_fp32.onnx /models/vad-diarization/wespeaker_voxblink2_samresnet100_LM.int8.onnx \
    && curl -fsSL https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet152_LM.onnx -o /models/vad-diarization/wespeaker_en_voxceleb_resnet152_LM.onnx \
    && curl -fsSL "https://registry.npmjs.org/onnxruntime-web/-/onnxruntime-web-${ORT_VERSION}.tgz" -o /tmp/ort.tgz \
    && tar -xzf /tmp/ort.tgz -C /tmp/ort-extract \
    && cp /tmp/ort-extract/package/dist/ort.webgpu.min.mjs /static/ort/ \
    && cp /tmp/ort-extract/package/dist/ort.wasm.min.mjs /static/ort/ \
    && cp /tmp/ort-extract/package/dist/ort-wasm-simd-threaded*.mjs /static/ort/ \
    && cp /tmp/ort-extract/package/dist/ort-wasm-simd-threaded*.wasm /static/ort/ \
    && rm -rf /tmp/ort.tgz /tmp/ort-extract /tmp/pyannote.tar.bz2 /tmp/pyannote-extract /tmp/voxblink2_fp32.onnx /tmp/quantize_voxblink2_int8.py /root/.cache /root/.local

# Stage 2: Pre-build and cache external Rust dependencies
FROM rust:bookworm AS deps

WORKDIR /usr/src/app

COPY Cargo.toml Cargo.lock ./
COPY crates/vad-diarization-wasm/Cargo.toml ./crates/vad-diarization-wasm/Cargo.toml
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target,target=/usr/src/app/target,sharing=locked \
    mkdir -p src crates/vad-diarization-wasm/src \
    && echo "pub fn dummy() {}" > src/lib.rs \
    && echo "fn main() {}" > src/main.rs \
    && echo "pub fn dummy() {}" > crates/vad-diarization-wasm/src/lib.rs \
    && cargo build --release -p diwop-begleitapp \
    && rm -rf src crates/vad-diarization-wasm/src target/release/diwop-begleitapp* target/release/deps/diwop_begleitapp* target/release/deps/libdiwop_begleitapp* target/release/.fingerprint/diwop-begleitapp-*

# Stage 3: Precompute dynamic few-shot buckets and embeddings (cached unless raw dialogs, few-shot template, or few_shots.rs change)
FROM deps AS indexer

COPY assets/gemma-4-tokenizer.json.gz ./assets/gemma-4-tokenizer.json.gz
COPY prompts/translate-dynamic-few-shots-sample.md ./prompts/translate-dynamic-few-shots-sample.md
COPY src/few_shots.rs ./src/few_shots.rs
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target,target=/usr/src/app/target,sharing=locked \
    mkdir -p crates/vad-diarization-wasm/src \
    && echo "pub fn dummy() {}" > crates/vad-diarization-wasm/src/lib.rs \
    && printf 'pub mod few_shots;\n' > src/lib.rs \
    && printf '#[tokio::main]\nasync fn main() {\n    tracing_subscriber::fmt().init();\n    diwop_begleitapp::few_shots::run_prepare_few_shots_cli().await.expect("Failed to prepare few-shots");\n}\n' > src/main.rs \
    && cargo build --release -p diwop-begleitapp \
    && cp target/release/diwop-begleitapp /usr/local/bin/prepare-few-shots \
    && rm -rf target/release/diwop-begleitapp* target/release/deps/diwop_begleitapp* target/release/deps/libdiwop_begleitapp* target/release/.fingerprint/diwop-begleitapp-*

COPY --from=models /models/multilingual-e5-base ./models/multilingual-e5-base
COPY data ./data
RUN /usr/local/bin/prepare-few-shots

# Stage 4: Compile crates/vad-diarization-wasm to WebAssembly (supports both linux/arm64 and linux/amd64 builders)
FROM rust:bookworm AS wasm-builder

ARG WASM_BINDGEN_VERSION=0.2.130
WORKDIR /usr/src/app

RUN ARCH="$(uname -m)" \
    && case "$ARCH" in \
         aarch64|arm64) WB_TRIPLE="aarch64-unknown-linux-musl" ;; \
         x86_64)        WB_TRIPLE="x86_64-unknown-linux-musl" ;; \
         *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;; \
       esac \
    && rustup target add wasm32-unknown-unknown \
    && curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-${WB_TRIPLE}.tar.gz" -o /tmp/wb.tar.gz \
    && tar -xzf /tmp/wb.tar.gz -C /tmp \
    && mv "/tmp/wasm-bindgen-${WASM_BINDGEN_VERSION}-${WB_TRIPLE}/wasm-bindgen" /usr/local/bin/wasm-bindgen \
    && chmod +x /usr/local/bin/wasm-bindgen \
    && rm -rf /tmp/wb.tar.gz "/tmp/wasm-bindgen-${WASM_BINDGEN_VERSION}-${WB_TRIPLE}"

COPY Cargo.toml Cargo.lock ./
COPY crates/vad-diarization-wasm ./crates/vad-diarization-wasm
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-wasm-target,target=/usr/src/app/target,sharing=locked \
    mkdir -p src /wasm-pkg \
    && echo "pub fn dummy() {}" > src/lib.rs \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release -p vad-diarization-wasm --target wasm32-unknown-unknown \
    && wasm-bindgen --target web --no-typescript --out-dir /wasm-pkg target/wasm32-unknown-unknown/release/vad_diarization_wasm.wasm

# Stage 5: Build the main application binary against cached Rust dependencies
FROM deps AS builder

COPY crates ./crates
COPY src ./src
COPY prompts ./prompts
COPY assets ./assets
COPY static ./static
COPY templates ./templates
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target,target=/usr/src/app/target,sharing=locked \
    cargo build --release -p diwop-begleitapp \
    && cp target/release/diwop-begleitapp /usr/local/bin/diwop-begleitapp

# Stage 6: Minimal hardened production runtime image
FROM debian:bookworm-slim

# Install necessary runtime dependencies and create an unprivileged non-root user
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 appuser \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin appuser

WORKDIR /app

# Order layers from least-frequently changing (static model weights, ORT runtime, and few-shot index)
# to most-frequently changing (WASM bundle and application binary) to maximize Docker layer cache reuse
COPY --from=models /models/multilingual-e5-base /app/models/multilingual-e5-base
COPY --from=models /models/vad-diarization /app/models/vad-diarization
COPY --from=models /static/ort /app/static/ort
COPY --from=indexer /usr/src/app/data/few_shots_index.json.gz /app/data/few_shots_index.json.gz
COPY --from=wasm-builder /wasm-pkg /app/static/wasm
COPY --from=builder /usr/local/bin/diwop-begleitapp /app/diwop-begleitapp

USER 10001:10001

# Default port
ENV PORT=3000
EXPOSE $PORT

CMD ["/app/diwop-begleitapp"]
