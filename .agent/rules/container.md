---
trigger: always_on
---

# Cloud-Agnostic Container Rules

This application is deployed as a portable Docker container across multiple cloud environments (such as AWS and STACKIT). Every change to the server or `Dockerfile` **MUST** preserve cloud-agnostic portability:

1. **Configuration via Environment Variables Only (12-Factor App)**:
   - All runtime configuration and secrets **MUST** be read exclusively from environment variables (`std::env::var`).
   - **NEVER** add cloud-provider-specific SDKs or runtime lookups (e.g., AWS SSM Parameter Store, AWS Secrets Manager, or STACKIT APIs) into the application binary. Secrets are injected into the container environment by the orchestrator (Terraform / Kubernetes / ECS).
2. **Standardized Observability (OpenTelemetry & `stdout` JSON Logs)**:
   - Traces and metrics **MUST** be exported via OTLP using standard OpenTelemetry environment variables (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`).
   - Application logs **MUST** be written to `stdout` (structured JSON in release builds, human-readable in debug builds) so any container runtime or log collector (AWS CloudWatch, STACKIT / Kubernetes log agents) can ingest them without vendor-specific log shippers in the image.
3. **Orchestrator Lifecycle & Health Checks**:
   - Keep `GET /healthz` unauthenticated (`200 OK`) so load balancers and container orchestrators (AWS ALB/ECS, STACKIT SKE liveness/readiness probes) can verify container health.
   - Maintain graceful shutdown on `SIGTERM` and `SIGINT` to drain in-flight HTTP requests and flush pending OpenTelemetry batches before process exit.
4. **Statelessness & Hardened Runtime Image**:
   - The container **MUST** remain stateless and never rely on local persistent disk storage.
   - The `Dockerfile` **MUST** run the application binary as an unprivileged non-root user (`UID 10001`) to comply with Kubernetes `restricted` Pod Security Standards on STACKIT and AWS security best practices.
