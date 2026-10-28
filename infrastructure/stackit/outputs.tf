output "image_repository_url" {
  description = "STACKIT OCI container registry repository URL for pushing images."
  value       = local.image_repository_url
}

output "ske_cluster_name" {
  description = "Name of the STACKIT SKE cluster."
  value       = stackit_ske_cluster.app.name
}

output "namespace" {
  description = "Kubernetes namespace where the service is deployed."
  value       = kubernetes_namespace.app.metadata[0].name
}
