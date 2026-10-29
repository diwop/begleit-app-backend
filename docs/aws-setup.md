# AWS Deployment Guide – Test / Staging Environment (OpenTofu)

This guide explains how to deploy `diwop-begleitapp` to AWS using OpenTofu (`infrastructure/aws`), Amazon ECR, AWS Secrets Manager, Amazon ECS Fargate (`ARM64` on `FARGATE_SPOT`), Application Load Balancer (ALB), ACM, and Route 53.

> [!NOTE]
> **Designed for Test / Staging Environments**: The AWS infrastructure in [`infrastructure/aws`](../infrastructure/aws) is intentionally cost-optimized for temporary test and staging deployments:
> - Runs a single `0.25 vCPU` (`256`) / `512 MB` `ARM64` container on **`FARGATE_SPOT`** in the default VPC (avoiding NAT Gateway costs).
> - Automatically scales down to **`0` tasks overnight (`00:00`–`08:00` `Europe/Berlin`)** and scales back up to `1` task at `08:00`.
> - Even when used for staging inside a shared AWS account, `var.environment` defaults to `"production"` so that containers never accidentally enable staging-only `%...%` test directives unless `environment = "staging"` is explicitly passed.

## 1. Prerequisites

1. **Installed Tools**:
   - `aws` CLI v2 (authenticated via `aws sso login` or IAM credentials)
   - `docker` (with `buildx` support for `linux/arm64`)
   - `tofu` (OpenTofu `>= 1.6.0`)

2. **Bootstrap the OpenTofu State S3 Bucket (One-Time Setup)**:
   OpenTofu requires an existing S3 bucket to store remote state before running `tofu init`. Create a versioned, encrypted bucket in `eu-central-1`:

   ```bash
   export TF_STATE_BUCKET="diwop-begleitapp-tfstate-<your-account-id>"
   export AWS_REGION="eu-central-1"

   aws s3api create-bucket \
     --bucket "$TF_STATE_BUCKET" \
     --region "$AWS_REGION" \
     --create-bucket-configuration LocationConstraint="$AWS_REGION"

   # Enable versioning (required for native S3 state locking via use_lockfile = true)
   aws s3api put-bucket-versioning \
     --bucket "$TF_STATE_BUCKET" \
     --versioning-configuration Status=Enabled

   # Enable default server-side encryption
   aws s3api put-bucket-encryption \
     --bucket "$TF_STATE_BUCKET" \
     --server-side-encryption-configuration '{
       "Rules": [{"ApplyServerSideEncryptionByDefault": {"SSEAlgorithm": "AES256"}}]
     }'

   # Block all public access
   aws s3api put-public-access-block \
     --bucket "$TF_STATE_BUCKET" \
     --public-access-block-configuration \
       BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
   ```

## 2. Initialize OpenTofu

Initialize the `infrastructure/aws` directory with your S3 state bucket:

```bash
tofu -chdir=infrastructure/aws init \
  -backend-config="bucket=$TF_STATE_BUCKET" \
  -backend-config="region=$AWS_REGION"
```

## 3. Create ECR Repository & Push Container Image

First, provision the Amazon ECR repository so you have a target registry for the Docker image:

```bash
export AWS_PROFILE="diwop-production"
export TF_VAR_environment="staging"
export TF_VAR_api_key="<generate-a-strong-api-key>"

tofu -chdir=infrastructure/aws apply -target=aws_ecr_repository.app
```

Authenticate Docker with ECR, build the `linux/arm64` container image from the repository root, and push it:

```bash
export ECR_URL=$(tofu -chdir=infrastructure/aws output -raw ecr_repository_url)

aws ecr get-login-password --region "$AWS_REGION" | \
  docker login --username AWS --password-stdin "$ECR_URL"

docker build --platform linux/arm64 -t "$ECR_URL:latest" .
docker push "$ECR_URL:latest"
```

## 4. Deploy Full AWS Stack

Apply the complete configuration to create the AWS Secrets Manager secret, least-privilege IAM roles, ACM certificate + Route 53 validation, Application Load Balancer, ECS Fargate `ARM64` `FARGATE_SPOT` service (`0.25 vCPU`, `512 MB`), and scheduled scaling (`00:00` down to `0`, `08:00` up to `1` `Europe/Berlin`):

```bash
tofu -chdir=infrastructure/aws apply -var="image_tag=latest"
```

## 5. Verify the Deployment

Retrieve the public HTTPS URLs (`service_url` and `custom_domain_url`) and verify the health probe and OpenAPI UI:

```bash
export SERVICE_URL=$(tofu -chdir=infrastructure/aws output -raw custom_domain_url)

# Unauthenticated health probe
curl -i "$SERVICE_URL/healthz"

# Swagger UI
open "$SERVICE_URL/open-api"
```

## 6. Automated GitHub Actions Deployment (`.github/workflows/deploy.yml`)

Pushes to `main` (or manual `workflow_dispatch` runs) automatically build the `linux/arm64` Docker image, push it to Amazon ECR, and apply the OpenTofu configuration when enabled via GitHub repository variables and secrets:

### Required GitHub Repository Variables (`Settings -> Secrets and variables -> Actions -> Variables`)
| Variable | Example Value | Description |
| :--- | :--- | :--- |
| `DEPLOY_TARGET` | `aws` | Enables the `deploy-aws` job on `main` |
| `AWS_ROLE_ARN` | `arn:aws:iam::224204046216:role/diwop-begleitapp-staging-github-actions-deploy` | IAM Role ARN provisioned by OpenTofu (`github_actions_role_arn` output) for GitHub OIDC authentication |
| `TF_STATE_BUCKET` | `diwop-begleitapp-tfstate-224204046216` | S3 bucket storing the OpenTofu state |
| `AWS_REGION` | `eu-central-1` | Target AWS region (defaults to `eu-central-1`) |
| `ENVIRONMENT` | `staging` | Passed as `TF_VAR_environment` (defaults to `staging` in the AWS job) |
| `HOSTED_ZONE_NAME` | `diwop.wulf.technology` | Existing Route 53 public hosted zone (defaults to `diwop.wulf.technology`) |

### Required GitHub Repository Secrets (`Settings -> Secrets and variables -> Actions -> Secrets`)
| Secret | Description |
| :--- | :--- |
| `API_KEY` | Bearer token stored in AWS Secrets Manager and injected into the container |

### Setting the `GEMINI_API_KEY` in AWS SSM Parameter Store
OpenTofu automatically provisions a `SecureString` parameter at `/<app_name>-<environment>/gemini-api-key` (for example, `/diwop-begleitapp-staging/gemini-api-key`) with `ignore_changes = [value]` and injects it into the ECS container as `GEMINI_API_KEY`. Populate the real Gemini API key directly in AWS Systems Manager Parameter Store:

```bash
aws ssm put-parameter \
  --region "$AWS_REGION" \
  --name "/diwop-begleitapp-staging/gemini-api-key" \
  --type "SecureString" \
  --value "<your-gemini-api-key>" \
  --overwrite
```
