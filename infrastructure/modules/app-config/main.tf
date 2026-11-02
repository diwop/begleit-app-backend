locals {
  base_env = {
    ENVIRONMENT       = var.environment
    PORT              = tostring(var.port)
    RUST_LOG          = var.rust_log
    OTEL_SERVICE_NAME = var.app_name
  }

  otel_endpoint_env = var.otel_exporter_otlp_endpoint != "" ? {
    OTEL_EXPORTER_OTLP_ENDPOINT = var.otel_exporter_otlp_endpoint
  } : {}

  otel_attributes_env = var.otel_resource_attributes != "" ? {
    OTEL_RESOURCE_ATTRIBUTES = var.otel_resource_attributes
  } : {}

  metrics_provider_env = var.metrics_provider != "" ? {
    METRICS_PROVIDER = var.metrics_provider
  } : {}

  cloudwatch_namespace_env = var.cloudwatch_metrics_namespace != "" ? {
    CLOUDWATCH_METRICS_NAMESPACE = var.cloudwatch_metrics_namespace
  } : {}

  environment_variables = merge(
    local.base_env,
    local.otel_endpoint_env,
    local.otel_attributes_env,
    local.metrics_provider_env,
    local.cloudwatch_namespace_env,
  )
}
