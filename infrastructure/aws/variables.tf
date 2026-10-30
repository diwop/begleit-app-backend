variable "aws_region" {
  description = "AWS region to deploy resources into."
  type        = string
  default     = "eu-central-1"
}

variable "aws_profile" {
  description = "Optional AWS CLI profile name for authentication."
  type        = string
  default     = null
}

variable "environment" {
  description = "Deployment environment name (e.g., production, staging)."
  type        = string
  default     = "production"
}

variable "hosted_zone_name" {
  description = "Existing Route 53 public hosted zone domain name."
  type        = string
  default     = "diwop.wulf.technology"
}

variable "custom_domain_name" {
  description = "Optional custom domain name override (defaults to <environment>.<hosted_zone_name>, e.g., staging.diwop.wulf.technology)."
  type        = string
  default     = ""
}

variable "image_tag" {
  description = "Docker image tag in the ECR repository to deploy."
  type        = string
  default     = "latest"
}

variable "api_key" {
  description = "Bearer API key required by the application in release mode."
  type        = string
  sensitive   = true
}

variable "otel_exporter_otlp_endpoint" {
  description = "Optional OTLP gRPC collector endpoint URL for traces and metrics."
  type        = string
  default     = ""
}

variable "cpu" {
  description = "CPU units allocated to each ECS Fargate task (512 = 0.5 vCPU)."
  type        = string
  default     = "512"
}

variable "memory" {
  description = "Memory (in MB) allocated to each ECS Fargate task."
  type        = string
  default     = "2048"
}

variable "github_repository" {
  description = "GitHub repository (owner/repo) allowed to assume the OIDC deployment role on main."
  type        = string
  default     = "diwop/begleit-app"
}
