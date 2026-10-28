output "app_name" {
  description = "Normalized application name."
  value       = var.app_name
}

output "port" {
  description = "Container HTTP port."
  value       = var.port
}

output "health_check_path" {
  description = "HTTP path for container health probes."
  value       = var.health_check_path
}

output "environment_variables" {
  description = "Map of non-secret runtime environment variables for the container."
  value       = local.environment_variables
}
