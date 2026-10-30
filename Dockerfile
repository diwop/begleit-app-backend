# Build stage
FROM rust:bookworm AS builder

WORKDIR /usr/src/app

# Pre-download intfloat/multilingual-e5-base model files in a static cached layer
RUN mkdir -p models/multilingual-e5-base \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/config.json -o models/multilingual-e5-base/config.json \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/tokenizer.json -o models/multilingual-e5-base/tokenizer.json \
    && curl -fsSL https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/model.safetensors -o models/multilingual-e5-base/model.safetensors

# Pre-build and cache Rust dependencies in a dedicated layer that only invalidates when Cargo.toml or Cargo.lock changes
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "pub fn dummy() {}" > src/lib.rs \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/diwop-begleitapp* target/release/deps/diwop_begleitapp* target/release/deps/libdiwop_begleitapp* target/release/.fingerprint/diwop-begleitapp-*

# Copy application source and compile-time embedded assets, then build the application binary
COPY src ./src
COPY prompts ./prompts
COPY assets ./assets
COPY static ./static
COPY templates ./templates
RUN cargo build --release

# Copy DVC raw dialogs and precompute dynamic few-shot buckets and embeddings
COPY data ./data
RUN ./target/release/diwop-begleitapp --prepare-few-shots

# Production stage
FROM debian:bookworm-slim

# Install necessary runtime dependencies and create an unprivileged non-root user
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 appuser \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin appuser

WORKDIR /app

# Copy the build artifact, embedding model weights, and precomputed few-shot index from the builder stage
COPY --from=builder /usr/src/app/target/release/diwop-begleitapp /app/diwop-begleitapp
COPY --from=builder /usr/src/app/models/multilingual-e5-base /app/models/multilingual-e5-base
COPY --from=builder /usr/src/app/data/few_shots_index.json.gz /app/data/few_shots_index.json.gz

USER 10001:10001

# Default port
ENV PORT=3000
EXPOSE $PORT

CMD ["/app/diwop-begleitapp"]
