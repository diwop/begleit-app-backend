# Tech Structure

This repository contains a Rust-based backend service. It acts as an integration and API layer, powered by the following core technologies:

- **Rust**: The core programming language used for high-performance and safe execution.
- **HTTP JSON APIs**: APIs are exposed as HTTP JSON REST endpoints using Axum.
- **Web Administration Interface**: Server-rendered HTML administration interface powered by Askama templates and semantic CSS at `/`, protected with `ADMIN_PASSWORD`.
- **Swagger UI**: The HTTP APIs are documented using OpenAPI and Swagger UI at `/open-api`.
- **Docker**: Cloud-agnostic containerization for portable deployment across AWS and STACKIT.

## Cloud-Agnostic Container Design

The Docker image is built to run identically across AWS (ECS / App Runner / EKS) and STACKIT (SKE / Cloud Foundry) without code or image changes:

- **No Cloud SDKs in Binary**: All configuration and secrets are injected as environment variables by the orchestrator (e.g., via Terraform). No runtime lookups to AWS Parameter Store, Secrets Manager, or STACKIT APIs are performed by the application.
- **Observability**:
  - **Logs**: Written to `stdout` (structured JSON in `--release`, human-readable in debug mode) for native ingestion by cloud log collectors, including extracted dialog `topic`, `classification_type`, and `classification_category` on `/translate`.
  - **Traces & Metrics**: Exported via OTLP (gRPC) whenever `OTEL_EXPORTER_OTLP_ENDPOINT` is configured (HTTP request counts/durations, `gen_ai.client.token.usage` across `/correct-transcript`, `/translate`, and `/translate/classify`, `diwop.translate.history_length`, and `diwop.translate.classification.count`).
- **Two-Stage Translation & EU AI Act / Safety Classification**:
  - `POST /correct-transcript` runs with `"minimal"` reasoning effort for low-latency transcript cleanup.
  - `POST /translate` runs the preparation prompt (`"medium"` reasoning) followed sequentially by the dialog classification prompt (`"low"` reasoning + JSON Schema `responseJsonSchema`) to extract the dialog topic, attach static *Leichte Sprache* disclaimers (`prompts/warnings/*.md`), or block unsafe content (`safety`, `youth-protection`). See [**Dialog Classification, EU AI Act Disclaimers & Topic Monitoring**](classification-and-disclaimers.md).
- **Health Check & Lifecycle**:
  - `GET /healthz` returns `200 OK` without authentication for process liveness probes.
  - `GET /readyz` returns `200 OK` without authentication for container readiness probes. On startup, the server initiates an asynchronous warmup check verifying both the embedded semantic few-shot vector index (`intfloat/multilingual-e5-base`) and the configured LLM API. The probe result is cached in-memory so recurring health checks do not incur LLM API costs or rate-limiting overhead.
  - Handles `SIGTERM` and `SIGINT` to drain in-flight HTTP requests and flush OpenTelemetry trace/metric batches before exiting.
- **Dynamic Few-Shot Vector Lookup (Build-Time Precomputation)**:
  - Raw training dialogs are tracked via DVC (`data/raw_dialogs.dvc` pointing to `s3://diwop-leichte-sprache/dvc-hsuper`), with 10 representative dialogs tracked in `tests/fixtures/raw_dialogs/` for unit tests.
  - During `docker build`, `./target/release/diwop-begleitapp --prepare-few-shots` parses the raw dialogs into 6 history-length buckets (`0..=5` preceding `user`/`partner` turns), downloads `intfloat/multilingual-e5-base`, and precomputes normalized 768-dimensional embeddings into `data/few_shots_index.json.gz`.
  - At runtime, `/translate` embeds the incoming partner statement (`query: <message>`) in pure Rust (`candle-transformers` + `tokenizers`) and injects the top 3 semantically closest examples from the matching history bucket into `prompts/translate-dynamic-few-shots.md`.
- **Zero-Downtime Rolling Deployments**:
  - Deployments add a new container instance first and only terminate the previous instance after `/readyz` passes.
  - On AWS ECS Fargate: `deployment_minimum_healthy_percent = 100`, `deployment_maximum_percent = 200`, ALB Target Group health check on `/readyz`, and circuit breaker with automatic rollback.
  - On STACKIT SKE: `RollingUpdate` strategy with `max_surge = 1` and `max_unavailable = 0`, Kubernetes `readinessProbe` on `/readyz`, and `livenessProbe` on `/healthz`.
- **Non-Root Execution**: Runs as unprivileged user `10001:10001` (`appuser`) in `debian:bookworm-slim` to satisfy Kubernetes `restricted` Pod Security Standards.

## Running modes

### `cargo run --release`

The _release_ profile is intended for production within the Docker container. It uses structured JSON logging on `stdout`.

Configuration via environment variables:

* `API_KEY` (required in release) - Bearer token required on protected API routes (`Authorization: Bearer <API_KEY>`)
* `ADMIN_PASSWORD` (required in release, defaults to `"admin"` in debug) - Password required to authenticate into the Web Administration interface at `/login`
* `GEMINI_API_KEY` (required in release, optional in debug) - LLM API key used by `rig-core` (or STACKIT AI Model Serving when deployed to STACKIT) for `/correct-transcript` and `/translate`. In debug mode, if unset, the server falls back to `EchoPromptRunner`.
* `ENVIRONMENT` (optional, default `production`) - Deployment environment (`production` or `staging`; setting `ENVIRONMENT=staging` enables inline `%...%` test directives on `/translate`)
* `PORT` (optional, default `3000`) - TCP port the HTTP server binds to (`0.0.0.0:<PORT>`)
* `RUST_LOG` (optional, default `diwop_begleitapp=info,tower_http=info`) - Log filter directive
* `OTEL_EXPORTER_OTLP_ENDPOINT` (optional) - OTLP collector endpoint URL for exporting traces and metrics (e.g., `http://localhost:4317`)
* `OTEL_SERVICE_NAME` / `OTEL_RESOURCE_ATTRIBUTES` (optional) - Standard OpenTelemetry resource metadata attributes

### `cargo run`

The _dev_ (debug) profile is intended for local development and uses human-readable colored logging (`diwop_begleitapp=debug,tower_http=debug` by default).
It is ready to go for local development and testing. All environment variables (including `API_KEY`) are optional.

## Infrastructure & Deployment (OpenTofu & GitHub Actions)

Infrastructure as Code is managed with **OpenTofu** under [`infrastructure/`](../infrastructure), sharing a common container contract between cloud targets:

- [`infrastructure/modules/app-config`](../infrastructure/modules/app-config): Shared module defining the container port (`3000`), `/healthz` path, and standard runtime/OpenTelemetry environment variables (`ENVIRONMENT` defaults to `"production"`).
- [`infrastructure/aws`](../infrastructure/aws): **Test / Staging environment** configuration on AWS using Amazon ECR, AWS SSM Parameter Store, Amazon ECS Fargate (`0.5 vCPU` / `2048 MB` `ARM64` on `FARGATE_SPOT` with overnight `00:00`–`08:00` `Europe/Berlin` downscaling to `0`), Application Load Balancer (ALB), ACM, and Route 53. See the [**AWS Setup Guide**](aws-setup.md).
- [`infrastructure/stackit`](../infrastructure/stackit): Sketched production deployment configuration targeting STACKIT Kubernetes Engine (SKE). See the [**STACKIT Setup Guide**](stackit-setup.md).
- [`.github/workflows/deploy.yml`](../.github/workflows/deploy.yml): Automated Continuous Deployment workflow triggered on pushes to `main` (and `workflow_dispatch`). Builds the Docker image, pushes it to the target container registry (Amazon ECR or STACKIT OCI Registry), and runs `tofu apply` on AWS (`DEPLOY_TARGET=aws`) or STACKIT (`DEPLOY_TARGET=stackit`).

## Agentic Coding

This project contains a set of agentic rules and skills in [`.agent/`](.agent) to support the setup and development process.

It utilizes specific agentic skills in [`.agent/skills`](.agent/skills) to assist with onboarding, installation, and GitHub repository configuration. These skills are designed to be run by AI coding assistants (like Google Antigravity or Anthropic Claude Code) to guide new developers.

- [`onboarding`](.agent/skills/onboarding/SKILL.md): Guides developers through installing necessary system requirements (`git`, `cargo`, `cargo nextest`, `tofu`) and setting up local MCP (Model Context Protocol) servers.
- [`initial-setup`](.agent/skills/initial-setup/SKILL.md): A guide for the initial project setup, including GitHub repository configuration and baseline application installation.
- [`configure-github-repo`](.agent/skills/configure-github-repo/SKILL.md): Sets up repository rules, merge strategies, issues, and wikis.

### Running the Onboarding Skill

If you are a new developer setting up this repository, it is highly suggested that you begin with the **Onboarding** skill to verify your system dependencies and configure the environment.

To run the onboarding skill using an agentic coding software like Antigravity or Claude Code, simply type the following slash command in the chat:

```text
/onboarding
```

The assistant will guide you step-by-step through installing any missing dependencies and ensuring your system is correctly prepared for local development.
