# STACKIT Deployment Guide – Sketch (OpenTofu)

This document describes the sketched OpenTofu deployment for **STACKIT** (the German Cloud Provider) located in [`infrastructure/stackit`](../infrastructure/stackit).

> [!NOTE]
> The STACKIT configuration is currently an architecture sketch validated in CI (`tofu validate`) and ready to be connected to a live STACKIT project when migrating or running multi-cloud.

## Architecture on STACKIT

Unlike AWS App Runner, STACKIT hosts containerized workloads primarily on **STACKIT Kubernetes Engine (SKE)**:

1. **Shared Application Contract (`../modules/app-config`)**:
   - Reuses the exact same port (`3000`), `/healthz` path, and non-secret OpenTelemetry / logging environment variables (`PORT`, `RUST_LOG`, `OTEL_SERVICE_NAME`, `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_RESOURCE_ATTRIBUTES`) as the AWS deployment.
2. **SKE Cluster (`stackit_ske_cluster` & `stackit_ske_kubeconfig`)**:
   - Provisions a managed Kubernetes cluster in region `eu01` using the official `stackitcloud/stackit` OpenTofu provider.
3. **Kubernetes Resources (`hashicorp/kubernetes`)**:
   - **`kubernetes_namespace`**: Isolates the workload per environment (`diwop-begleitapp-staging`).
   - **`kubernetes_secret`**: Stores `API_KEY`, `GEMINI_API_KEY` (LLM API credentials, targeting STACKIT AI Model Serving), and `ADMIN_PASSWORD` (web administration interface password), and injects them into the pod as environment variables via `secret_key_ref`.
   - **`kubernetes_deployment`**: Runs the container as non-root (`10001:10001`) with zero-downtime rolling updates (`RollingUpdate` strategy with `max_surge = 1`, `max_unavailable = 0`), a `liveness_probe` targeting `GET /healthz:3000`, and a `readiness_probe` targeting `GET /readyz:3000` (which verifies LLM API connectivity on startup before receiving traffic).
   - **`kubernetes_service`**: Exposes the deployment within the cluster / via a STACKIT LoadBalancer.

## Prerequisites (When Activating STACKIT Deployment)

1. **STACKIT Project & Service Account**:
   - A STACKIT `project_id` (UUID).
   - A Service Account key (`STACKIT_SERVICE_ACCOUNT_KEY_PATH` or `STACKIT_SERVICE_ACCOUNT_TOKEN`) with permissions for SKE and Object Storage.
2. **Bootstrap S3-Compatible State Bucket on STACKIT Object Storage**:
   - Create a bucket in STACKIT Object Storage (`https://object.storage.eu01.onstackit.cloud`) and generate S3 credentials (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`).
   - Initialize OpenTofu:
     ```bash
     tofu -chdir=infrastructure/stackit init \
       -backend-config="bucket=<stackit-tfstate-bucket>" \
       -backend-config="endpoints={s3=\"https://object.storage.eu01.onstackit.cloud\"}"
     ```
3. **Container Registry**:
   - Push the `linux/amd64` Docker image to the STACKIT OCI Container Registry (`registry.onstackit.cloud/<project>/diwop-begleitapp-<environment>:<tag>`) and pass its tag via `-var="image_tag=..."`.

## Automated GitHub Actions Deployment (`.github/workflows/deploy.yml`)

The `deploy-stackit` job in [`.github/workflows/deploy.yml`](../.github/workflows/deploy.yml) runs automatically on `main` when `DEPLOY_TARGET=stackit` is configured in the GitHub repository variables:

### GitHub Repository Variables
| Variable | Example Value | Description |
| :--- | :--- | :--- |
| `DEPLOY_TARGET` | `stackit` | Enables the `deploy-stackit` job on `main` |
| `STACKIT_PROJECT_ID` | `<project-uuid>` | STACKIT project UUID |
| `STACKIT_REGION` | `eu01` | STACKIT region (defaults to `eu01`) |
| `STACKIT_TF_STATE_BUCKET` | `<stackit-tfstate-bucket>` | S3-compatible bucket on STACKIT Object Storage |
| `STACKIT_REGISTRY_HOST` | `registry.onstackit.cloud` | STACKIT OCI container registry hostname (defaults to `registry.onstackit.cloud`) |
| `STACKIT_REGISTRY_PROJECT` | `diwop` | STACKIT Harbor / OCI registry project name (defaults to `diwop`) |
| `STACKIT_REGISTRY_USERNAME` | `<robot-account-username>` | STACKIT OCI container registry Robot Account username |
| `ENVIRONMENT` | `production` | Passed as `TF_VAR_environment` (defaults to `production`) |

### GitHub Repository Secrets
| Secret | Description |
| :--- | :--- |
| `API_KEY` | Bearer token stored in the Kubernetes Secret and injected into the pod |
| `ADMIN_PASSWORD` | Admin password for the web administration interface stored in the Kubernetes Secret |
| `GEMINI_API_KEY` | LLM API credentials (STACKIT AI Model Serving token) stored in the Kubernetes Secret and injected into the pod |
| `STACKIT_SERVICE_ACCOUNT_KEY` | STACKIT Service Account JSON key for the `stackitcloud/stackit` OpenTofu provider |
| `STACKIT_S3_ACCESS_KEY_ID` | Access key ID for the STACKIT Object Storage state bucket |
| `STACKIT_S3_SECRET_ACCESS_KEY` | Secret access key for the STACKIT Object Storage state bucket |
| `STACKIT_REGISTRY_PASSWORD` | STACKIT OCI container registry Robot Account password / token |
