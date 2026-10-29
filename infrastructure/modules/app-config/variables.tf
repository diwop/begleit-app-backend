variable "app_name" {
  description = "Application name used for naming resources and OpenTelemetry service identification."
  type        = string
  default     = "diwop-begleitapp"
}

variable "environment" {
  description = "Deployment environment name (e.g., production, staging)."
  type        = string
  default     = "production"
}

variable "port" {
  description = "TCP port the container HTTP server listens on."
  type        = number
  default     = 3000
}

variable "health_check_path" {
  description = "Unauthenticated HTTP path used for liveness and readiness probes."
  type        = string
  default     = "/healthz"
}

variable "readiness_check_path" {
  description = "Unauthenticated HTTP path used for container readiness checks."
  type        = string
  default     = "/readyz"
}

variable "rust_log" {
  description = "Log filter directive for tracing_subscriber."
  type        = string
  default     = "diwop_begleitapp=info,tower_http=info"
}

variable "otel_exporter_otlp_endpoint" {
  description = "Optional OTLP gRPC collector endpoint URL for traces and metrics."
  type        = string
  default     = ""
}

variable "otel_resource_attributes" {
  description = "Optional OpenTelemetry resource attributes (key=value comma-separated list)."
  type        = string
  default     = ""
}
