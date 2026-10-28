# DiWop Begleit-App Backend

Welcome to the `begleit-app-backend` repository.

See [docs/implementation-details.md](docs/implementation-details.md) for technical information and [docs/diwop-api-contract.md](docs/diwop-api-contract.md) for the API contract.

## Cloud-Agnostic Container Architecture

The service is packaged as a cloud-agnostic Docker container designed to run unmodified across different cloud providers (such as AWS for test/staging environments and STACKIT for production) provisioned via OpenTofu (`infrastructure/`):

- **12-Factor Configuration**: Configured strictly through environment variables injected by the container runtime—without cloud-specific SDKs or parameter store lookups.
- **Standardized Observability**: Emits structured JSON logs to `stdout` and exports traces and metrics via OpenTelemetry (OTLP).
- **Orchestrator-Ready**: Provides an unauthenticated `/healthz` probe endpoint, handles `SIGTERM` for graceful shutdown and telemetry flushing, and runs as an unprivileged non-root user.

## Getting Started

Use the agentic skill [`/onboarding`](.agent/skills/onboarding/SKILL.md) to get started.

Start the server with `cargo run`.

Check the Swagger UI at `http://localhost:3000/open-api`.