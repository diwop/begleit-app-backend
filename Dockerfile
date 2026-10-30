# Build stage
FROM rust:bookworm AS builder

WORKDIR /usr/src/app
COPY . .

# Build the application and precompute dynamic few-shot buckets and embeddings
RUN cargo build --release \
    && ./target/release/diwop-begleitapp --prepare-few-shots

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
