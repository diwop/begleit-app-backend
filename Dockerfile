# Stage 1: Download static intfloat/multilingual-e5-base model weights in an independent cached stage
FROM debian:bookworm-slim AS models

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /models/multilingual-e5-base \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/config.json -o /models/multilingual-e5-base/config.json \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/tokenizer.json -o /models/multilingual-e5-base/tokenizer.json \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/model.safetensors -o /models/multilingual-e5-base/model.safetensors

# Stage 2: Pre-build and cache all external Rust dependencies (invalidates only when Cargo.toml or Cargo.lock changes)
FROM rust:bookworm AS deps

WORKDIR /usr/src/app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "pub fn dummy() {}" > src/lib.rs \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/diwop-begleitapp* target/release/deps/diwop_begleitapp* target/release/deps/libdiwop_begleitapp* target/release/.fingerprint/diwop-begleitapp-*

# Stage 3: Precompute dynamic few-shot buckets and embeddings (cached unless raw dialogs, few-shot template, or few_shots.rs change)
FROM deps AS indexer

COPY assets/gemma-4-tokenizer.json.gz ./assets/gemma-4-tokenizer.json.gz
COPY prompts/translate-dynamic-few-shots-sample.md ./prompts/translate-dynamic-few-shots-sample.md
COPY src/few_shots.rs ./src/few_shots.rs
RUN printf 'pub mod few_shots;\n' > src/lib.rs \
    && printf '#[tokio::main]\nasync fn main() {\n    tracing_subscriber::fmt().init();\n    diwop_begleitapp::few_shots::run_prepare_few_shots_cli().await.expect("Failed to prepare few-shots");\n}\n' > src/main.rs \
    && cargo build --release

COPY --from=models /models/multilingual-e5-base ./models/multilingual-e5-base
COPY data ./data
RUN ./target/release/diwop-begleitapp

# Stage 4: Build the main application binary against cached Rust dependencies
FROM deps AS builder

COPY src ./src
COPY prompts ./prompts
COPY assets ./assets
COPY static ./static
COPY templates ./templates
RUN cargo build --release

# Stage 5: Minimal hardened production runtime image
FROM debian:bookworm-slim

# Install necessary runtime dependencies and create an unprivileged non-root user
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 appuser \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin appuser

WORKDIR /app

# Order layers from least-frequently changing (static 1.12 GB model weights and few-shot index)
# to most-frequently changing (application binary) to maximize Docker layer cache and ECR push reuse
COPY --from=models /models/multilingual-e5-base /app/models/multilingual-e5-base
COPY --from=indexer /usr/src/app/data/few_shots_index.json.gz /app/data/few_shots_index.json.gz
COPY --from=builder /usr/src/app/target/release/diwop-begleitapp /app/diwop-begleitapp

USER 10001:10001

# Default port
ENV PORT=3000
EXPOSE $PORT

CMD ["/app/diwop-begleitapp"]
