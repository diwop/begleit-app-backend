# Build stage
FROM rust:bookworm AS builder

WORKDIR /usr/src/app
COPY . .

# Build the application
RUN cargo build --release

# Production stage
FROM debian:bookworm-slim

# Install necessary runtime dependencies and create an unprivileged non-root user
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 appuser \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin appuser

WORKDIR /app

# Copy the build artifact from the builder stage
COPY --from=builder /usr/src/app/target/release/diwop-begleitapp /app/diwop-begleitapp

USER 10001:10001

# Default port
ENV PORT=3000
EXPOSE $PORT

CMD ["/app/diwop-begleitapp"]
