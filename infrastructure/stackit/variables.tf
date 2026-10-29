variable "project_id" {
  description = "STACKIT project UUID."
  type        = string
}

variable "stackit_region" {
  description = "STACKIT region (e.g., eu01)."
  type        = string
  default     = "eu01"
}

variable "environment" {
  description = "Deployment environment name (e.g., production, staging)."
  type        = string
  default     = "production"
}

variable "registry_host" {
  description = "STACKIT OCI container registry hostname."
  type        = string
  default     = "registry.onstackit.cloud"
}

variable "registry_project_name" {
  description = "STACKIT Harbor / OCI registry project name."
  type        = string
  default     = "diwop"
}

variable "image_tag" {
  description = "Docker image tag in the STACKIT OCI registry to deploy."
  type        = string
  default     = "latest"
}

variable "api_key" {
  description = "Bearer API key required by the application in release mode."
  type        = string
  sensitive   = true
}

variable "gemini_api_key" {
  description = "Gemini API key required by the application in release mode."
  type        = string
  default     = "REPLACE_ME"
  sensitive   = true
}

variable "otel_exporter_otlp_endpoint" {
  description = "Optional OTLP gRPC collector endpoint URL inside the SKE cluster."
  type        = string
  default     = ""
}
